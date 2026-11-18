//! What a plugin's JavaScript can ask of Notia. The prelude calls these by
//! name, with arguments and results as JSON.

use std::{
    cell::Cell,
    collections::BTreeMap,
    fs, io,
    path::{Component, Path, PathBuf},
    rc::Rc,
    time::{Duration, UNIX_EPOCH},
};

use gpui_kit::{App, AppContext as _, Entity, component::notification::Notification};
use notia_plugin::{
    Command, EditorChange, EditorCommand, EditorId, EditorSelection, EditorSnapshot,
    EditorTransaction, Notia, OpenOptions, Panel, PanelPlacement, RibbonItem, SettingsTab,
    StatusItem, VaultModel, ViewMode,
    core::{Vault, VaultPath},
    workspace,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    icon, offsets,
    runtime::{Contribution, JsRuntime, PanelSlot, notify, with_window_later},
    settings::JsSettings,
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

#[derive(Deserialize)]
struct SettingsTabSpec {
    /// The view the prelude renders, `settings:<n>`.
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    icon: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct OpenSpec {
    line: Option<f64>,
    new_tab: bool,
}

/// An edit from the prelude's `Editor`, with offsets in UTF-16 code units.
/// The changes refer to the text before the edit, and the selection to the
/// text after it.
#[derive(Deserialize)]
struct EditSpec {
    #[serde(default)]
    changes: Vec<ChangeSpec>,
    #[serde(default)]
    selection: Option<SelectionSpec>,
}

#[derive(Deserialize)]
struct ChangeSpec {
    from: f64,
    to: f64,
    text: String,
}

#[derive(Deserialize)]
struct SelectionSpec {
    anchor: f64,
    head: f64,
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
            "addSettingTab" => self.add_settings_tab(args.get(0)?),
            "refreshSettings" => {
                let id: String = args.get(0)?;
                let state = self.state.borrow();
                // Refreshing from `render` would render forever.
                if state.rendering.as_ref() != Some(&id) {
                    let tab = state
                        .settings
                        .get(&id)
                        .cloned()
                        .ok_or_else(|| format!("there is no settings tab “{id}”"))?;
                    drop(state);
                    tab.invalidate();
                    cx.refresh_windows();
                }
                Ok(Value::Null)
            }
            "loadData" => self.load_data(),
            "saveData" => self.save_data(&args.get(0)?),
            "readFile" => self.read_file(args.get(0)?),
            "writeFile" => self.write_file(args.get(0)?, args.get(1)?),
            "openFile" => self.open_file(args.get(0)?, cx),
            "setThemeColors" => self.set_theme_colors(args.get(0)?, cx),
            "setFileIcons" => {
                self.set_file_icons(args.0.first().cloned().unwrap_or(Value::Null), cx)
            }
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
            "requestUrl" => {
                let (id, request): (u64, crate::http::Request) = (args.get(0)?, args.get(1)?);
                self.start_request(id, request, cx)
            }
            _ if method.starts_with("vault.") => vault_call(method, &args, cx),
            _ if method.starts_with("workspace.") => self.workspace_call(method, &args, cx),
            _ if method.starts_with("editor.") => editor_call(method, &args, cx),
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
        let view =
            cx.new(|_| JsPanel::new(Rc::downgrade(self), spec.id.clone(), dirty.clone(), bar));
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

    fn add_settings_tab(self: &Rc<Self>, spec: SettingsTabSpec) -> Result<Value, String> {
        let settings = Rc::new(JsSettings::new(Rc::downgrade(self), spec.id.clone()));
        self.state
            .borrow_mut()
            .settings
            .insert(spec.id.clone(), settings.clone());
        let mut tab = SettingsTab::new(spec.id.replace(':', "-"), move |cx| settings.sections(cx))
            .with_icon(icon(spec.icon.as_deref()));
        if let Some(title) = spec.title {
            tab = tab.with_title(title);
        }
        self.add(Contribution::SettingsTab(tab));
        Ok(Value::Null)
    }

    pub(crate) fn add(&self, contribution: Contribution) {
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

    fn read_file(&self, name: String) -> Result<Value, String> {
        let path = plugin_file(&self.plugin.dir, &name)?;
        match fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > MAX_PLUGIN_FILE {
                    return Err(format!("“{name}” is too large"));
                }
                String::from_utf8(bytes)
                    .map(Value::String)
                    .map_err(|_| format!("“{name}” is not text"))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Value::Null),
            Err(error) => Err(format!("Could not read “{name}”: {error}")),
        }
    }

    fn write_file(&self, name: String, text: String) -> Result<Value, String> {
        if text.len() > MAX_PLUGIN_FILE {
            return Err(format!("“{name}” is too large"));
        }
        let path = plugin_file(&self.plugin.dir, &name)?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|error| format!("Could not save “{name}”: {error}"))?;
        }
        let temp = self.plugin.dir.join(".plugin-file.notia-tmp");
        fs::write(&temp, text)
            .and_then(|()| fs::rename(&temp, &path))
            .map_err(|error| format!("Could not save “{name}”: {error}"))?;
        Ok(Value::Null)
    }

    fn open_file(&self, name: String, cx: &mut App) -> Result<Value, String> {
        let path = plugin_file(&self.plugin.dir, &name)?;
        cx.open_with_system(&path);
        Ok(Value::Null)
    }

    fn set_theme_colors(&self, colors: Value, cx: &mut App) -> Result<Value, String> {
        let Some(object) = colors.as_object() else {
            return Err("theme colors must be an object".to_string());
        };
        let mut overrides = BTreeMap::new();
        let mut warnings = Vec::new();
        for (key, value) in object {
            match value {
                Value::String(color) => {
                    overrides.insert(key.clone(), color.clone());
                }
                Value::Null => {}
                _ => warnings.push(format!("“{key}” needs a color")),
            }
        }
        warnings.extend(notia_plugin::theme::ThemeCatalog::set_color_overrides(
            self.plugin.id.as_ref(),
            overrides,
            cx,
        ));
        Ok(json!({ "warnings": warnings }))
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

    /// Starts an HTTP request and resolves the plugin's promise when it
    /// finishes. The call returns at once, so a slow server does not freeze
    /// the window or use up the plugin's time limit.
    fn start_request(
        self: &Rc<Self>,
        id: u64,
        request: crate::http::Request,
        cx: &mut App,
    ) -> Result<Value, String> {
        let prepared = crate::http::prepare(request)?;
        {
            let mut state = self.state.borrow_mut();
            if state.in_flight >= crate::http::MAX_IN_FLIGHT {
                return Err("too many requests at once".into());
            }
            state.in_flight += 1;
        }
        let js = Rc::downgrade(self);
        cx.spawn(async move |cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::http::perform(prepared) })
                .await;
            if let Some(js) = js.upgrade() {
                cx.update(|cx| js.finish_request(id, result, cx));
            }
        })
        .detach();
        Ok(Value::Null)
    }

    fn finish_request(&self, id: u64, result: Result<Value, String>, cx: &mut App) {
        {
            let mut state = self.state.borrow_mut();
            state.in_flight = state.in_flight.saturating_sub(1);
        }
        if self.unloaded.get() {
            return;
        }
        let payload = match result {
            Ok(response) => json!({ "ok": true, "response": response }),
            Err(error) => json!({ "ok": false, "error": error }),
        };
        if let Err(error) = self.dispatch("request", &id.to_string(), &payload.to_string(), cx) {
            self.report(&error, cx);
        }
    }

    fn workspace_call(&self, method: &str, args: &Args, cx: &mut App) -> Result<Value, String> {
        match method {
            "workspace.activeNote" => Ok(note_json(workspace::active_editor(cx).as_ref())),
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

/// The calls behind the prelude's `Editor`. Offsets are in UTF-16 code
/// units, like JavaScript string indices.
fn editor_call(method: &str, args: &Args, cx: &mut App) -> Result<Value, String> {
    match method {
        "editor.get" => {
            let editor = match args.get::<Option<f64>>(0)? {
                Some(id) => workspace::editor(editor_id(id), cx),
                None => workspace::active_editor(cx),
            };
            Ok(editor.as_ref().map_or(Value::Null, editor_json))
        }
        "editor.edit" => {
            let id = editor_id(args.get(0)?);
            let editor = workspace::editor(id, cx).ok_or("the note is no longer open")?;
            let transaction = transaction(&editor, args.get(1)?)?;
            workspace::edit(id, transaction, cx).map_err(|error| format!("{error:#}"))?;
            Ok(Value::Null)
        }
        "editor.exec" => {
            let id = editor_id(args.get(0)?);
            let command = match args.get::<String>(1)?.as_str() {
                "undo" => EditorCommand::Undo,
                "redo" => EditorCommand::Redo,
                "focus" => EditorCommand::Focus,
                other => return Err(format!("editors have no command “{other}”")),
            };
            workspace::run_editor_command(id, command, cx);
            Ok(Value::Null)
        }
        _ => Err(format!("Notia has no function “{method}”")),
    }
}

fn editor_id(id: f64) -> EditorId {
    EditorId::new(id.max(0.) as u64)
}

/// An open note's editor as the prelude reads it.
fn editor_json(editor: &EditorSnapshot) -> Value {
    let text = editor.text.as_ref();
    json!({
        "id": editor.id.as_u64(),
        "path": editor.path.as_str(),
        "text": text,
        "anchor": offsets::to_utf16(text, editor.selection.anchor),
        "head": offsets::to_utf16(text, editor.selection.head),
    })
}

/// The edit `spec` describes, in bytes of the editor's text.
fn transaction(editor: &EditorSnapshot, spec: EditSpec) -> Result<EditorTransaction, String> {
    let text = editor.text.as_ref();
    let byte = |text: &str, units: f64| offsets::from_utf16(text, units.max(0.) as usize);
    let changes = spec
        .changes
        .into_iter()
        .map(|change| {
            let (from, to) = (byte(text, change.from), byte(text, change.to));
            EditorChange {
                range: from.min(to)..from.max(to),
                text: change.text,
            }
        })
        .collect();
    let mut transaction = EditorTransaction {
        changes,
        selection: None,
    };
    if let Some(selection) = spec.selection {
        let (after, _) = transaction
            .apply(text, editor.selection)
            .map_err(|error| format!("{error:#}"))?;
        transaction.selection = Some(EditorSelection::new(
            byte(&after, selection.anchor),
            byte(&after, selection.head),
        ));
    }
    Ok(transaction)
}

/// The note in an editor, as `getActiveNote` and workspace events give it.
pub(crate) fn note_json(editor: Option<&EditorSnapshot>) -> Value {
    editor.map_or(Value::Null, |editor| {
        let text = editor.text.as_ref();
        let before_cursor = text.get(..editor.selection.head).unwrap_or(text);
        json!({
            "path": editor.path.as_str(),
            "name": editor.path.file_stem(),
            "text": text,
            "line": before_cursor.matches('\n').count(),
            "mode": match editor.mode {
                ViewMode::Editing => "editing",
                ViewMode::Half => "half",
                ViewMode::Reading => "preview",
            },
        })
    })
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

/// A stylesheet or other text file a plugin keeps beside its code. A megabyte
/// is enough for a theme and small enough that a plugin cannot fill the disk.
const MAX_PLUGIN_FILE: usize = 1024 * 1024;

/// A file inside the plugin folder. `..` and absolute paths stay out, so a
/// plugin cannot read or write the rest of the vault.
fn plugin_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains('\0') {
        return Err("the file name is empty".to_string());
    }
    let rel = Path::new(name);
    let inside = rel
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    if !inside || rel.is_absolute() {
        return Err("the file is outside the plugin folder".to_string());
    }
    Ok(dir.join(rel))
}

#[cfg(test)]
mod tests {
    use super::plugin_file;

    #[test]
    fn plugin_files_stay_in_the_plugin_folder() {
        let dir = std::path::Path::new("/vault/.notia/plugins/theme");
        assert_eq!(
            plugin_file(dir, "theme.css").unwrap(),
            dir.join("theme.css")
        );
        assert_eq!(
            plugin_file(dir, "packs/classic.css").unwrap(),
            dir.join("packs/classic.css")
        );
        assert!(plugin_file(dir, "../secret").is_err());
        assert!(plugin_file(dir, "/etc/passwd").is_err());
        assert!(plugin_file(dir, "").is_err());
    }
}

fn check_id(id: &str, what: &str) -> Result<(), String> {
    if id.is_empty() || id.contains(':') {
        return Err(format!("a {what} needs an id without “:”"));
    }
    Ok(())
}
