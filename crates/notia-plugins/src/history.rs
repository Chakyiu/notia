//! Saved versions of a note. Each save that changes the text is kept, and
//! an older version can be restored.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Local, Utc};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
// A glob import would pull in GPUI's `test` macro and shadow `#[test]` below.
use gpui_kit::{
    App, AppContext, Context, Entity, FontWeight, Global, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Window, div, px,
};
use notia_plugin::{
    Command, EditorChange, EditorSelection, EditorTransaction, Notia, Panel, PanelPlacement,
    Plugin, PluginContext, PluginManifest, Setting, SettingsSection, SettingsTab, VaultEvent,
    WorkspaceEvent, core::VaultPath, workspace,
};
use serde::{Deserialize, Serialize};

use crate::ui;

const PLUGIN_ID: &str = "history";
const PANEL_ID: &str = "history:history";
/// Versions kept for one note when the vault has no saved preference.
const DEFAULT_KEEP: u32 = 80;
const MIN_KEEP: u32 = 1;
const MAX_KEEP: u32 = 500;

pub struct HistoryPlugin;

impl Plugin for HistoryPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(PLUGIN_ID, "History").with_description(
            "Keeps each saved version of a note, and restores any earlier version.",
        )
    }

    fn activate(&self, ctx: &mut PluginContext, cx: &mut App) {
        let model = cx.new(HistoryModel::new);
        cx.set_global(HistoryHost { model });
        ctx.on_deactivate(|cx| {
            cx.remove_global::<HistoryHost>();
        });

        ctx.add_panel(
            Panel::new(
                "history",
                "History",
                Icon::new(Lucide::Clock),
                PanelPlacement::Right,
                |_, cx| cx.new(HistoryPanel::new),
            )
            .with_order(40),
        );
        ctx.add_command(
            Command::new("show", "Show history", |window, cx| {
                workspace::show_panel(PANEL_ID, window, cx);
            })
            .with_icon(Icon::new(Lucide::Clock)),
        );
        ctx.add_command(
            Command::new(
                "compare-previous",
                "Compare previous version",
                |window, cx| compare_previous(window, cx),
            )
            .with_icon(Icon::new(Lucide::GitCompare))
            .available_when(has_previous),
        );
        ctx.add_command(
            Command::new(
                "restore-previous",
                "Restore previous version",
                |window, cx| {
                    restore_previous(window, cx);
                },
            )
            .with_icon(Icon::new(Lucide::Undo2))
            .available_when(has_previous),
        );
        ctx.add_command(
            Command::new("clear", "Clear history for this note", |_, cx| {
                clear_active(cx)
            })
            .available_when(has_previous),
        );
        ctx.add_settings_tab(
            SettingsTab::new("options", settings_page).with_icon(Icon::new(Lucide::Clock)),
        );
    }
}

fn settings_page(_: &mut App) -> Vec<SettingsSection> {
    vec![SettingsSection::new().setting(
        Setting::number(
            "Versions to keep",
            (MIN_KEEP as f64)..=(MAX_KEEP as f64),
            1.,
            |cx| HistorySettings::load(cx).keep as f64,
            |value, cx| HistorySettings::set_keep(value as u32, cx),
        )
        .with_description(
            "How many saved versions each note keeps. Older ones are dropped. The newest stays.",
        ),
    )]
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct HistorySettings {
    keep: u32,
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self { keep: DEFAULT_KEEP }
    }
}

impl HistorySettings {
    fn load(cx: &App) -> Self {
        let mut settings: Self = Notia::load_plugin_data(PLUGIN_ID, cx);
        settings.keep = settings.keep.clamp(MIN_KEEP, MAX_KEEP);
        settings
    }

    fn limit(cx: &App) -> usize {
        Self::load(cx).keep as usize
    }

    fn set_keep(keep: u32, cx: &mut App) {
        let settings = Self {
            keep: keep.clamp(MIN_KEEP, MAX_KEEP),
        };
        Notia::save_plugin_data(PLUGIN_ID, &settings, cx);
        if let Some(model) = HistoryHost::model(cx) {
            let limit = settings.keep as usize;
            model.update(cx, |model, cx| model.enforce_limit(limit, cx));
        }
    }
}

struct HistoryHost {
    model: Entity<HistoryModel>,
}

impl Global for HistoryHost {}

