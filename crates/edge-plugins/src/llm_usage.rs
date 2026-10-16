//! `edgehost.llmusage:LlmUsage:1.0` (`window.plugins.Llmusage`): quota left and reset times of
//! the user's LLM subscriptions, from [`llm_usage::Service`].
//!
//! Methods: `snapshot()` → the last snapshot or `null` (never waits for the network);
//! `refresh()` → asks for a fetch now and returns `null`. Signal: `updated(snapshot)`, sent when
//! the provider starts, after each poll, after `refresh()`, and after a page reload.
//!
//! Every widget instance has a provider and a poll thread, but they share one process-wide
//! service, so several widgets cost one set of requests per poll interval.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use llm_usage::{Service, Snapshot};
use serde_json::Value;

use crate::{Descriptor, Emitter, PluginError, Provider};

pub(crate) const DESCRIPTOR: Descriptor = Descriptor {
    module: "edgehost.llmusage",
    name: "LlmUsage",
    major: 1,
    minor: 0,
    methods: &["snapshot", "refresh"],
    signals: &["updated"],
};

enum Command {
    /// Fetch now, then send `updated`.
    Refresh,
    /// Send `updated` with what is at hand (fetching only if it is older than the interval).
    Report,
}

/// The LLM Usage provider for one widget instance. Dropping it stops its poll thread.
pub struct LlmUsage {
    service: Arc<Service>,
    commands: Mutex<Sender<Command>>,
}

fn to_value(s: &Snapshot) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

impl LlmUsage {
    /// A provider over `service`, reporting through `emit`.
    #[must_use]
    pub fn new(service: Arc<Service>, emit: Emitter) -> Self {
        let (tx, rx) = mpsc::channel();
        let poller = Arc::clone(&service);
        std::thread::Builder::new()
            .name("llm-usage".into())
            .spawn(move || {
                let mut next = Command::Report;
                loop {
                    let snap = match next {
                        Command::Refresh => poller.refresh(),
                        Command::Report => poller.current(),
                    };
                    emit("updated", vec![to_value(&snap)]);
                    next = match rx.recv_timeout(poller.poll_interval()) {
                        Ok(c) => c,
                        Err(RecvTimeoutError::Timeout) => Command::Report,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                }
            })
            .map_err(|e| eprintln!("llm-usage: cannot start the poll thread: {e}"))
            .ok();
        Self {
            service,
            commands: Mutex::new(tx),
        }
    }

    fn send(&self, c: Command) {
        // A closed channel means the poll thread is gone; there is nothing more to do.
        let _ = self
            .commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(c);
    }
}

impl Provider for LlmUsage {
    fn call(&self, method: &str, _args: &[Value]) -> Result<Value, PluginError> {
        match method {
            "snapshot" => Ok(self.service.cached().map_or(Value::Null, |s| to_value(&s))),
            "refresh" => {
                self.send(Command::Refresh);
                Ok(Value::Null)
            }
            other => Err(PluginError::UnknownMethod(other.to_owned())),
        }
    }

    fn reset(&self) {
        // The page reloaded and its signals were dropped: send it the current numbers.
        self.send(Command::Report);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    use llm_usage::http::{Http, NetworkError, Request, Response};

    use super::*;

    struct NoNetwork;

    impl Http for NoNetwork {
        fn send(&self, req: &Request) -> Result<Response, NetworkError> {
            Err(NetworkError(format!("offline: {}", req.url)))
        }
    }

    fn provider() -> (LlmUsage, Receiver<(String, Vec<Value>)>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(Service::new(
            Some(dir.path().join("missing.toml")),
            Box::new(NoNetwork),
        ));
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let emit: Emitter = Arc::new(move |signal: &str, args: Vec<Value>| {
            let _ = tx.lock().unwrap().send((signal.to_owned(), args));
        });
        (LlmUsage::new(service, emit), rx, dir)
    }

    #[test]
    fn reports_on_start_reset_and_refresh() {
        let (p, rx, _dir) = provider();
        let (signal, args) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(signal, "updated");
        assert!(
            args[0]["configError"]
                .as_str()
                .unwrap()
                .contains("does not exist")
        );
        assert_eq!(p.call("snapshot", &[]).unwrap(), args[0]);

        p.reset();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap().0,
            "updated"
        );
        assert_eq!(p.call("refresh", &[]).unwrap(), Value::Null);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap().0,
            "updated"
        );
        assert_eq!(
            p.call("nope", &[]),
            Err(PluginError::UnknownMethod("nope".into()))
        );
    }

    #[test]
    fn dropping_the_provider_stops_its_thread() {
        let (p, rx, _dir) = provider();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(p);
        // The thread drops its emitter (the only sender) when it exits.
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(RecvTimeoutError::Disconnected)
        );
    }
}
