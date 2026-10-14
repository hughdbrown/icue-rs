//! The configurator window: an ordinary app window whose web UI (`apps/edgehost/ui/`, embedded at
//! build time) edits the layout through [`crate::config_api`].
//!
//! Requests arrive over IPC and are forwarded to the event loop as [`AppEvent::Config`]; replies and
//! "changed" events are delivered by evaluating `window.__configurator.*` in the page.

use std::borrow::Cow;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use widget_runtime::assets::{mime_for, read_asset};
use widget_runtime::platform;
use wry::http::{Response, header};
use wry::{WebView, WebViewBuilder};

use crate::AppEvent;

const SCHEME: &str = "edgecfg";

/// Forwards the page's errors to EdgeHost's log as `{"k":"console",…}` messages.
const CONSOLE_FORWARDER: &str = r#"(() => {
  const send = (level, msg) => { try { window.ipc.postMessage(JSON.stringify({ k: "console", level, msg: String(msg) })); } catch (_) {} };
  const original = console.error.bind(console);
  console.error = (...a) => { original(...a); send("error", a.map(String).join(" ")); };
  window.addEventListener("error", (e) => send("error", `${e.message} (${e.filename}:${e.lineno})`));
  window.addEventListener("unhandledrejection", (e) => send("error", `unhandled rejection: ${e.reason}`));
})();"#;

/// UI files embedded in the binary: (path, contents, MIME type).
const UI_FILES: [(&str, &str, &str); 3] = [
    (
        "index.html",
        include_str!("../ui/index.html"),
        "text/html; charset=utf-8",
    ),
    (
        "app.js",
        include_str!("../ui/app.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "app.css",
        include_str!("../ui/app.css"),
        "text/css; charset=utf-8",
    ),
];

/// The open configurator window.
pub struct Configurator {
    window: Window,
    view: WebView,
}

impl Configurator {
    /// Open the configurator. `widgets_dir` is the store's widget folder (for icons).
    ///
    /// # Errors
    ///
    /// If the window or web view cannot be created.
    pub fn open(
        target: &EventLoopWindowTarget<AppEvent>,
        proxy: EventLoopProxy<AppEvent>,
        widgets_dir: PathBuf,
        devtools: bool,
    ) -> Result<Self> {
        // Centre on the primary (non-Edge) display; otherwise macOS opens it on the active screen,
        // which is the Edge when the surface has focus.
        let size = LogicalSize::new(1180.0, 780.0);
        let mut builder = WindowBuilder::new()
            .with_title("EdgeHost — Widgets")
            .with_inner_size(size)
            .with_min_inner_size(LogicalSize::new(900.0, 600.0));
        if let Some(primary) = target.primary_monitor() {
            let scale = primary.scale_factor();
            let origin = primary.position().to_logical::<f64>(scale);
            let area = primary.size().to_logical::<f64>(scale);
            builder = builder.with_position(LogicalPosition::new(
                origin.x + ((area.width - size.width) / 2.0).max(0.0),
                origin.y + ((area.height - size.height) / 2.0).max(0.0),
            ));
        }
        let window = builder
            .build(target)
            .context("creating the configurator window")?;
        let builder = WebViewBuilder::new()
            .with_devtools(devtools)
            .with_initialization_script(CONSOLE_FORWARDER)
            .with_custom_protocol(SCHEME.to_owned(), move |_id, request| {
                serve(&widgets_dir, request.uri())
            })
            .with_ipc_handler(move |request| {
                // Fails only after the loop has exited.
                let _ = proxy.send_event(AppEvent::Config(request.body().clone()));
            })
            .with_url(format!("{SCHEME}://app/index.html"));
        let view =
            platform::attach_fill(&window, builder).context("creating the configurator view")?;
        window.set_focus();
        Ok(Self { window, view })
    }

    /// The window's id, to route window events.
    #[must_use]
    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }

    /// Bring the window to the front.
    pub fn focus(&self) {
        self.window.set_focus();
    }

    /// Answer request `id` with `result` or an error message.
    pub fn reply(&self, id: u64, result: Result<Value, String>) {
        let message = match result {
            Ok(result) => json!({ "id": id, "result": result }),
            Err(message) => json!({ "id": id, "error": { "message": message } }),
        };
        self.call("reply", &message);
    }

    /// Tell the UI that the layout or installed widgets changed elsewhere.
    pub fn notify_changed(&self) {
        self.call("event", &json!({ "type": "changed" }));
    }

    fn call(&self, function: &str, message: &Value) {
        let script = format!(
            "window.__configurator && window.__configurator.{function}({})",
            script_safe(message)
        );
        if let Err(e) = self.view.evaluate_script(&script) {
            eprintln!("configurator: {e}");
        }
    }
}

/// JSON that cannot close a `<script>` element or break a JS string.
fn script_safe(value: &Value) -> String {
    value
        .to_string()
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// `edgecfg://app/<file>` serves the embedded UI; `edgecfg://widget/<id>/<version>/<path>` serves
/// installed widget files (icons).
fn serve(widgets_dir: &std::path::Path, uri: &wry::http::Uri) -> Response<Cow<'static, [u8]>> {
    let path = uri.path().trim_start_matches('/');
    let result: Result<(Cow<'static, [u8]>, &'static str), u16> = match uri.host() {
        Some("app") => UI_FILES
            .iter()
            .find(|(name, ..)| *name == path)
            .map(|(_, body, mime)| (Cow::Borrowed(body.as_bytes()), *mime))
            .ok_or(404),
        Some("widget") => widget_file(widgets_dir, path),
        _ => Err(404),
    };
    let built = match result {
        Ok((body, mime)) => Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .body(body),
        Err(status) => Response::builder()
            .status(status)
            .body(Cow::Borrowed(b"not found".as_slice())),
    };
    built.unwrap_or_else(|_| Response::new(Cow::Borrowed(b"".as_slice())))
}

fn widget_file(
    widgets_dir: &std::path::Path,
    path: &str,
) -> Result<(Cow<'static, [u8]>, &'static str), u16> {
    let mut parts = path.splitn(3, '/');
    let (Some(id), Some(version), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(404);
    };
    let safe = |s: &str| !s.is_empty() && s != "." && !s.contains("..");
    if !safe(id) || !safe(version) {
        return Err(403);
    }
    let root = widgets_dir.join(id).join(version);
    read_asset(&root, rest)
        .map(|bytes| (Cow::Owned(bytes), mime_for(std::path::Path::new(rest))))
        .map_err(|e| e.status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_ui_and_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let get = |url: &str| serve(dir.path(), &url.parse().unwrap());
        assert_eq!(get("edgecfg://app/index.html").status(), 200);
        assert_eq!(get("edgecfg://app/missing.js").status(), 404);
        assert_eq!(get("edgecfg://widget/../x/icon.svg").status(), 403);
        assert_eq!(get("edgecfg://widget/a.b/1.0.0/icon.svg").status(), 404);
        assert_eq!(get("edgecfg://other/x").status(), 404);
    }

    #[test]
    fn script_safe_json() {
        let s = script_safe(&json!({ "m": "</script>\u{2028}" }));
        assert!(!s.contains("</script>") && s.contains("\\u2028"));
    }
}
