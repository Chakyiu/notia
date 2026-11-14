//! What a plugin's JavaScript can ask of Notia. The prelude calls these by
//! name, with arguments and results as JSON.

use std::{
    cell::Cell,
    fs, io,
    rc::Rc,
    time::{Duration, UNIX_EPOCH},
};

use gpui_kit::{App, AppContext as _, Entity, component::notification::Notification};
use notia_plugin::{
    Command, Notia, OpenOptions, Panel, PanelPlacement, RibbonItem, StatusItem, VaultModel,
    core::{Vault, VaultPath},
    workspace,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    icon,
    runtime::{Contribution, JsRuntime, PanelSlot, active_note_json, notify, with_window_later},
    view::{JsPanel, StatusText},
};

/// Contributions of community plugins come after those of core plugins.
const ORDER: i32 = 100;

pub(crate) struct Args(pub(crate) Vec<Value>);

impl Args {
    fn get<T: DeserializeOwned>(&self, ix: usize) -> Result<T, String> {
        let value = self.0.get(ix).cloned().unwrap_or_default();
        serde_json::from_value(value).map_err(|error| format!("argument {}: {error}", ix + 1))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandSpec {
    id: String,
    name: String,
    icon: Option<String>,
    hotkey: Option<String>,
    needs_note: bool,
    callback: u64,
}

#[derive(Deserialize)]
struct RibbonSpec {
    id: String,
    icon: String,
    title: String,
    callback: u64,
}

#[derive(Deserialize)]
struct PanelSpec {
    id: String,
    title: String,
    icon: Option<String>,
    side: String,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct OpenSpec {
    line: Option<f64>,
    new_tab: bool,
}

impl JsRuntime {
    pub(crate) fn host_call(
        self: &Rc<Self>,
        method: &str,
        args: Args,
        cx: &mut App,
    ) -> Result<Value, String> {
        match method {
            "log" => {
                let level = match args.get::<String>(0)?.as_str() {
                    "error" => log::Level::Error,
                    "warn" => log::Level::Warn,
                    "debug" => log::Level::Debug,
                    _ => log::Level::Info,
                };
                let message: String = args.get(1)?;
                log::log!(target: "notia_js", level, "{}: {message}", self.plugin.id);
                Ok(Value::Null)
            }
            "error" => {
                self.report(&args.get::<String>(0)?, cx);
                Ok(Value::Null)
            }
            "notice" => {
                let message: String = args.get(0)?;
                notify(Notification::new().message(message), cx);
                Ok(Value::Null)
            }
            "listen" => {
                self.state.borrow_mut().listened.insert(args.get(0)?);
                Ok(Value::Null)
            }
            "addCommand" => self.add_command(args.get(0)?),
            "addRibbonIcon" => self.add_ribbon_icon(args.get(0)?),
            "addStatusBarItem" => Ok(self.add_status_item(cx).into()),
            "setStatusBarText" => {
                let (item, text): (usize, String) = (args.get(0)?, args.get(1)?);
                let item = self.state.borrow().status_items.get(item).cloned();
                let item = item.ok_or("there is no such status bar item")?;
                item.update(cx, |item, cx| item.set_text(text, cx));
                Ok(Value::Null)
            }
            "addPanel" => self.add_panel(args.get(0)?, cx),
            "refreshPanel" => {
                let id: String = args.get(0)?;
                let state = self.state.borrow();
                // Refreshing from `render` would render forever.
                if state.rendering.as_ref() != Some(&id) {
                    let slot = state
                        .panels
                        .get(&id)
                        .ok_or_else(|| format!("there is no panel “{id}”"))?;
                    slot.dirty.set(true);
                    let entity = slot.entity;
                    drop(state);
                    cx.notify(entity);
                }
                Ok(Value::Null)
            }
            "showPanel" => {
                let id = format!("{}:{}", self.plugin.id, args.get::<String>(0)?);
                with_window_later(cx, move |window, cx| workspace::show_panel(&id, window, cx));
                Ok(Value::Null)
            }
            "loadData" => self.load_data(),
            "saveData" => self.save_data(&args.get(0)?),
            "setTimer" => {
                let (id, delay, repeat): (u64, f64, bool) =
                    (args.get(0)?, args.get(1)?, args.get(2)?);
                self.set_timer(id, delay, repeat, cx);
                Ok(Value::Null)
            }
            "clearTimer" => {
                self.state
                    .borrow_mut()
                    .intervals
                    .remove(&args.get::<u64>(0)?);
                Ok(Value::Null)
            }
            _ if method.starts_with("vault.") => vault_call(method, &args, cx),
            _ if method.starts_with("workspace.") => self.workspace_call(method, &args, cx),
            _ if method.starts_with("metadata.") => metadata_call(method, &args, cx),
            _ => Err(format!("Notia has no function “{method}”")),
        }
    }

    fn add_command(self: &Rc<Self>, spec: CommandSpec) -> Result<Value, String> {
        check_id(&spec.id, "command")?;
        if spec.name.trim().is_empty() {
            return Err("a command needs a name".into());
        }
        let js = Rc::downgrade(self);
        let mut command = Command::new(spec.id, spec.name, move |_, cx| {
            if let Some(js) = js.upgrade() {
                js.call_back(spec.callback, cx);
            }
        });
        if let Some(name) = spec.icon {
            command = command.with_icon(icon(Some(&name)));
        }
        if let Some(hotkey) = spec.hotkey {
            command = command.with_keybinding(hotkey);
        }
        if spec.needs_note {
            command =
                command.available_when(|cx| Notia::workspace(cx).read(cx).active_note().is_some());
        }
        self.add(Contribution::Command(command));
        Ok(Value::Null)
    }

    fn add_ribbon_icon(self: &Rc<Self>, spec: RibbonSpec) -> Result<Value, String> {
        let js = Rc::downgrade(self);
        let item = RibbonItem::new(spec.id, icon(Some(&spec.icon)), spec.title, move |_, cx| {
            if let Some(js) = js.upgrade() {
                js.call_back(spec.callback, cx);
            }
        });
        self.add(Contribution::RibbonItem(item.with_order(ORDER)));
        Ok(Value::Null)
    }

    fn add_status_item(&self, cx: &mut App) -> usize {
        let item = cx.new(|_| StatusText::default());
        let id = {
            let mut state = self.state.borrow_mut();
            state.status_items.push(item.clone());
            state.status_items.len() - 1
        };
        let item = StatusItem::new(format!("status-{id}"), move |_, _| item.clone());
        self.add(Contribution::StatusItem(item.with_order(ORDER)));
        id
    }

    fn add_panel(self: &Rc<Self>, spec: PanelSpec, cx: &mut App) -> Result<Value, String> {
        check_id(&spec.id, "panel")?;
        if self.state.borrow().panels.contains_key(&spec.id) {
            return Err(format!("there already is a panel “{}”", spec.id));
        }
        let dirty = Rc::new(Cell::new(true));
        let bar = spec.side == "editor";
        let view = cx.new(|_| {
            JsPanel::new(Rc::downgrade(self), spec.id.clone(), dirty.clone(), bar)
        });
        self.state.borrow_mut().panels.insert(
            spec.id.clone(),
            PanelSlot {
                entity: view.entity_id(),
                dirty,
            },
        );
        let placement = match spec.side.as_str() {
            "left" => PanelPlacement::Left,
            "editor" => PanelPlacement::Editor,
            _ => PanelPlacement::Right,
        };
        let panel = Panel::new(
            spec.id,
            spec.title,
            icon(spec.icon.as_deref()),
            placement,
            move |_, _| view.clone(),
        );
        self.add(Contribution::Panel(panel.with_order(ORDER)));
        Ok(Value::Null)
    }

    fn add(&self, contribution: Contribution) {
        self.state.borrow_mut().pending.push(contribution);
    }

    fn load_data(&self) -> Result<Value, String> {
        match fs::read_to_string(self.plugin.dir.join("data.json")) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|error| format!("data.json is not valid JSON: {error}")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Value::Null),
            Err(error) => Err(format!("Could not read data.json: {error}")),
        }
    }

    fn save_data(&self, data: &Value) -> Result<Value, String> {
        let json = serde_json::to_string_pretty(data).map_err(|error| error.to_string())?;
        let temp = self.plugin.dir.join(".data.json.notia-tmp");
        fs::write(&temp, json)
            .and_then(|()| fs::rename(&temp, self.plugin.dir.join("data.json")))
            .map_err(|error| format!("Could not save data.json: {error}"))?;
        Ok(Value::Null)
    }

    fn set_timer(self: &Rc<Self>, id: u64, delay: f64, repeat: bool, cx: &mut App) {
        // Like browsers: at most about 24 days, and intervals of at least a
        // few milliseconds.
        let mut delay = Duration::from_secs_f64(delay.clamp(0., 2_147_483_647.) / 1000.);
        if repeat {
            delay = delay.max(Duration::from_millis(10));
        }
        let js = Rc::downgrade(self);
        let task = cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(delay).await;
                let Some(js) = js.upgrade() else {
                    break;
                };
                cx.update(|cx| js.fire_timer(id, cx));
                if !repeat {
                    break;
                }
            }
        });
        if repeat {
            self.state.borrow_mut().intervals.insert(id, task);
        } else {
            // `clearTimeout` forgets the callback, so a cleared timeout that
            // fires does nothing.
            task.detach();
        }
    }

    fn workspace_call(&self, method: &str, args: &Args, cx: &mut App) -> Result<Value, String> {
        match method {
            "workspace.activeNote" => Ok(active_note_json(
                Notia::workspace(cx).read(cx).active_note(),
            )),
            "workspace.openNote" => {
                let path = note_path(&args.get::<String>(0)?)?;
                let spec = args.get::<Option<OpenSpec>>(1)?.unwrap_or_default();
                if !Notia::vault(cx).read(cx).vault().exists(&path) {
                    return Err(format!("there is no note “{path}”"));
                }
                let mut options = OpenOptions::new();
                if let Some(line) = spec.line {
                    options = options.at_line(line.max(0.) as usize);
                }
                if spec.new_tab {
                    options = options.in_new_tab();
                }
                with_window_later(cx, move |window, cx| {
                    workspace::open_note(path, options, window, cx)
                });
                Ok(Value::Null)
            }
            "workspace.openLink" => {
                let target: String = args.get(0)?;
                let spec = args.get::<Option<OpenSpec>>(1)?.unwrap_or_default();
                let from = Notia::workspace(cx).read(cx).active_path().cloned();
                with_window_later(cx, move |window, cx| {
                    workspace::open_link(&target, from, spec.new_tab, window, cx)
                });
                Ok(Value::Null)
            }
            "workspace.newNote" => {
                let folder = args.get::<Option<String>>(0)?.map(VaultPath::new);
                with_window_later(cx, move |window, cx| {
                    workspace::new_note(folder, window, cx)
                });
                Ok(Value::Null)
            }
            "workspace.insertText" => {
                let text: String = args.get(0)?;
                with_window_later(cx, move |window, cx| {
                    workspace::insert_text(&text, window, cx)
                });
                Ok(Value::Null)
            }
            "workspace.selection" => {
                let selection = workspace::selection(cx);
                Ok(json!({
                    "text": selection.text,
                    "before": selection.before,
                    "after": selection.after,
                }))
            }
            "workspace.replaceSelection" => {
                let text: String = args.get(0)?;
                let expand_before = args.get::<Option<f64>>(1)?.unwrap_or(0.).max(0.) as usize;
                let expand_after = args.get::<Option<f64>>(2)?.unwrap_or(0.).max(0.) as usize;
                with_window_later(cx, move |window, cx| {
                    workspace::replace_selection(&text, expand_before, expand_after, window, cx)
                });
                Ok(Value::Null)
            }
            "workspace.undo" => {
                with_window_later(cx, |window, cx| workspace::undo(window, cx));
                Ok(Value::Null)
            }
            "workspace.redo" => {
                with_window_later(cx, |window, cx| workspace::redo(window, cx));
                Ok(Value::Null)
            }
            "workspace.revealLine" => {
                let line = args.get::<f64>(0)?.max(0.) as usize;
                with_window_later(cx, move |window, cx| {
                    workspace::reveal_line(line, window, cx)
                });
                Ok(Value::Null)
            }
            "workspace.search" => {
                let query: String = args.get(0)?;
                Notia::workspace(cx).update(cx, |model, cx| model.request_search(query.into(), cx));
                Ok(Value::Null)
            }
            "workspace.runCommand" => {
                let id: String = args.get(0)?;
                let id = if id.contains(':') {
                    id
                } else {
                    format!("{}:{id}", self.plugin.id)
                };
                with_window_later(cx, move |window, cx| {
                    workspace::run_command(&id, window, cx)
                });
                Ok(Value::Null)
            }
            _ => Err(format!("Notia has no function “{method}”")),
        }
    }
}

