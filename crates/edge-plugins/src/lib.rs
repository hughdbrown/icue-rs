//! Plugins (data providers) for iCUE widgets: the native side of `window.plugins.<Facade>`.
//!
//! A [`PluginHost`] belongs to one widget instance. It creates the providers the widget's
//! `required_plugins` name, runs their methods ([`PluginHost::call`]) and queues their signals for
//! the page to collect ([`PluginHost::wait_events`]). The transport between the page and the host
//! lives in `widget-runtime`; nothing here depends on a web view.
//!
//! Built in: `widgetbuilder.filesystemprovider:FileSystem:1.0` ([`filesystem`]). Any other plugin
//! can come from a Rhai add-in ([`addin`], [`rhai_provider`]); built-ins win over add-ins.

pub mod addin;
pub mod filesystem;
pub mod net;
pub mod paths;
pub mod rhai_provider;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use icuew_manifest::{PathPermission, PluginRef};
use serde::Serialize;
use serde_json::Value;

use crate::addin::Registry;
use crate::filesystem::FileSystem;
use crate::paths::{Grants, Macros};
use crate::rhai_provider::RhaiProvider;

/// Signals kept for a page that is not collecting them; older ones are dropped.
const QUEUE_LIMIT: usize = 1000;

/// Why a plugin call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    /// The widget did not require this plugin, or EdgeHost does not provide it.
    #[error("plugin {0} is not available")]
    UnknownPlugin(String),
    /// The plugin has no such method.
    #[error("unknown method {0}")]
    UnknownMethod(String),
    /// Missing or mistyped arguments.
    #[error("bad arguments: {0}")]
    BadArguments(String),
    /// The widget has no permission for this resource.
    #[error("{0} is not covered by the widget's permissions")]
    NotPermitted(String),
    /// The provider could not do it.
    #[error("{0}")]
    Failed(String),
}

/// How a provider reports a signal: name and arguments.
pub type Emitter = Arc<dyn Fn(&str, Vec<Value>) + Send + Sync>;

/// A provider's methods. Signals go through the [`Emitter`] it was created with.
pub trait Provider: Send + Sync {
    /// Run `method` with `args`.
    ///
    /// # Errors
    ///
    /// Unknown method, bad arguments, or the provider's own failure.
    fn call(&self, method: &str, args: &[Value]) -> Result<Value, PluginError>;

    /// Forget all per-page state (the page was reloaded).
    fn reset(&self);
}

/// What a provider offers, as the manifest names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Descriptor {
    /// Module, e.g. `widgetbuilder.filesystemprovider`.
    pub module: &'static str,
    /// Plugin name, e.g. `FileSystem`.
    pub name: &'static str,
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Method names.
    pub methods: &'static [&'static str],
    /// Signal names.
    pub signals: &'static [&'static str],
}

impl Descriptor {
    /// Whether this provider satisfies `wanted`: same module and name, same major version, and a
    /// minor version at least as new.
    #[must_use]
    pub fn satisfies(&self, wanted: &PluginRef) -> bool {
        wanted.module == self.module
            && wanted.name == self.name
            && wanted.major == self.major
            && wanted.minor <= self.minor
    }
}

const PROVIDERS: &[Descriptor] = &[filesystem::DESCRIPTOR];

/// The built-in provider for `wanted`, if any. Matches module and name only, so an add-in cannot
/// provide another version of a built-in plugin.
#[must_use]
pub fn builtin(wanted: &PluginRef) -> Option<&'static Descriptor> {
    PROVIDERS
        .iter()
        .find(|d| d.module == wanted.module && d.name == wanted.name)
}

/// The JS facade the page builds for a supported plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Facade {
    /// `window.plugins` key, e.g. `Filesystemprovider`.
    pub name: String,
    /// Method names.
    pub methods: Vec<String>,
    /// Signal names.
    pub signals: Vec<String>,
}

/// The facade for `wanted`, if EdgeHost or one of `addins` provides it.
#[must_use]
pub fn facade(wanted: &PluginRef, addins: &Registry) -> Option<Facade> {
    let owned = |names: &[&str]| names.iter().map(|&n| n.to_owned()).collect();
    if let Some(d) = builtin(wanted) {
        return d.satisfies(wanted).then(|| Facade {
            name: wanted.facade_name(),
            methods: owned(d.methods),
            signals: owned(d.signals),
        });
    }
    addins.find(wanted).map(|a| Facade {
        name: wanted.facade_name(),
        methods: a.methods.clone(),
        signals: a.signals.clone(),
    })
}

/// A signal waiting for the page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Event {
    /// Position in this host's stream, starting at 1.
    pub seq: u64,
    /// Facade name.
    pub plugin: String,
    /// Signal name.
    pub signal: String,
    /// Signal arguments.
    pub args: Vec<Value>,
}

#[derive(Default)]
struct QueueState {
    events: VecDeque<Event>,
    last_seq: u64,
}

#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, plugin: &str, signal: &str, args: Vec<Value>) {
        let mut q = self.lock();
        q.last_seq += 1;
        let seq = q.last_seq;
        if q.events.len() == QUEUE_LIMIT {
            q.events.pop_front();
        }
        q.events.push_back(Event {
            seq,
            plugin: plugin.to_owned(),
            signal: signal.to_owned(),
            args,
        });
        self.ready.notify_all();
    }
}

/// What a widget instance may touch.
#[derive(Debug, Clone)]
pub struct HostContext {
    /// Path macro locations.
    pub macros: Macros,
    /// Declared `file` / `folder` permissions.
    pub path_permissions: Vec<PathPermission>,
    /// Installed add-ins.
    pub addins: Arc<Registry>,
}