impl HistoryHost {
    fn model(cx: &App) -> Option<Entity<HistoryModel>> {
        cx.try_global::<Self>().map(|host| host.model.clone())
    }
}

fn compare_previous(window: &mut Window, cx: &mut App) {
    let Some(model) = HistoryHost::model(cx) else {
        return;
    };
    let Some(path) = model.read(cx).active.clone() else {
        return;
    };
    let Some(revision) = model.read(cx).previous_revision(&path) else {
        return;
    };
    let (at, text) = (revision.at, revision.text.clone());
    model.update(cx, |model, cx| model.open_compare(path, at, text, cx));
    open_compare_popup(window, cx);
}

fn open_compare_popup(window: &mut Window, cx: &mut App) {
    window.defer(cx, |window, cx| {
        if window.has_active_dialog(cx) {
            return;
        }
        let view = cx.new(ComparePanel::new);
        window.open_dialog(cx, move |dialog, window, cx| {
            let height = (window.viewport_size().height - px(140.)).min(px(560.));
            dialog
                .title("Compare")
                .w(px(720.))
                .child(div().h(height).overflow_hidden().child(view.clone()))
        });
    });
}

fn restore_previous(window: &mut Window, cx: &mut App) {
    let Some(model) = HistoryHost::model(cx) else {
        return;
    };
    let Some((path, text)) = model.read(cx).previous_text() else {
        return;
    };
    restore(&path, text, window, cx);
}

fn clear_active(cx: &mut App) {
    if let Some(model) = HistoryHost::model(cx) {
        model.update(cx, |model, cx| model.clear_active(cx));
    }
}

fn has_previous(cx: &App) -> bool {
    HistoryHost::model(cx).is_some_and(|model| model.read(cx).previous_text().is_some())
}

