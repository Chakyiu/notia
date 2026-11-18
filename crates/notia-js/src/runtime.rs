//! One community plugin's JavaScript: its QuickJS runtime, and the calls
//! between it and Notia.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use gpui_kit::{
    App, Entity, EntityId, Task, Window,
    component::{WindowExt as _, notification::Notification},
};
use notia_plugin::{
    Command, NavigationIcon, Notia, Panel, PluginContext, PluginHost, RibbonItem, SettingsTab,
    StatusItem, VaultEvent, WorkspaceEvent, workspace,
};
use rquickjs::{
    CatchResultExt as _, Ctx, Error, Exception, Function, Module, Promise,
    loader::{ImportAttributes, Loader, Resolver},
    module::Declared,
};
use serde_json::{Value, json};

use crate::{
    host::{Args, note_json},
    manifest::PluginInfo,
    scope,
    settings::JsSettings,
    view::StatusText,
};

/// How long one call into a plugin may run before it is stopped.
const TIME_LIMIT: Duration = Duration::from_secs(5);
/// How much memory the JavaScript of one plugin may use.
const MEMORY_LIMIT: usize = 256 * 1024 * 1024;

const PRELUDE: &str = include_str!("prelude.js");
const API_MODULE: &str = "notia";
const API_SOURCE: &str = r#"const api = globalThis[Symbol.for("notia")];
export const { Plugin, Notice, ui, requestUrl } = api;
export default api;
"#;

/// Something a plugin registered, waiting to be handed to its
/// [`PluginContext`].
pub(crate) enum Contribution {
    Command(Command),
    RibbonItem(RibbonItem),
    Panel(Panel),
    StatusItem(StatusItem),
    SettingsTab(SettingsTab),
    NavigationIcon(NavigationIcon),
}

impl Contribution {
    fn add_to(self, ctx: &mut PluginContext) {
        match self {
            Self::Command(command) => ctx.add_command(command),
            Self::RibbonItem(item) => ctx.add_ribbon_item(item),
            Self::Panel(panel) => ctx.add_panel(panel),
            Self::StatusItem(item) => ctx.add_status_item(item),
            Self::SettingsTab(tab) => ctx.add_settings_tab(tab),
            Self::NavigationIcon(icon) => ctx.add_navigation_icon(icon),
        }
    }
}

pub(crate) struct PanelSlot {
    pub(crate) entity: EntityId,
    pub(crate) dirty: Rc<Cell<bool>>,
}

/// What the host functions keep between calls.
#[derive(Default)]
pub(crate) struct HostState {
    /// Whether the plugin is loading. Its contributions are handed to the
    /// plugin context once it is done; later ones extend the plugin.
    pub(crate) loading: bool,
    pub(crate) pending: Vec<Contribution>,
    pub(crate) status_items: Vec<Entity<StatusText>>,
    pub(crate) panels: HashMap<String, PanelSlot>,
    pub(crate) settings: HashMap<String, Rc<JsSettings>>,
    /// The panel or settings tab whose `render` is running.
    pub(crate) rendering: Option<String>,
    /// The events that have listeners.
    pub(crate) listened: HashSet<String>,
    /// Events that arrived while the plugin was running, such as those its
    /// own changes to the vault cause.
    queued_events: Vec<(String, Value)>,
    pub(crate) intervals: HashMap<u64, Task<()>>,
    /// HTTP calls started by the plugin that have not finished.
    pub(crate) in_flight: usize,
    last_error: Option<(String, Instant)>,
    pub(crate) file_icons: crate::file_icons::FileIconSlot,
}

pub(crate) struct JsRuntime {
    pub(crate) plugin: Rc<PluginInfo>,
    // Declared before the runtime, so that it is dropped first.
    context: rquickjs::Context,
    _runtime: rquickjs::Runtime,
    deadline: Rc<Cell<Option<Instant>>>,
    running: Cell<bool>,
    pub(crate) unloaded: Cell<bool>,
    pub(crate) state: RefCell<HostState>,
}