fn vault_call(method: &str, args: &Args, cx: &mut App) -> Result<Value, String> {
    let vault = Notia::vault(cx);
    match method {
        "vault.name" => Ok(vault.read(cx).name().into()),
        "vault.notes" => {
            let model = vault.read(cx);
            let mut notes: Vec<_> = model.index().notes().collect();
            notes.sort_by(|a, b| a.path.cmp(&b.path));
            Ok(notes
                .into_iter()
                .map(|note| {
                    json!({
                        "path": note.path.as_str(),
                        "name": note.title(),
                        "tags": note.meta.tags,
                        "words": note.meta.word_count,
                        "modified": note.modified
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .map(|since| since.as_millis() as f64),
                    })
                })
                .collect())
        }
        "vault.read" => {
            let path = note_path(&args.get::<String>(0)?)?;
            let text = vault
                .read(cx)
                .read_note(&path)
                .map_err(|error| format!("Could not read “{path}”: {error}"))?;
            Ok(text.into())
        }
        "vault.exists" => {
            let path = note_path(&args.get::<String>(0)?)?;
            Ok(vault.read(cx).vault().exists(&path).into())
        }
        "vault.create" => {
            let (path, text): (String, String) = (args.get(0)?, args.get(1)?);
            let path = note_path(&path)?;
            if vault.read(cx).vault().exists(&path) {
                return Err(format!("“{path}” already exists"));
            }
            save(&vault, &path, &text, cx)?;
            Ok(path.as_str().into())
        }
        "vault.modify" => {
            let (path, text): (String, String) = (args.get(0)?, args.get(1)?);
            let path = note_path(&path)?;
            if !vault.read(cx).vault().exists(&path) {
                return Err(format!("there is no note “{path}”"));
            }
            save(&vault, &path, &text, cx)?;
            Ok(Value::Null)
        }
        "vault.append" => {
            let (path, addition): (String, String) = (args.get(0)?, args.get(1)?);
            let path = note_path(&path)?;
            let mut text = if vault.read(cx).vault().exists(&path) {
                vault
                    .read(cx)
                    .read_note(&path)
                    .map_err(|error| format!("Could not read “{path}”: {error}"))?
            } else {
                String::new()
            };
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&addition);
            save(&vault, &path, &text, cx)?;
            Ok(Value::Null)
        }
        _ => Err(format!("Notia has no function “{method}”")),
    }
}

