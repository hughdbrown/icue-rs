//! Page ↔ [`PluginHost`] transport over the widget's own `icuew://` origin.
//!
//! The shim's `window.plugins.<Facade>` objects use three routes, each answered with JSON on a
//! background thread:
//! - `GET /__edgehost/plugin/reset` → `{"seq": n}`: the page (re)loaded; collect events after `n`.
//! - `GET /__edgehost/plugin/call?p=<facade>&m=<method>&a=<JSON array>` → `{"value": …}` or
//!   `{"error": "…"}`.
//! - `GET /__edgehost/plugin/events?after=<n>` → `[{seq, plugin, signal, args}, …]`, held open up
//!   to [`POLL_TIMEOUT`] until there is something to deliver.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use edge_plugins::PluginHost;
use serde_json::{Value, json};
use wry::RequestAsyncResponder;
use wry::http::{Response, header};

use crate::assets::percent_decode;

/// Path prefix of the plugin routes inside a widget's origin.
pub const PREFIX: &str = "/__edgehost/plugin/";
/// How long an `events` request waits for a signal before answering with none.
pub const POLL_TIMEOUT: Duration = Duration::from_secs(25);

/// Answer a plugin route on its own thread. `host` is `None` for views without plugins.
pub fn handle(
    host: Option<Arc<PluginHost>>,
    label: String,
    route: String,
    query: Option<String>,
    responder: RequestAsyncResponder,
) {
    std::thread::spawn(move || {
        let (status, body) = match host {
            Some(host) => answer(&host, &label, &route, query.as_deref()),
            None => (404, json!({"error": "this widget has no plugins"})),
        };
        responder.respond(json_response(status, &body));
    });
}

fn answer(host: &PluginHost, label: &str, route: &str, query: Option<&str>) -> (u16, Value) {
    let param = |name: &str| -> Option<String> {
        query?
            .split('&')
            .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
            .and_then(percent_decode)
    };
    match route {
        "reset" => (200, json!({"seq": host.reset()})),
        "events" => {
            let after = param("after").and_then(|a| a.parse().ok()).unwrap_or(0);
            let events = host.wait_events(after, POLL_TIMEOUT);
            (200, serde_json::to_value(events).unwrap_or_default())
        }
        "call" => {
            let (Some(plugin), Some(method)) = (param("p"), param("m")) else {
                return (400, json!({"error": "missing p or m"}));
            };
            let args: Vec<Value> = match param("a").as_deref().map(serde_json::from_str) {
                None => Vec::new(),
                Some(Ok(args)) => args,
                Some(Err(e)) => return (400, json!({"error": format!("bad arguments: {e}")})),
            };
            match host.call(&plugin, &method, &args) {
                Ok(value) => (200, json!({"value": value})),
                Err(e) => {
                    eprintln!("[{label}] {plugin}.{method}: {e}");
                    (200, json!({"error": e.to_string()}))
                }
            }
        }
        _ => (
            404,
            json!({"error": format!("unknown plugin route {route}")}),
        ),
    }
}

fn json_response(status: u16, body: &Value) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Cow::Owned(body.to_string().into_bytes()))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"{}".as_slice())))
}

#[cfg(test)]
mod tests {
    use edge_plugins::HostContext;
    use edge_plugins::paths::Macros;
    use icuew_manifest::{PathPermission, PathPermissionKind, PluginRef};

    use super::*;

    #[test]
    fn routes_reset_calls_and_events() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("a.txt"), "hi").unwrap();
        let fs: PluginRef = "widgetbuilder.filesystemprovider:FileSystem:1.0"
            .parse()
            .unwrap();
        let host = PluginHost::new(
            &[fs],
            &HostContext {
                macros: Macros::new(&root, &root),
                path_permissions: vec![PathPermission {
                    kind: PathPermissionKind::Folder,
                    path: "<Home>".into(),
                }],
            },
        );
        assert_eq!(answer(&host, "w", "reset", None), (200, json!({"seq": 0})));
        let call = "p=Filesystemprovider&m=watchPath&a=%5B%22%7BHome%7D%2Fa.txt%22%5D";
        assert_eq!(
            answer(&host, "w", "call", Some(call)),
            (200, json!({"value": null}))
        );
        let (status, events) = answer(&host, "w", "events", Some("after=0"));
        assert_eq!(status, 200);
        assert_eq!(events[0]["signal"], "fileChanged");
        assert_eq!(events[0]["args"][1], "hi");

        let denied = "p=Filesystemprovider&m=watchPath&a=%5B%22%2Fetc%2Fhosts%22%5D";
        let (_, body) = answer(&host, "w", "call", Some(denied));
        assert!(body["error"].as_str().unwrap().contains("permissions"));
        assert_eq!(answer(&host, "w", "call", Some("p=X")).0, 400);
        assert_eq!(answer(&host, "w", "call", Some("p=X&m=y&a=nope")).0, 400);
        assert_eq!(answer(&host, "w", "bogus", None).0, 404);
    }
}