impl JsRuntime {
    /// Creates the plugin's runtime, evaluates its main module and runs its
    /// `onload`.
    pub(crate) fn load(plugin: Rc<PluginInfo>, cx: &mut App) -> Result<Rc<Self>, String> {
        let source = fs::read(plugin.dir.join(&plugin.main))
            .map_err(|error| format!("Could not read {}: {error}", plugin.main))?;

        let runtime = rquickjs::Runtime::new().map_err(|error| error.to_string())?;
        runtime.set_memory_limit(MEMORY_LIMIT);
        let deadline = Rc::new(Cell::new(None::<Instant>));
        runtime.set_interrupt_handler(Some(Box::new({
            let deadline = deadline.clone();
            move || {
                deadline
                    .get()
                    .is_some_and(|deadline| Instant::now() >= deadline)
            }
        })));
        let modules = PluginModules {
            dir: plugin.dir.clone(),
        };
        runtime.set_loader(modules.clone(), modules);
        let context = rquickjs::Context::full(&runtime).map_err(|error| error.to_string())?;

        let js = Rc::new(Self {
            plugin,
            context,
            _runtime: runtime,
            deadline,
            running: Cell::new(false),
            unloaded: Cell::new(false),
            state: RefCell::new(HostState {
                loading: true,
                ..HostState::default()
            }),
        });
        let weak = Rc::downgrade(&js);
        let main = js.plugin.main.clone();
        let manifest = js.plugin.manifest_json.clone();
        js.run(cx, move |ctx| {
            let host = Function::new(
                ctx.clone(),
                move |ctx: Ctx<'_>, method: String, args: String| {
                    host_function(&weak, &ctx, &method, &args)
                },
            )?;
            ctx.globals().set("__notiaHost", host)?;
            ctx.eval::<(), _>(PRELUDE)?;

            let (module, evaluated) = Module::declare(ctx.clone(), main, source)?.eval()?;
            settle(ctx, &evaluated)?;
            let start: Function = ctx.globals().get("__notiaStart")?;
            let loaded: Promise = start.call((module.namespace()?, manifest))?;
            settle(ctx, &loaded)
        })?;
        js.state.borrow_mut().loading = false;
        Ok(js)
    }

    /// Hands what `onload` registered to `ctx`, and forwards events to the
    /// plugin until it is disabled.
    pub(crate) fn attach(self: &Rc<Self>, ctx: &mut PluginContext, cx: &mut App) {
        let pending = std::mem::take(&mut self.state.borrow_mut().pending);
        for contribution in pending {
            contribution.add_to(ctx);
        }

        let weak = Rc::downgrade(self);
        ctx.keep_subscription(cx.subscribe(
            &Notia::workspace(cx),
            move |_, event: &WorkspaceEvent, cx| {
                let Some(js) = weak.upgrade() else {
                    return;
                };
                let name = match event {
                    WorkspaceEvent::ActiveNoteChanged => "workspace:file-open",
                    WorkspaceEvent::ActiveNoteEdited => "workspace:editor-change",
                    _ => return,
                };
                if js.listens(name) {
                    let note = note_json(workspace::active_editor(cx).as_ref());
                    js.emit(name, json!([note]), cx);
                }
            },
        ));

        let weak = Rc::downgrade(self);
        ctx.keep_subscription(
            cx.subscribe(&Notia::vault(cx), move |_, event: &VaultEvent, cx| {
                let Some(js) = weak.upgrade() else {
                    return;
                };
                match event {
                    VaultEvent::NotesChanged(paths) => {
                        for path in paths {
                            js.emit("vault:modify", json!([path.as_str()]), cx);
                        }
                    }
                    VaultEvent::Renamed { from, to } => {
                        js.emit("vault:rename", json!([to.as_str(), from.as_str()]), cx)
                    }
                    VaultEvent::Trashed(path) => {
                        js.emit("vault:delete", json!([path.as_str()]), cx)
                    }
                    VaultEvent::TreeChanged => {}
                }
                js.emit("vault:change", json!([]), cx);
            }),
        );

        let js = self.clone();
        ctx.on_deactivate(move |cx| js.unload(cx));
    }

    /// Calls `onunload` and stops the plugin's timers.
    fn unload(&self, cx: &mut App) {
        // Color replacements live on the theme, not on the plugin's
        // contributions, so disabling the plugin has to clear them itself.
        notia_plugin::theme::ThemeCatalog::clear_color_overrides(&self.plugin.id, cx);
        self.state.borrow_mut().listened.clear();
        if let Err(error) = self.dispatch("unload", "", "", cx) {
            self.report(&error, cx);
        }
        self.unloaded.set(true);
        let state = &mut *self.state.borrow_mut();
        state.intervals.clear();
        state.pending.clear();
    }

    /// Runs a command or ribbon icon callback.
    pub(crate) fn call_back(&self, callback: u64, cx: &mut App) {
        if let Err(error) = self.dispatch("callback", &callback.to_string(), "", cx) {
            self.report(&error, cx);
        }
    }

    pub(crate) fn fire_timer(&self, timer: u64, cx: &mut App) {
        if let Err(error) = self.dispatch("timer", &timer.to_string(), "", cx) {
            self.report(&error, cx);
        }
    }

    /// Runs the `render` of a panel or settings tab and returns the elements
    /// it made, as JSON.
    pub(crate) fn render_view(&self, view: &str, cx: &mut App) -> Result<String, String> {
        self.state.borrow_mut().rendering = Some(view.to_string());
        let result = self.dispatch("render", view, "", cx);
        self.state.borrow_mut().rendering = None;
        result?.ok_or_else(|| "The plugin was disabled.".to_string())
    }

    /// Runs a handler of an element of a view, such as a button's `onClick`
    /// or a toggle's `onChange`, with `args`.
    pub(crate) fn call_handler(&self, view: &str, handler: usize, args: Vec<Value>, cx: &mut App) {
        let mut payload = vec![Value::from(handler)];
        payload.extend(args);
        let payload = Value::Array(payload).to_string();
        if let Err(error) = self.dispatch("ui", view, &payload, cx) {
            self.report(&error, cx);
        }
    }

    fn listens(&self, event: &str) -> bool {
        self.state.borrow().listened.contains(event)
    }

    fn emit(&self, event: &str, args: Value, cx: &mut App) {
        if !self.listens(event) {
            return;
        }
        if self.running.get() {
            self.state
                .borrow_mut()
                .queued_events
                .push((event.to_string(), args));
            return;
        }
        if let Err(error) = self.dispatch("event", event, &args.to_string(), cx) {
            self.report(&error, cx);
        }
    }

    pub(crate) fn dispatch(
        &self,
        kind: &str,
        target: &str,
        payload: &str,
        cx: &mut App,
    ) -> Result<Option<String>, String> {
        if self.unloaded.get() {
            return Ok(None);
        }
        let result = self.run(cx, |ctx| {
            let dispatch: Function = ctx.globals().get("__notiaDispatch")?;
            dispatch.call((kind, target, payload))
        });
        loop {
            let next = {
                let mut state = self.state.borrow_mut();
                (!state.queued_events.is_empty()).then(|| state.queued_events.remove(0))
            };
            let Some((event, args)) = next else {
                break;
            };
            self.emit(&event, args, cx);
        }
        result
    }

    /// Runs `f` in the plugin's context, with `cx` lent to the host
    /// functions, then the promise jobs it queued.
    fn run<R>(
        &self,
        cx: &mut App,
        f: impl for<'js> FnOnce(&Ctx<'js>) -> rquickjs::Result<R>,
    ) -> Result<R, String> {
        // QuickJS cannot be entered twice, and nothing a plugin calls runs
        // its JavaScript synchronously, so this only guards against bugs.
        if self.running.replace(true) {
            return Err("The plugin is already running.".into());
        }
        let result = scope::lend(cx, || {
            self.deadline.set(Some(Instant::now() + TIME_LIMIT));
            let result = self.context.with(|ctx| {
                let result = f(&ctx).catch(&ctx).map_err(|error| error.to_string());
                while ctx.execute_pending_job() {}
                result
            });
            // An interrupted promise job leaves its promise pending, so the
            // call may even seem to have succeeded.
            let timed_out = self
                .deadline
                .take()
                .is_some_and(|deadline| Instant::now() >= deadline);
            if timed_out {
                return Err(format!(
                    "Stopped after running for more than {} seconds.",
                    TIME_LIMIT.as_secs()
                ));
            }
            result
        });
        self.running.set(false);
        self.extend_plugin(cx);
        result
    }

    /// Hands contributions registered after loading, by a timer for
    /// example, to the plugin host.
    fn extend_plugin(&self, cx: &mut App) {
        let pending = {
            let mut state = self.state.borrow_mut();
            if state.loading || self.unloaded.get() {
                return;
            }
            std::mem::take(&mut state.pending)
        };
        if !pending.is_empty() {
            PluginHost::extend(&self.plugin.id, cx, |ctx| {
                for contribution in pending {
                    contribution.add_to(ctx);
                }
            });
        }
    }

    /// Logs a failure and tells the user, unless the same failure was just
    /// shown, as happens when an event handler keeps failing.
    pub(crate) fn report(&self, error: &str, cx: &mut App) {
        log::error!("{} ({}): {error}", self.plugin.name, self.plugin.id);
        let summary = error.lines().next().unwrap_or(error).to_string();
        let repeated = {
            let mut state = self.state.borrow_mut();
            let repeated = state.last_error.as_ref().is_some_and(|(last, at)| {
                *last == summary && at.elapsed() < Duration::from_secs(10)
            });
            state.last_error = Some((summary.clone(), Instant::now()));
            repeated
        };
        if !repeated {
            notify(
                Notification::error(format!("{}: {summary}", self.plugin.name)),
                cx,
            );
        }
    }
}