/// Puts `text` back into the open note. The editor saves it, and that save
/// is recorded as a new version, so the one it replaced stays in the list.
fn restore(path: &VaultPath, text: String, window: &mut Window, cx: &mut App) {
    let Some(editor) = workspace::active_editor(cx).filter(|editor| &editor.path == path) else {
        window.push_notification("Open the note to restore a version.", cx);
        return;
    };
    if editor.text.as_ref() == text {
        return;
    }
    let id = editor.id;
    let replace = EditorTransaction {
        changes: vec![EditorChange {
            range: 0..editor.text.len(),
            text: text.clone(),
        }],
        selection: Some(EditorSelection::cursor(0)),
    };
    if let Err(error) = workspace::edit(id, replace, cx) {
        window.push_notification(format!("Could not restore this version: {error}"), cx);
        return;
    }
    if let Some(model) = HistoryHost::model(cx) {
        model.update(cx, |model, cx| model.record(path, &text, cx));
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Revision {
    at: i64,
    text: String,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct NoteLog {
    revisions: Vec<Revision>,
}

impl NoteLog {
    /// Appends `text` when it differs from the latest version.
    fn record(&mut self, text: &str, at: i64, limit: usize) -> bool {
        if self
            .revisions
            .last()
            .is_some_and(|revision| revision.text == text)
        {
            return false;
        }
        self.revisions.push(Revision {
            at,
            text: text.to_string(),
        });
        trim(self, limit);
        true
    }

    fn previous(&self) -> Option<&Revision> {
        let len = self.revisions.len();
        (len >= 2).then(|| &self.revisions[len - 2])
    }
}

/// Drops the oldest versions past `limit`. Reports whether any were dropped.
fn trim(log: &mut NoteLog, limit: usize) -> bool {
    let extra = log.revisions.len().saturating_sub(limit.max(1));
    if extra == 0 {
        return false;
    }
    log.revisions.drain(0..extra);
    true
}

/// How a version differs from the one before it.
fn change_label(before: &str, after: &str) -> String {
    let delta = after.chars().count() as i64 - before.chars().count() as i64;
    if delta > 0 {
        format!("+{delta}")
    } else if delta < 0 {
        delta.to_string()
    } else {
        "edited".to_string()
    }
}

fn snippet(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Empty");
    let line = line.trim();
    let count = line.chars().count();
    if count > 72 {
        format!("{}…", line.chars().take(72).collect::<String>())
    } else if line.is_empty() {
        "Empty".to_string()
    } else {
        line.to_string()
    }
}

fn log_file(dir: &Path, note: &VaultPath) -> PathBuf {
    dir.join(format!("{note}.json"))
}

#[derive(Clone)]
struct CompareSelection {
    path: VaultPath,
    at: i64,
    text: String,
}

struct HistoryModel {
    dir: PathBuf,
    logs: HashMap<VaultPath, NoteLog>,
    active: Option<VaultPath>,
    compare: Option<CompareSelection>,
    _subscriptions: Vec<Subscription>,
}

impl HistoryModel {
    fn new(cx: &mut Context<Self>) -> Self {
        let dir = Notia::vault(cx)
            .read(cx)
            .vault()
            .config_dir()
            .join("history");
        let subscriptions = vec![
            cx.subscribe(&Notia::workspace(cx), |this, workspace, event, cx| {
                if matches!(event, WorkspaceEvent::ActiveNoteChanged) {
                    let active = workspace.read(cx).active_path().cloned();
                    this.follow(active, cx);
                }
            }),
            cx.subscribe(&Notia::vault(cx), |this, _, event, cx| {
                this.on_vault(event, cx);
            }),
        ];
        let mut model = Self {
            dir,
            logs: HashMap::new(),
            active: None,
            compare: None,
            _subscriptions: subscriptions,
        };
        if let Some(path) = Notia::workspace(cx).read(cx).active_path().cloned() {
            model.follow(Some(path), cx);
        }
        let limit = HistorySettings::limit(cx);
        model.enforce_limit(limit, cx);
        model
    }

    fn follow(&mut self, active: Option<VaultPath>, cx: &mut Context<Self>) {
        if self.compare.as_ref().map(|item| &item.path) != active.as_ref() {
            self.compare = None;
        }
        self.active = active.clone();
        if let Some(path) = active
            && let Some(text) = note_text(&path, cx)
        {
            self.record(&path, &text, cx);
        }
        cx.notify();
    }

    fn on_vault(&mut self, event: &VaultEvent, cx: &mut Context<Self>) {
        match event {
            VaultEvent::NotesChanged(paths) => {
                let mut changed = false;
                for path in paths {
                    if !self.logs.contains_key(path) && self.active.as_ref() != Some(path) {
                        continue;
                    }
                    let Some(text) = indexed_text(path, cx) else {
                        continue;
                    };
                    changed |= self.record(path, &text, cx);
                }
                if changed {
                    cx.notify();
                }
            }
            VaultEvent::Renamed { from, to } => {
                self.relocate(from, to);
                cx.notify();
            }
            VaultEvent::Trashed(path) => {
                self.forget(path);
                cx.notify();
            }
            VaultEvent::TreeChanged => {}
        }
    }

    fn record(&mut self, path: &VaultPath, text: &str, cx: &mut Context<Self>) -> bool {
        if !self.logs.contains_key(path) {
            let loaded = self.read(path);
            self.logs.insert(path.clone(), loaded);
        }
        let keep = {
            let limit = HistorySettings::limit(cx);
            let log = self.logs.get_mut(path).unwrap();
            if !log.record(text, unix_now(), limit) {
                return false;
            }
            log.revisions.len() >= 2
        };
        // A single baseline is the note as it is. Keep it in memory, and
        // write the file once a later save has something to restore.
        if keep {
            self.write(path);
        }
        cx.notify();
        true
    }

    fn previous_revision(&self, path: &VaultPath) -> Option<&Revision> {
        self.logs.get(path)?.previous()
    }

    fn open_compare(&mut self, path: VaultPath, at: i64, text: String, cx: &mut Context<Self>) {
        self.compare = Some(CompareSelection { path, at, text });
        cx.notify();
    }

    fn previous_text(&self) -> Option<(VaultPath, String)> {
        let path = self.active.clone()?;
        let revision = self.logs.get(&path)?.previous()?;
        Some((path, revision.text.clone()))
    }

    fn revisions(&self, path: &VaultPath) -> Vec<Revision> {
        self.logs
            .get(path)
            .map(|log| log.revisions.clone())
            .unwrap_or_default()
    }

    fn clear_active(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.active.clone() else {
            return;
        };
        self.forget(&path);
        if let Some(text) = note_text(&path, cx) {
            let limit = HistorySettings::limit(cx);
            self.logs
                .entry(path)
                .or_default()
                .record(&text, unix_now(), limit);
        }
        cx.notify();
    }

    fn read(&self, path: &VaultPath) -> NoteLog {
        let file = log_file(&self.dir, path);
        match fs::read_to_string(&file) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|error| {
                log::warn!("ignoring the invalid history {}: {error}", file.display());
                NoteLog::default()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => NoteLog::default(),
            Err(error) => {
                log::warn!("could not read {}: {error}", file.display());
                NoteLog::default()
            }
        }
    }

    /// Drops versions past `limit` from open notes and from every saved log.
    fn enforce_limit(&mut self, limit: usize, cx: &mut Context<Self>) {
        let limit = limit.max(1);
        for log in self.logs.values_mut() {
            trim(log, limit);
        }
        let paths: Vec<VaultPath> = self.logs.keys().cloned().collect();
        for path in &paths {
            let len = self
                .logs
                .get(path)
                .map(|log| log.revisions.len())
                .unwrap_or(0);
            if len >= 2 {
                self.write(path);
            } else {
                remove_log_file(&self.dir, path);
            }
        }
        trim_saved(&self.dir, limit);
        cx.notify();
    }

    fn write(&self, path: &VaultPath) {
        let Some(log) = self.logs.get(path) else {
            return;
        };
        let file = log_file(&self.dir, path);
        let saved = (|| -> io::Result<()> {
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent)?;
            }
            let json = serde_json::to_string_pretty(log).map_err(io::Error::other)?;
            fs::write(&file, json)?;
            Ok(())
        })();
        if let Err(error) = saved {
            log::error!("could not save {}: {error}", file.display());
        }
    }

    fn relocate(&mut self, from: &VaultPath, to: &VaultPath) {
        let mut moved = Vec::new();
        for path in self.logs.keys() {
            if let Some(next) = path.rebase(from, to) {
                moved.push((path.clone(), next));
            }
        }
        for (path, next) in moved {
            if let Some(log) = self.logs.remove(&path) {
                self.logs.insert(next, log);
            }
        }
        move_entry(&log_file(&self.dir, from), &log_file(&self.dir, to));
        move_entry(&self.dir.join(from.as_str()), &self.dir.join(to.as_str()));
    }

    fn forget(&mut self, path: &VaultPath) {
        self.logs.retain(|note, _| !note.starts_with(path));
        let file = log_file(&self.dir, path);
        if file.is_file() {
            if let Err(error) = fs::remove_file(&file) {
                log::warn!("could not remove {}: {error}", file.display());
            }
        }
        let folder = self.dir.join(path.as_str());
        if folder.is_dir()
            && let Err(error) = fs::remove_dir_all(&folder)
        {
            log::warn!("could not remove {}: {error}", folder.display());
        }
    }
}

fn note_text(path: &VaultPath, cx: &App) -> Option<String> {
    if let Some(editor) = workspace::active_editor(cx)
        && &editor.path == path
    {
        return Some(editor.text.to_string());
    }
    indexed_text(path, cx)
}

fn indexed_text(path: &VaultPath, cx: &App) -> Option<String> {
    Notia::vault(cx)
        .read(cx)
        .index()
        .get(path)
        .map(|note| note.text.to_string())
        .or_else(|| Notia::vault(cx).read(cx).read_note(path).ok())
}

fn remove_log_file(dir: &Path, note: &VaultPath) {
    let file = log_file(dir, note);
    if file.is_file()
        && let Err(error) = fs::remove_file(&file)
    {
        log::warn!("could not remove {}: {error}", file.display());
    }
}

/// Trims every saved log under `dir` to `limit` versions.
fn trim_saved(dir: &Path, limit: usize) {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(json) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut log) = serde_json::from_str::<NoteLog>(&json) else {
                continue;
            };
            if !trim(&mut log, limit) {
                continue;
            }
            if log.revisions.len() < 2 {
                if let Err(error) = fs::remove_file(&path) {
                    log::warn!("could not remove {}: {error}", path.display());
                }
                continue;
            }
            let saved = serde_json::to_string_pretty(&log)
                .map_err(io::Error::other)
                .and_then(|json| fs::write(&path, json));
            if let Err(error) = saved {
                log::warn!("could not save {}: {error}", path.display());
            }
        }
    }
}