/// The plugins of one widget instance.
pub struct PluginHost {
    providers: BTreeMap<String, Box<dyn Provider>>,
    queue: Arc<Queue>,
}

impl PluginHost {
    /// Create the providers `required` names that EdgeHost supports; others are left out (the page
    /// keeps their `plugin<Facade>_initialized` flag `false`).
    #[must_use]
    pub fn new(required: &[PluginRef], ctx: &HostContext) -> Self {
        let queue = Arc::new(Queue::default());
        let mut providers: BTreeMap<String, Box<dyn Provider>> = BTreeMap::new();
        for wanted in required {
            let Some(facade) = facade(wanted, &ctx.addins) else {
                continue;
            };
            let emit: Emitter = {
                let queue = Arc::clone(&queue);
                let plugin = facade.name.clone();
                Arc::new(move |signal: &str, args: Vec<Value>| queue.push(&plugin, signal, args))
            };
            if wanted.module == filesystem::DESCRIPTOR.module {
                let grants = Grants::new(&ctx.path_permissions, &ctx.macros);
                providers.insert(
                    facade.name,
                    Box::new(FileSystem::new(ctx.macros.clone(), grants, emit)),
                );
            } else if let Some(addin) = ctx.addins.find(wanted) {
                match RhaiProvider::start(Arc::clone(addin), emit) {
                    Ok(p) => {
                        providers.insert(facade.name, Box::new(p));
                    }
                    Err(e) => eprintln!("{e}"),
                }
            }
        }
        Self { providers, queue }
    }

    /// Whether any provider is running (otherwise the page needs no event stream).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Run `plugin.method(args)`.
    ///
    /// # Errors
    ///
    /// [`PluginError::UnknownPlugin`] for a plugin this instance does not have, otherwise the
    /// provider's error.
    pub fn call(&self, plugin: &str, method: &str, args: &[Value]) -> Result<Value, PluginError> {
        self.providers
            .get(plugin)
            .ok_or_else(|| PluginError::UnknownPlugin(plugin.to_owned()))?
            .call(method, args)
    }

    /// The page (re)loaded: drop provider state and pending signals. Returns the sequence number
    /// to collect after.
    #[must_use = "the page must collect events after the returned sequence number"]
    pub fn reset(&self) -> u64 {
        for p in self.providers.values() {
            p.reset();
        }
        let mut q = self.queue.lock();
        q.events.clear();
        q.last_seq
    }

    /// Signals after `after`, waiting up to `timeout` for the first one. Empty on timeout.
    #[must_use]
    pub fn wait_events(&self, after: u64, timeout: Duration) -> Vec<Event> {
        let deadline = Instant::now() + timeout;
        let mut q = self.queue.lock();
        loop {
            let ready: Vec<Event> = q.events.iter().filter(|e| e.seq > after).cloned().collect();
            let now = Instant::now();
            if !ready.is_empty() || now >= deadline {
                return ready;
            }
            q = self
                .queue
                .ready
                .wait_timeout(q, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn fs_ref(minor: u32) -> PluginRef {
        format!("widgetbuilder.filesystemprovider:FileSystem:1.{minor}")
            .parse()
            .unwrap()
    }

    fn host(dir: &Path, required: &[PluginRef]) -> PluginHost {
        PluginHost::new(
            required,
            &HostContext {
                macros: Macros::new(dir, dir),
                path_permissions: vec![PathPermission {
                    kind: icuew_manifest::PathPermissionKind::Folder,
                    path: "<Home>".into(),
                }],
                addins: Arc::default(),
            },
        )
    }

    #[test]
    fn version_negotiation() {
        let none = Registry::default();
        assert!(facade(&fs_ref(0), &none).is_some());
        assert!(
            facade(&fs_ref(1), &none).is_none(),
            "newer minor than provided"
        );
        let sensors = "widgetbuilder.sensorsdataprovider:Sensors:1.0"
            .parse()
            .unwrap();
        assert!(facade(&sensors, &none).is_none());
        let f = facade(&fs_ref(0), &none).unwrap();
        assert_eq!(f.name, "Filesystemprovider");
        assert!(f.methods.iter().any(|m| m == "watchPath"));
    }

    #[test]
    fn host_routes_calls_and_queues_signals() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("note.txt"), "hello").unwrap();
        let sensors: PluginRef = "widgetbuilder.sensorsdataprovider:Sensors:1.0"
            .parse()
            .unwrap();
        let h = host(&root, &[fs_ref(0), sensors]);
        assert!(!h.is_empty());
        assert_eq!(
            h.call("Sensorsdataprovider", "getAllSensorIds", &[]),
            Err(PluginError::UnknownPlugin("Sensorsdataprovider".into()))
        );
        h.call(
            "Filesystemprovider",
            "watchPath",
            &[Value::from("{Home}/note.txt")],
        )
        .unwrap();
        let events = h.wait_events(0, Duration::from_secs(5));
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[0].plugin, "Filesystemprovider");
        assert_eq!(events[0].signal, "fileChanged");
        assert_eq!(events[0].args[1], "hello");

        assert_eq!(h.wait_events(1, Duration::from_millis(50)), []);
        assert_eq!(h.reset(), 1);
        assert_eq!(h.wait_events(0, Duration::from_millis(50)), []);
    }

    #[test]
    fn queue_drops_the_oldest_beyond_its_limit() {
        let q = Queue::default();
        for _ in 0..QUEUE_LIMIT + 5 {
            q.push("P", "s", vec![]);
        }
        let state = q.lock();
        assert_eq!(state.events.len(), QUEUE_LIMIT);
        assert_eq!(state.events.front().map(|e| e.seq), Some(6));
    }
}