/// Runs promise jobs until `promise` settles, and fails if it was rejected.
/// A promise that is still pending, like an `onload` waiting for a timer,
/// counts as success.
fn settle<'js>(ctx: &Ctx<'js>, promise: &Promise<'js>) -> rquickjs::Result<()> {
    while ctx.execute_pending_job() {}
    match promise.result::<rquickjs::Value>() {
        Some(Err(error)) => Err(error),
        _ => Ok(()),
    }
}

/// The function behind `__notiaHost`: decodes the arguments, borrows the
/// `App` and turns failures into JavaScript exceptions.
fn host_function<'js>(
    js: &Weak<JsRuntime>,
    ctx: &Ctx<'js>,
    method: &str,
    args: &str,
) -> rquickjs::Result<String> {
    let result = match js.upgrade() {
        None => Err("The plugin was disabled.".to_string()),
        Some(js) => serde_json::from_str(args)
            .map_err(|error| error.to_string())
            .and_then(|args| {
                scope::with_app(|cx| js.host_call(method, Args(args), cx))
                    .unwrap_or_else(|| Err("Notia is busy.".into()))
            }),
    };
    match result {
        Ok(value) => Ok(value.to_string()),
        Err(message) => Err(Exception::throw_message(ctx, &message)),
    }
}