fn move_entry(from: &Path, to: &Path) {
    if !from.exists() {
        return;
    }
    if let Some(parent) = to.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        log::warn!("could not create {}: {error}", parent.display());
        return;
    }
    if let Err(error) = fs::rename(from, to) {
        log::warn!(
            "could not move {} to {}: {error}",
            from.display(),
            to.display()
        );
    }
}

fn unix_now() -> i64 {
    Utc::now().timestamp_millis()
}

/// `YYYY-MM-DD HH:mm:ss.xxx` in local time. Values saved before milliseconds
/// were recorded are whole seconds and show `.000`.
fn format_when(at: i64) -> String {
    let millis = if at.abs() < 100_000_000_000 {
        at.saturating_mul(1000)
    } else {
        at
    };
    let Some(time) = DateTime::<Utc>::from_timestamp_millis(millis) else {
        return String::new();
    };
    time.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DiffLine {
    Same(String),
    Removed(String),
    Added(String),
}

/// Line differences from an older version to the current text.
fn diff_lines(before: &str, after: &str) -> Vec<DiffLine> {
    let old = split_lines(before);
    let new = split_lines(after);
    if old.len().saturating_mul(new.len()) > 800 * 800 {
        let mut lines = Vec::new();
        lines.extend(
            old.into_iter()
                .map(|line| DiffLine::Removed(line.to_string())),
        );
        lines.extend(
            new.into_iter()
                .map(|line| DiffLine::Added(line.to_string())),
        );
        return lines;
    }
    let (rows, cols) = (old.len(), new.len());
    let mut scores = vec![vec![0u32; cols + 1]; rows + 1];
    for row in (0..rows).rev() {
        for col in (0..cols).rev() {
            scores[row][col] = if old[row] == new[col] {
                scores[row + 1][col + 1] + 1
            } else {
                scores[row + 1][col].max(scores[row][col + 1])
            };
        }
    }
    let mut lines = Vec::new();
    let (mut row, mut col) = (0, 0);
    while row < rows && col < cols {
        if old[row] == new[col] {
            lines.push(DiffLine::Same(old[row].to_string()));
            row += 1;
            col += 1;
        } else if scores[row + 1][col] >= scores[row][col + 1] {
            lines.push(DiffLine::Removed(old[row].to_string()));
            row += 1;
        } else {
            lines.push(DiffLine::Added(new[col].to_string()));
            col += 1;
        }
    }
    lines.extend(
        old[row..]
            .iter()
            .map(|line| DiffLine::Removed(line.to_string())),
    );
    lines.extend(
        new[col..]
            .iter()
            .map(|line| DiffLine::Added(line.to_string())),
    );
    lines
}

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.lines().collect()
    }
}

