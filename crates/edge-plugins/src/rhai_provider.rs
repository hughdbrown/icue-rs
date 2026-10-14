//! Runs one add-in's `main.rhai` as a [`Provider`].
//!
//! Each provider owns a sandboxed `rhai::Engine` on its own thread; [`Provider::call`] hands
//! requests to that thread and waits for the answer. The script defines any of:
//!
//! - `fn init()`: runs at start and whenever the page reloads, with a fresh, empty `this` map;
//! - `fn handle(method, args)`: a widget called `window.plugins.<Facade>.<method>(…args)`; the
//!   return value (any JSON-like value) resolves the widget's promise, a `throw` rejects it;
//! - `fn poll()`: runs every `poll_ms` (the first time right after `init`).
//!
//! All three share state through `this` (`this.watched = []`). Host functions:
//!
//! | Function | Does |
//! |---|---|
//! | `emit(signal)`, `emit(signal, [args…])` | Deliver a declared signal to the widget |
//! | `http_get(url)` | GET a URL on a host in `permissions.http`; returns the body, throws on failure or status ≥ 400 |
//! | `parse_json(text)`, `to_json(value)` | Convert between JSON text and Rhai values |
//! | `now_ms()` | Milliseconds since the Unix epoch |
//! | `print(…)`, `debug(…)` | Write to EdgeHost's log |
//!
//! There is no module loading, no `eval`, and no file, process or raw network access. Each entry
//! point runs under [`MAX_OPERATIONS`], [`CALL_BUDGET`] of script time (time inside `http_get` is
//! not counted) and size limits. A script that exceeds a limit gets a fresh `this` and `init()`
//! runs again; a failing `poll()` backs off up to [`MAX_BACKOFF`].

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rhai::{AST, Array, CallFnOptions, Dynamic, Engine, EvalAltResult, FuncArgs, Map, Scope};
use serde_json::Value;

use crate::addin::{Addin, HttpGrant};
use crate::{Emitter, PluginError, Provider};

/// Operations one entry point may run.
pub const MAX_OPERATIONS: u64 = 5_000_000;
/// Script time one entry point may use.
pub const CALL_BUDGET: Duration = Duration::from_secs(2);
/// Longest wait between polls after repeated failures.
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How long a widget call waits for the script (including its `http_get`s).
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout of one `http_get`.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest `http_get` body.
const MAX_BODY_BYTES: u64 = 1024 * 1024;

/// A sandboxed engine with EdgeHost's limits and no host functions.
fn sandbox_engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new())
        .disable_symbol("eval")
        .set_max_operations(MAX_OPERATIONS)
        .set_max_call_levels(64)
        .set_max_expr_depths(64, 32)
        .set_max_string_size(1024 * 1024)
        .set_max_array_size(10_000)
        .set_max_map_size(10_000);
    engine
}

/// Compile `source` and list its functions as `(name, parameter count)`.
///
/// # Errors
///
/// The compiler's message.
pub fn script_functions(source: &str) -> Result<BTreeSet<(String, usize)>, String> {
    let ast = sandbox_engine()
        .compile(source)
        .map_err(|e| e.to_string())?;
    Ok(ast
        .iter_functions()
        .map(|f| (f.name.to_owned(), f.params.len()))
        .collect())
}

enum Request {
    Call {
        method: String,
        args: Vec<Value>,
        reply: SyncSender<Result<Value, PluginError>>,
    },
    Reset,
}

/// An add-in running for one widget instance. Dropping it stops the script's thread.
pub struct RhaiProvider {
    addin: Arc<Addin>,
    tx: Sender<Request>,
}

impl RhaiProvider {
    /// Start `addin`'s script on its own thread. Signals go to `emit`.
    ///
    /// # Errors
    ///
    /// [`PluginError::Failed`] if the thread cannot be started.
    pub fn start(addin: Arc<Addin>, emit: Emitter) -> Result<Self, PluginError> {
        let (tx, rx) = mpsc::channel();
        let worker_addin = Arc::clone(&addin);
        std::thread::Builder::new()
            .name(format!("addin {}", addin.id))
            .spawn(move || run(&worker_addin, &emit, &rx))
            .map_err(|e| PluginError::Failed(format!("starting add-in {}: {e}", addin.id)))?;
        Ok(Self { addin, tx })
    }
}