/// Runs `f` with the active window, or any window, once the current update
/// is over.
pub(crate) fn with_window_later(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx
            .active_window()
            .or_else(|| cx.windows().into_iter().next());
        if let Some(window) = window {
            window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}

pub(crate) fn notify(notification: Notification, cx: &mut App) {
    with_window_later(cx, move |window, cx| {
        window.push_notification(notification, cx)
    });
}

/// Resolves `import`s: `"notia"` is the plugin API, and relative paths are
/// files in the plugin's own folder.
#[derive(Clone)]
struct PluginModules {
    dir: PathBuf,
}

impl Resolver for PluginModules {
    fn resolve<'js>(
        &mut self,
        _: &Ctx<'js>,
        base: &str,
        name: &str,
        _: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        if name == API_MODULE {
            return Ok(name.into());
        }
        if !name.starts_with("./") && !name.starts_with("../") {
            return Err(Error::new_resolving_message(
                base,
                name,
                r#"plugins can import "notia" and files in their own folder"#,
            ));
        }
        let mut segments: Vec<&str> = base.split('/').collect();
        segments.pop();
        for segment in name.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    if segments.pop().is_none() {
                        return Err(Error::new_resolving_message(
                            base,
                            name,
                            "the file is outside the plugin folder",
                        ));
                    }
                }
                segment => segments.push(segment),
            }
        }
        let path = segments.join("/");
        [
            path.clone(),
            format!("{path}.js"),
            format!("{path}/index.js"),
        ]
        .into_iter()
        .find(|candidate| self.dir.join(candidate).is_file())
        .ok_or_else(|| Error::new_resolving_message(base, name, "no such file"))
    }
}

impl Loader for PluginModules {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, Declared>> {
        if name == API_MODULE {
            return Module::declare(ctx.clone(), name, API_SOURCE);
        }
        let source = fs::read(self.dir.join(name))
            .map_err(|error| Error::new_loading_message(name, error.to_string()))?;
        Module::declare(ctx.clone(), name, source)
    }
}