/// One row of the compare view, numbered like a code diff.
struct CodeLine {
    /// Line number in the older version. Empty when the line was added.
    old: Option<u32>,
    /// Line number in the current note. Empty when the line was removed.
    new: Option<u32>,
    /// `"-"` removed, `"+"` added, or empty when the line is unchanged.
    sign: &'static str,
    text: String,
}

fn code_lines(diff: &[DiffLine]) -> Vec<CodeLine> {
    let mut old_no = 1u32;
    let mut new_no = 1u32;
    diff.iter()
        .map(|line| match line {
            DiffLine::Same(text) => {
                let row = CodeLine {
                    old: Some(old_no),
                    new: Some(new_no),
                    sign: "",
                    text: text.clone(),
                };
                old_no += 1;
                new_no += 1;
                row
            }
            DiffLine::Removed(text) => {
                let row = CodeLine {
                    old: Some(old_no),
                    new: None,
                    sign: "-",
                    text: text.clone(),
                };
                old_no += 1;
                row
            }
            DiffLine::Added(text) => {
                let row = CodeLine {
                    old: None,
                    new: Some(new_no),
                    sign: "+",
                    text: text.clone(),
                };
                new_no += 1;
                row
            }
        })
        .collect()
}

fn line_no(number: Option<u32>) -> String {
    number.map(|number| number.to_string()).unwrap_or_default()
}

struct HistoryPanel {
    _subscription: Option<Subscription>,
}

impl HistoryPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        let subscription =
            HistoryHost::model(cx).map(|model| cx.observe(&model, |_, _, cx| cx.notify()));
        Self {
            _subscription: subscription,
        }
    }
}