impl Provider for RhaiProvider {
    fn call(&self, method: &str, args: &[Value]) -> Result<Value, PluginError> {
        if !self.addin.methods.iter().any(|m| m == method) {
            return Err(PluginError::UnknownMethod(method.to_owned()));
        }
        let stopped = || PluginError::Failed(format!("add-in {} has stopped", self.addin.id));
        let (reply, answer) = mpsc::sync_channel(1);
        self.tx
            .send(Request::Call {
                method: method.to_owned(),
                args: args.to_vec(),
                reply,
            })
            .map_err(|_| stopped())?;
        answer.recv_timeout(REPLY_TIMEOUT).map_err(|e| match e {
            RecvTimeoutError::Timeout => {
                PluginError::Failed(format!("add-in {} did not answer in time", self.addin.id))
            }
            RecvTimeoutError::Disconnected => stopped(),
        })?
    }

    fn reset(&self) {
        // Fails only if the thread is gone, which `call` reports.
        let _ = self.tx.send(Request::Reset);
    }
}

/// The script thread: answer requests and run `poll()` on schedule until the provider is dropped.
fn run(addin: &Arc<Addin>, emit: &Emitter, rx: &Receiver<Request>) {
    let mut worker = match Worker::new(Arc::clone(addin), emit) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[addin {}] {e}", addin.id);
            // Answer calls with the error until the provider goes away.
            while let Ok(request) = rx.recv() {
                if let Request::Call { reply, .. } = request {
                    let _ = reply.send(Err(PluginError::Failed(e.clone())));
                }
            }
            return;
        }
    };
    worker.restart();
    let mut failures = 0_u32;
    let mut next_poll = addin.poll.map(|_| Instant::now());
    loop {
        let request = match next_poll {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match request {
            Ok(Request::Call {
                method,
                args,
                reply,
            }) => {
                let _ = reply.send(worker.call(&method, &args));
            }
            Ok(Request::Reset) => {
                worker.restart();
                failures = 0;
                next_poll = addin.poll.map(|_| Instant::now());
            }
            Err(RecvTimeoutError::Timeout) => {
                failures = if worker.poll() { 0 } else { failures + 1 };
                next_poll = addin
                    .poll
                    .map(|interval| Instant::now() + backoff(interval, failures));
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The wait before the next poll after `failures` failures in a row.
fn backoff(interval: Duration, failures: u32) -> Duration {
    let grown = interval.saturating_mul(1 << failures.min(10));
    grown.min(MAX_BACKOFF.max(interval))
}

struct Worker {
    addin: Arc<Addin>,
    engine: Engine,
    ast: AST,
    scope: Scope<'static>,
    this: Dynamic,
    deadline: Rc<Cell<Instant>>,
    has_init: bool,
}

impl Worker {
    fn new(addin: Arc<Addin>, emit: &Emitter) -> Result<Self, String> {
        let deadline = Rc::new(Cell::new(Instant::now() + CALL_BUDGET));
        let mut engine = sandbox_engine();
        let label = addin.id.clone();
        engine.on_print(move |s| eprintln!("[addin {label}] {s}"));
        let label = addin.id.clone();
        engine.on_debug(move |s, _, pos| eprintln!("[addin {label}] debug {pos}: {s}"));
        {
            let deadline = Rc::clone(&deadline);
            engine.on_progress(move |ops| {
                (ops % 256 == 0 && Instant::now() > deadline.get())
                    .then(|| Dynamic::from("script time budget exceeded"))
            });
        }
        register_emit(&mut engine, &addin, emit);
        register_http(&mut engine, &addin.http, &deadline);
        engine.register_fn(
            "parse_json",
            |text: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                let value: Value =
                    serde_json::from_str(text).map_err(|e| format!("parse_json: {e}"))?;
                rhai::serde::to_dynamic(value)
            },
        );
        engine.register_fn(
            "to_json",
            |value: Dynamic| -> Result<String, Box<EvalAltResult>> {
                let value: Value = rhai::serde::from_dynamic(&value)?;
                Ok(value.to_string())
            },
        );
        engine.register_fn("now_ms", || -> i64 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        });
        let ast = engine.compile(&addin.source).map_err(|e| e.to_string())?;
        let has_init = ast
            .iter_functions()
            .any(|f| f.name == "init" && f.params.is_empty());
        Ok(Self {
            addin,
            engine,
            ast,
            scope: Scope::new(),
            this: Dynamic::from_map(Map::new()),
            deadline,
            has_init,
        })
    }

    fn log(&self, what: &str, e: &EvalAltResult) {
        eprintln!("[addin {}] {what}: {e}", self.addin.id);
    }

    /// Fresh state: run the script's top level, then `init()`.
    fn restart(&mut self) {
        self.this = Dynamic::from_map(Map::new());
        self.scope = Scope::new();
        self.deadline.set(Instant::now() + CALL_BUDGET);
        if let Err(e) = self.engine.run_ast_with_scope(&mut self.scope, &self.ast) {
            self.log("top level", &e);
        }
        if self.has_init {
            if let Err(e) = self.invoke("init", ()) {
                self.log("init()", &e);
            }
        }
    }

    fn invoke(&mut self, name: &str, args: impl FuncArgs) -> Result<Dynamic, Box<EvalAltResult>> {
        self.deadline.set(Instant::now() + CALL_BUDGET);
        let options = CallFnOptions::new()
            .eval_ast(false)
            .rewind_scope(true)
            .bind_this_ptr(&mut self.this);
        self.engine
            .call_fn_with_options(options, &mut self.scope, &self.ast, name, args)
    }

    /// After a limit violation the script's state may be inconsistent: start it over.
    fn after_error(&mut self, what: &str, e: &EvalAltResult) {
        self.log(what, e);
        if is_limit(e) {
            eprintln!(
                "[addin {}] restarting after a limit violation",
                self.addin.id
            );
            self.restart();
        }
    }

    fn call(&mut self, method: &str, args: &[Value]) -> Result<Value, PluginError> {
        let args: Array = args
            .iter()
            .map(|a| rhai::serde::to_dynamic(a).unwrap_or(Dynamic::UNIT))
            .collect();
        match self.invoke("handle", (method.to_owned(), args)) {
            Ok(result) => rhai::serde::from_dynamic::<Value>(&result).map_err(|e| {
                PluginError::Failed(format!("{method} returned a value that is not JSON: {e}"))
            }),
            Err(e) => {
                let message = widget_message(&e);
                self.after_error(&format!("handle({method:?})"), &e);
                Err(PluginError::Failed(message))
            }
        }
    }

    /// Run `poll()`; `false` if it failed.
    fn poll(&mut self) -> bool {
        match self.invoke("poll", ()) {
            Ok(_) => true,
            Err(e) => {
                self.after_error("poll()", &e);
                false
            }
        }
    }
}

/// What the widget's rejected promise says: the innermost error, without Rhai's call-stack
/// wrapping, and a thrown value as itself (`throw "bad pair"` → `bad pair`). The log keeps the
/// full trace.
fn widget_message(e: &EvalAltResult) -> String {
    let mut inner = e;
    while let EvalAltResult::ErrorInFunctionCall(_, _, next, _) = inner {
        inner = next;
    }
    match inner {
        EvalAltResult::ErrorRuntime(value, _) => value.to_string(),
        other => {
            let mut text = other.to_string();
            // Drop the " (line …, position …)" suffix.
            if let Some(at) = text.rfind(" (line ") {
                text.truncate(at);
            }
            text
        }
    }
}

fn is_limit(e: &EvalAltResult) -> bool {
    matches!(
        e,
        EvalAltResult::ErrorTooManyOperations(_)
            | EvalAltResult::ErrorTerminated(..)
            | EvalAltResult::ErrorStackOverflow(_)
            | EvalAltResult::ErrorDataTooLarge(..)
    )
}

fn register_emit(engine: &mut Engine, addin: &Arc<Addin>, emit: &Emitter) {
    let send = {
        let addin = Arc::clone(addin);
        let emit = Arc::clone(emit);
        Rc::new(
            move |signal: &str, args: Array| -> Result<(), Box<EvalAltResult>> {
                if !addin.signals.iter().any(|s| s == signal) {
                    return Err(format!("emit: {signal:?} is not in addin.toml's signals").into());
                }
                let args = args
                    .iter()
                    .map(rhai::serde::from_dynamic::<Value>)
                    .collect::<Result<Vec<_>, _>>()?;
                emit(signal, args);
                Ok(())
            },
        )
    };
    let with_args = Rc::clone(&send);
    engine.register_fn("emit", move |signal: &str, args: Array| {
        with_args(signal, args)
    });
    engine.register_fn("emit", move |signal: &str| send(signal, Array::new()));
}

fn register_http(engine: &mut Engine, grants: &[HttpGrant], deadline: &Rc<Cell<Instant>>) {
    let grants = grants.to_vec();
    let deadline = Rc::clone(deadline);
    engine.register_fn(
        "http_get",
        move |url: &str| -> Result<String, Box<EvalAltResult>> {
            let started = Instant::now();
            let result = http_get(&grants, url);
            // Waiting for the network does not count against the script's time budget.
            deadline.set(deadline.get() + started.elapsed());
            result.map_err(|e| format!("http_get: {e}").into())
        },
    );
}

/// GET `url` if a grant covers it.
fn http_get(grants: &[HttpGrant], url: &str) -> Result<String, String> {
    let lower = url.to_ascii_lowercase();
    let default_port = if lower.starts_with("https://") {
        443
    } else if lower.starts_with("http://") {
        80
    } else {
        return Err(format!("{url:?} is not an http(s) URL"));
    };
    let uri: ureq::http::Uri = url.parse().map_err(|_| format!("{url:?} is not a URL"))?;
    let host = uri
        .host()
        .ok_or_else(|| format!("{url:?} has no host"))?
        .to_ascii_lowercase();
    let port = uri.port_u16().unwrap_or(default_port);
    if !grants.iter().any(|g| g.covers(&host, port, default_port)) {
        return Err(format!(
            "{host}:{port} is not in the add-in's permissions.http"
        ));
    }
    // Only a host written as loopback, which a grant must name with its port, may reach this
    // machine; any other name that resolves here is refused at connection time.
    let mut response = agent(crate::net::is_loopback_host(&host))
        .get(url)
        .call()
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        // Following it could reach a host the add-in did not declare.
        let to = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("?");
        return Err(format!(
            "HTTP {status} from {url} redirects to {to}; redirects are not followed, so request the final URL (its host must be in permissions.http)"
        ));
    }
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    if status >= 400 {
        return Err(format!("HTTP {status} from {url}"));
    }
    Ok(body)
}

/// The agent for public hosts, or for an explicitly granted loopback host (`local`).
fn agent(local: bool) -> &'static ureq::Agent {
    static PUBLIC: OnceLock<ureq::Agent> = OnceLock::new();
    static LOCAL: OnceLock<ureq::Agent> = OnceLock::new();
    let make = || {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(HTTP_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(concat!("EdgeHost/", env!("CARGO_PKG_VERSION")))
            .build();
        crate::net::agent(config, local)
    };
    if local {
        LOCAL.get_or_init(make)
    } else {
        PUBLIC.get_or_init(make)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;
    use crate::addin::{DESCRIPTOR_FILE, SCRIPT_FILE};

    type Signals = Arc<Mutex<Vec<(String, Vec<Value>)>>>;

    fn provider(toml_extra: &str, script: &str) -> (RhaiProvider, Signals) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("dev.test.t");
        std::fs::create_dir_all(&dir).unwrap();
        let toml = format!(
            "id = \"dev.test.t\"\nmodule = \"edgehost.t\"\nname = \"T\"\nversion = \"1.0\"\n\
             methods = [\"get\", \"add\", \"fail\", \"spin\", \"fetch\"]\nsignals = [\"changed\"]\n{toml_extra}"
        );
        std::fs::write(dir.join(DESCRIPTOR_FILE), toml).unwrap();
        std::fs::write(dir.join(SCRIPT_FILE), script).unwrap();
        let addin = Arc::new(Addin::load(Path::new(&dir)).unwrap());
        let signals: Signals = Arc::default();
        let sink = Arc::clone(&signals);
        let emit: Emitter = Arc::new(move |s: &str, a: Vec<Value>| {
            sink.lock().unwrap().push((s.to_owned(), a));
        });
        (RhaiProvider::start(addin, emit).unwrap(), signals)
    }

    const COUNTER: &str = r#"
        fn init() { this.n = 0; }
        fn handle(method, args) {
            switch method {
                "get" => this.n,
                "add" => { this.n += args[0]; emit("changed", [this.n, #{ by: args[0] }]); this.n }
                "fail" => throw "nope",
                "spin" => { loop { } }
                "fetch" => http_get(args[0]),
            }
        }
    "#;

    #[test]
    fn calls_keep_state_in_this_and_emit_signals() {
        let (p, signals) = provider("", COUNTER);
        assert_eq!(p.call("get", &[]), Ok(Value::from(0)));
        assert_eq!(p.call("add", &[Value::from(5)]), Ok(Value::from(5)));
        assert_eq!(p.call("add", &[Value::from(2)]), Ok(Value::from(7)));
        let got = signals.lock().unwrap().clone();
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].0, "changed");
        assert_eq!(got[1].1, vec![Value::from(7), serde_json::json!({"by": 2})]);
        assert_eq!(
            p.call("nosuch", &[]),
            Err(PluginError::UnknownMethod("nosuch".into()))
        );
    }

    #[test]
    fn throw_rejects_and_reset_starts_over() {
        let (p, _) = provider("", COUNTER);
        assert_eq!(p.call("fail", &[]), Err(PluginError::Failed("nope".into())));
        p.call("add", &[Value::from(3)]).unwrap();
        p.reset();
        assert_eq!(p.call("get", &[]), Ok(Value::from(0)));
    }

    #[test]
    fn runaway_scripts_are_stopped_and_restarted() {
        let (p, _) = provider("", COUNTER);
        p.call("add", &[Value::from(4)]).unwrap();
        let err = p.call("spin", &[]).unwrap_err();
        assert!(matches!(err, PluginError::Failed(_)), "{err:?}");
        assert_eq!(p.call("get", &[]), Ok(Value::from(0)), "state was reset");
    }

    #[test]
    fn sandbox_blocks_undeclared_hosts_signals_eval_and_imports() {
        let (p, _) = provider("", COUNTER);
        let err = p.call("fetch", &[Value::from("https://example.com/")]);
        assert!(
            matches!(&err, Err(PluginError::Failed(m)) if m.contains("permissions.http")),
            "{err:?}"
        );
        let err = p.call("fetch", &[Value::from("file:///etc/passwd")]);
        assert!(
            matches!(&err, Err(PluginError::Failed(m)) if m.contains("http(s)")),
            "{err:?}"
        );

        let (p, _) = provider("", r#"fn handle(m, a) { emit("other"); }"#);
        let err = p.call("get", &[]);
        assert!(
            matches!(&err, Err(PluginError::Failed(m)) if m.contains("signals")),
            "{err:?}"
        );

        assert!(script_functions(r#"fn handle(m, a) { eval("1") }"#).is_err());
        let (p, _) = provider("", r#"fn handle(m, a) { import "x" as x; 1 }"#);
        assert!(p.call("get", &[]).is_err());
    }

    /// A one-shot HTTP server on 127.0.0.1 answering every request with `response`.
    fn stub_server(response: &'static str) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(4) {
                let mut s = stream.unwrap();
                let mut buf = [0_u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(response.as_bytes());
            }
        });
        port
    }

    #[test]
    fn http_get_reads_granted_hosts_and_does_not_follow_redirects() {
        let ok =
            stub_server("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        let grant = |port: u16| {
            [crate::addin::HttpGrant {
                host: "127.0.0.1".into(),
                port: Some(port),
            }]
        };
        assert_eq!(
            http_get(&grant(ok), &format!("http://127.0.0.1:{ok}/x")),
            Ok("hello".into())
        );
        let moved = stub_server(
            "HTTP/1.1 302 Found\r\nLocation: http://evil.example/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let err = http_get(&grant(moved), &format!("http://127.0.0.1:{moved}/")).unwrap_err();
        assert!(err.contains("redirects to http://evil.example/"), "{err}");
        let missing =
            stub_server("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let err = http_get(&grant(missing), &format!("http://127.0.0.1:{missing}/")).unwrap_err();
        assert!(err.starts_with("HTTP 404"), "{err}");
    }

    #[test]
    fn http_get_cannot_reach_this_machine_through_a_non_loopback_name() {
        let port =
            stub_server("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi");
        let grant = [crate::addin::HttpGrant {
            host: "0.0.0.0".into(),
            port: Some(port),
        }];
        let err = http_get(&grant, &format!("http://0.0.0.0:{port}/")).unwrap_err();
        assert!(err.contains("resolves to this machine"), "{err}");
    }

    #[test]
    fn json_helpers_round_trip() {
        let (p, _) = provider(
            "",
            "fn handle(m, a) { let v = parse_json(a[0]); v.list.push(3); to_json(v) }",
        );
        let out = p.call("get", &[Value::from(r#"{"list":[1,2]}"#)]).unwrap();
        assert_eq!(out, Value::from(r#"{"list":[1,2,3]}"#));
    }

    #[test]
    fn poll_runs_on_schedule_and_backs_off() {
        let (p, signals) = provider(
            "poll_ms = 100",
            r#"fn init() { this.k = 0; } fn handle(m, a) { this.k } fn poll() { this.k += 1; emit("changed", [this.k]); }"#,
        );
        std::thread::sleep(Duration::from_millis(450));
        let n = signals.lock().unwrap().len();
        assert!((3..=6).contains(&n), "{n} polls");
        assert!(p.call("get", &[]).unwrap().as_i64().unwrap() >= 3);
        assert_eq!(backoff(Duration::from_secs(1), 0), Duration::from_secs(1));
        assert_eq!(backoff(Duration::from_secs(1), 3), Duration::from_secs(8));
        assert_eq!(backoff(Duration::from_secs(1), 30), MAX_BACKOFF);
        assert_eq!(
            backoff(Duration::from_secs(600), 2),
            Duration::from_secs(600)
        );
    }
}