fn metadata_call(method: &str, args: &Args, cx: &mut App) -> Result<Value, String> {
    let vault = Notia::vault(cx);
    let index = vault.read(cx).index();
    match method {
        "metadata.headings" => {
            let path = note_path(&args.get::<String>(0)?)?;
            let headings = index.get(&path).map(|note| note.meta.headings.as_slice());
            Ok(headings
                .unwrap_or_default()
                .iter()
                .map(|heading| {
                    json!({ "level": heading.level, "text": heading.text, "line": heading.line })
                })
                .collect())
        }
        "metadata.links" => {
            let path = note_path(&args.get::<String>(0)?)?;
            Ok(index
                .outgoing_links(&path)
                .into_iter()
                .map(|(link, resolved)| {
                    json!({
                        "target": link.target,
                        "subpath": link.subpath,
                        "display": link.display,
                        "line": link.line,
                        "embed": link.embed,
                        "resolved": resolved.as_ref().map(VaultPath::as_str),
                    })
                })
                .collect())
        }
        "metadata.backlinks" => {
            let path = note_path(&args.get::<String>(0)?)?;
            Ok(index
                .backlinks(&path)
                .into_iter()
                .map(|backlink| {
                    json!({
                        "source": backlink.source.as_str(),
                        "line": backlink.line,
                        "context": backlink.context,
                    })
                })
                .collect())
        }
        "metadata.tags" => Ok(index
            .tags()
            .into_iter()
            .map(|tag| json!({ "tag": tag.tag, "count": tag.count }))
            .collect()),
        "metadata.notesWithTag" => {
            let tag: String = args.get(0)?;
            Ok(index
                .notes_with_tag(tag.trim_start_matches('#'))
                .iter()
                .map(|path| Value::from(path.as_str()))
                .collect())
        }
        "metadata.resolveLink" => {
            let target: String = args.get(0)?;
            let from = args.get::<Option<String>>(1)?.map(VaultPath::new);
            Ok(index
                .resolve(&target, from.as_ref())
                .map_or(Value::Null, |path| path.as_str().into()))
        }
        _ => Err(format!("Notia has no function “{method}”")),
    }
}

/// The note a plugin means by `path`: `.md` may be left out, and hidden
/// files, like the vault's configuration, are out of reach.
fn note_path(path: &str) -> Result<VaultPath, String> {
    let path = VaultPath::new(path);
    if path.is_root() {
        return Err("the path is empty".into());
    }
    if Vault::is_hidden(&path) {
        return Err(format!("plugins cannot access hidden files like “{path}”"));
    }
    Ok(match path.extension() {
        Some(_) => path,
        None => VaultPath::new(format!("{}.md", path.as_str())),
    })
}

fn save(
    vault: &Entity<VaultModel>,
    path: &VaultPath,
    text: &str,
    cx: &mut App,
) -> Result<(), String> {
    vault
        .update(cx, |vault, cx| vault.save_note(path, text, cx))
        .map_err(|error| format!("Could not save “{path}”: {error}"))
}

fn check_id(id: &str, what: &str) -> Result<(), String> {
    if id.is_empty() || id.contains(':') {
        return Err(format!("a {what} needs an id without “:”"));
    }
    Ok(())
}