impl Render for HistoryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(model) = HistoryHost::model(cx) else {
            return v_flex()
                .size_full()
                .child(ui::empty_message("History is unavailable.", cx));
        };
        let active = Notia::workspace(cx).read(cx).active_path().cloned();
        let revisions = active
            .as_ref()
            .map(|path| model.read(cx).revisions(path))
            .unwrap_or_default();
        let title = active
            .as_ref()
            .map(|path| path.file_stem().to_string())
            .unwrap_or_else(|| "History".to_string());
        let muted = cx.theme().muted_foreground;

        let body = if active.is_none() {
            ui::empty_message("No note is open.", cx).into_any_element()
        } else if revisions.len() < 2 {
            ui::empty_message(
                "Saved edits appear here. Any earlier version can be restored.",
                cx,
            )
            .into_any_element()
        } else {
            let path = active.clone().unwrap();
            v_flex()
                .children(revisions.iter().enumerate().rev().map(|(index, revision)| {
                    let latest = index + 1 == revisions.len();
                    let label = if latest {
                        "Current".to_string()
                    } else if index == 0 {
                        "Original".to_string()
                    } else {
                        change_label(&revisions[index - 1].text, &revision.text)
                    };
                    let version_path = path.clone();
                    let version_text = revision.text.clone();
                    let version_at = revision.at;
                    ui::row(SharedString::from(format!("history-{index}")), cx).child(
                        h_flex().w_full().items_center().gap_1().child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_sm()
                                        .when(latest, |row| row.font_weight(FontWeight::MEDIUM))
                                        .child(format_when(revision.at)),
                                )
                                .child(
                                    div().truncate().text_xs().text_color(muted).child(label),
                                ),
                        )
                        .when(!latest, |row| {
                            let compare_path = version_path.clone();
                            let compare_text = version_text.clone();
                            row.child(
                                Button::new(SharedString::from(format!(
                                    "history-compare-{index}"
                                )))
                                .icon(Icon::new(Lucide::GitCompare))
                                .ghost()
                                .xsmall()
                                .tooltip("Compare")
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    let path = compare_path.clone();
                                    let text = compare_text.clone();
                                    if let Some(model) = HistoryHost::model(cx) {
                                        model.update(cx, |model, cx| {
                                            model.open_compare(path, version_at, text, cx);
                                        });
                                    }
                                    open_compare_popup(window, cx);
                                }),
                            )
                        }),
                    )
                }))
                .into_any_element()
        };

        v_flex()
            .size_full()
            .child(ui::section_header(
                title,
                (revisions.len() >= 2).then_some(revisions.len()),
                cx,
            ))
            .child(
                v_flex()
                    .id("history")
                    .flex_1()
                    .min_h_0()
                    .pb_2()
                    .child(body)
                    .overflow_y_scrollbar(),
            )
    }
}

struct ComparePanel {
    _subscription: Option<Subscription>,
}

impl ComparePanel {
    fn new(cx: &mut Context<Self>) -> Self {
        let subscription =
            HistoryHost::model(cx).map(|model| cx.observe(&model, |_, _, cx| cx.notify()));
        Self {
            _subscription: subscription,
        }
    }
}

impl Render for ComparePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(model) = HistoryHost::model(cx) else {
            return v_flex()
                .size_full()
                .child(ui::empty_message("Compare is unavailable.", cx));
        };
        let selection = model.read(cx).compare.clone();
        let Some(selection) = selection else {
            return v_flex()
                .size_full()
                .child(ui::empty_message("Choose Compare on a saved version.", cx));
        };
        let current = note_text(&selection.path, cx).unwrap_or_default();
        let lines = code_lines(&diff_lines(&selection.text, &current));
        let theme = cx.theme();
        let (muted, foreground, removed, added) = (
            theme.muted_foreground,
            theme.foreground,
            theme.danger,
            theme.success,
        );
        let (removed_bg, added_bg) = (removed.opacity(0.28), added.opacity(0.22));
        let (background, border) = (theme.background, theme.border);
        let (mono, mono_size) = (theme.mono_font_family.clone(), theme.mono_font_size);
        let file_name = selection.path.file_name().to_string();
        let when = format_when(selection.at);
        let restore_path = selection.path.clone();
        let restore_text = selection.text.clone();

        v_flex().size_full().bg(background).child(
            h_flex()
                .flex_none()
                .h(px(32.))
                .px_3()
                .gap_2()
                .items_center()
                .bg(theme.sidebar)
                .border_b_1()
                .border_color(border)
                .child(div().text_sm().child(file_name))
                .child(div().flex_1().min_w_0().truncate().text_xs().text_color(muted).child(when))
                .child(
                    Button::new("compare-restore")
                        .ghost()
                        .xsmall()
                        .label("Restore")
                        .on_click(move |_, window, cx| {
                            restore(&restore_path, restore_text.clone(), window, cx);
                        }),
                ),
        )
        .child(
            v_flex()
                .id("history-compare")
                .flex_1()
                .min_h_0()
                .w_full()
                .font_family(mono)
                .text_size(mono_size)
                .bg(background)
                .children(lines.into_iter().enumerate().map(|(index, line)| {
                    let (marker, marker_color, row_bg) = match line.sign {
                        "-" => ("-", removed, Some(removed_bg)),
                        "+" => ("+", added, Some(added_bg)),
                        _ => ("", muted, None),
                    };
                    let old_no = line_no(line.old);
                    let new_no = line_no(line.new);
                    h_flex()
                        .id(("compare-line", index))
                        .w_full()
                        .min_h(px(20.))
                        .items_center()
                        .text_color(foreground)
                        .when_some(row_bg, |row, background| row.bg(background))
                        .child(line_number(old_no, muted))
                        .child(line_number(new_no, muted))
                        .child(
                            div()
                                .flex_none()
                                .w(px(16.))
                                .text_center()
                                .text_color(marker_color)
                                .child(marker),
                        )
                        .child(
                            div()
                                .pr_3()
                                .whitespace_nowrap()
                                .child(if line.text.is_empty() {
                                    " ".to_string()
                                } else {
                                    line.text
                                }),
                        )
                }))
                .overflow_y_scrollbar(),
        )
    }
}

fn line_number(number: String, color: gpui_kit::Hsla) -> gpui_kit::Div {
    div()
        .flex_none()
        .w(px(36.))
        .pr_2()
        .text_right()
        .text_color(color)
        .child(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_kept_only_when_the_text_changes() {
        let mut log = NoteLog::default();
        assert!(log.record("Hello", 1, 80));
        assert!(!log.record("Hello", 2, 80));
        assert!(log.record("Hello world", 3, 80));
        assert_eq!(log.revisions.len(), 2);
        assert_eq!(log.previous().unwrap().text, "Hello");
        assert_eq!(log.revisions.last().unwrap().text, "Hello world");
    }

    #[test]
    fn old_versions_drop_off_past_the_limit() {
        let mut log = NoteLog::default();
        for n in 0..=5 {
            assert!(log.record(&format!("v{n}"), n, 3));
        }
        assert_eq!(log.revisions.len(), 3);
        assert_eq!(log.revisions[0].text, "v3");
        assert_eq!(log.revisions.last().unwrap().text, "v5");
    }

    #[test]
    fn labels_count_characters_added_or_removed() {
        assert_eq!(change_label("Hi", "Hello"), "+3");
        assert_eq!(change_label("Hello", "Hi"), "-3");
        assert_eq!(change_label("ab", "ba"), "edited");
        assert_eq!(snippet("\n\n  Keep this line\n"), "Keep this line");
    }

    #[test]
    fn timestamps_show_milliseconds_and_older_seconds_still_read() {
        let millis = format_when(1_763_000_000_123);
        let seconds = format_when(1_763_000_000);
        assert_eq!(millis.len(), 23, "{millis}");
        assert_eq!(seconds.len(), 23, "{seconds}");
        assert!(millis.ends_with(".123"), "{millis}");
        assert!(seconds.ends_with(".000"), "{seconds}");
        assert_eq!(&millis[..19], &seconds[..19]);
    }

    #[test]
    fn compare_lines_up_unchanged_text() {
        let lines = diff_lines("a\nb\nc\n", "a\nc\nd\n");
        assert_eq!(
            lines,
            vec![
                DiffLine::Same("a".into()),
                DiffLine::Removed("b".into()),
                DiffLine::Same("c".into()),
                DiffLine::Added("d".into()),
            ]
        );
        let code = code_lines(&lines);
        assert_eq!(code[0].old, Some(1));
        assert_eq!(code[0].new, Some(1));
        assert_eq!(code[0].sign, "");
        assert_eq!(code[1].old, Some(2));
        assert_eq!(code[1].new, None);
        assert_eq!(code[1].sign, "-");
        assert_eq!(code[2].old, Some(3));
        assert_eq!(code[2].new, Some(2));
        assert_eq!(code[3].old, None);
        assert_eq!(code[3].new, Some(3));
        assert_eq!(code[3].sign, "+");
        assert_eq!(code[3].text, "d");
    }
}
