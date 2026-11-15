//! Notes the user has opened, and back and forward through that trail.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
// A glob import would pull in GPUI's `test` macro and shadow `#[test]` below.
use gpui_kit::{
    App, AppContext, Context, Div, Entity, FontWeight, Global, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};
use notia_plugin::{
    Command, Notia, OpenOptions, Panel, PanelPlacement, Plugin, PluginContext, PluginManifest,
    VaultEvent, WorkspaceEvent, core::VaultPath, workspace,
};
use serde::{Deserialize, Serialize};

use crate::ui;

const PLUGIN_ID: &str = "history";
const PANEL_ID: &str = "history:history";
/// Opened notes kept in the list, newest first.
const MAX_VISITS: usize = 100;
/// Notes remembered for back and forward during this session.
const MAX_STACK: usize = 100;

pub struct HistoryPlugin;

impl Plugin for HistoryPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(PLUGIN_ID, "History").with_description(
            "Lists notes you have opened, newest first, and moves back and forward through them.",
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
                PanelPlacement::Left,
                |_, cx| cx.new(HistoryPanel::new),
            )
            .with_order(20),
        );
        ctx.add_command(
            Command::new("show", "Show history", |window, cx| {
                workspace::show_panel(PANEL_ID, window, cx);
            })
            .with_icon(Icon::new(Lucide::Clock)),
        );
        ctx.add_command(
            Command::new("back", "Go back", go_back)
                .with_icon(Icon::new(Lucide::ArrowLeft))
                .with_keybinding("secondary-alt-left")
                .available_when(can_go_back),
        );
        ctx.add_command(
            Command::new("forward", "Go forward", go_forward)
                .with_icon(Icon::new(Lucide::ArrowRight))
                .with_keybinding("secondary-alt-right")
                .available_when(can_go_forward),
        );
        ctx.add_command(
            Command::new("clear", "Clear history", |_, cx| clear_history(cx))
                .available_when(can_clear),
        );
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

fn go_back(window: &mut Window, cx: &mut App) {
    let Some(model) = HistoryHost::model(cx) else {
        return;
    };
    let Some(path) = model.update(cx, |model, cx| model.go_back(cx)) else {
        return;
    };
    workspace::open_note(path, OpenOptions::new(), window, cx);
}

fn go_forward(window: &mut Window, cx: &mut App) {
    let Some(model) = HistoryHost::model(cx) else {
        return;
    };
    let Some(path) = model.update(cx, |model, cx| model.go_forward(cx)) else {
        return;
    };
    workspace::open_note(path, OpenOptions::new(), window, cx);
}

fn clear_history(cx: &mut App) {
    if let Some(model) = HistoryHost::model(cx) {
        model.update(cx, |model, cx| model.clear(cx));
    }
}

fn can_go_back(cx: &App) -> bool {
    HistoryHost::model(cx).is_some_and(|model| model.read(cx).history.can_go_back())
}

fn can_go_forward(cx: &App) -> bool {
    HistoryHost::model(cx).is_some_and(|model| model.read(cx).history.can_go_forward())
}

fn can_clear(cx: &App) -> bool {
    HistoryHost::model(cx).is_some_and(|model| model.read(cx).history.can_clear())
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Visit {
    path: VaultPath,
    opened_at: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct SavedHistory {
    visits: Vec<Visit>,
}

/// The opened-note list and the session's back and forward stacks.
#[derive(Default)]
struct History {
    visits: Vec<Visit>,
    back: Vec<VaultPath>,
    forward: Vec<VaultPath>,
    current: Option<VaultPath>,
}

impl History {
    fn from_saved(saved: SavedHistory) -> Self {
        let mut history = Self {
            visits: saved.visits,
            ..Self::default()
        };
        history.dedup();
        history.visits.truncate(MAX_VISITS);
        history
    }

    fn can_go_back(&self) -> bool {
        self.back
            .iter()
            .any(|path| self.current.as_ref() != Some(path))
    }

    fn can_go_forward(&self) -> bool {
        self.forward
            .iter()
            .any(|path| self.current.as_ref() != Some(path))
    }

    fn can_clear(&self) -> bool {
        self.visits.len() > 1 || !self.back.is_empty() || !self.forward.is_empty()
    }

    /// Records `path` as the note the user just opened.
    ///
    /// Opening the note that is already current only refreshes its time, so
    /// back and forward can set `current` before the workspace reports it.
    fn note_opened(&mut self, path: VaultPath, at: i64) -> bool {
        let moved = if self.current.as_ref() == Some(&path) {
            false
        } else {
            if let Some(current) = self.current.clone() {
                self.push_back(current);
            }
            self.forward.clear();
            self.current = Some(path.clone());
            true
        };
        self.touch(path, at) || moved
    }

    fn note_closed(&mut self) -> bool {
        let Some(current) = self.current.take() else {
            return false;
        };
        self.push_back(current);
        self.forward.clear();
        true
    }

    fn go_back(&mut self, at: i64) -> Option<VaultPath> {
        let path = pop_different(&mut self.back, self.current.as_ref())?;
        if let Some(current) = self.current.clone() {
            self.push_forward(current);
        }
        self.current = Some(path.clone());
        self.touch(path.clone(), at);
        Some(path)
    }

    fn go_forward(&mut self, at: i64) -> Option<VaultPath> {
        let path = pop_different(&mut self.forward, self.current.as_ref())?;
        if let Some(current) = self.current.clone() {
            self.push_back(current);
        }
        self.current = Some(path.clone());
        self.touch(path.clone(), at);
        Some(path)
    }

    fn clear(&mut self, at: i64) -> bool {
        if !self.can_clear() {
            return false;
        }
        let current = self.current.clone();
        self.back.clear();
        self.forward.clear();
        self.visits.clear();
        if let Some(current) = current {
            self.touch(current, at);
        }
        true
    }

    fn rebase(&mut self, from: &VaultPath, to: &VaultPath) -> bool {
        let mut changed = false;
        for visit in &mut self.visits {
            changed |= map_path(&mut visit.path, from, to);
        }
        for path in &mut self.back {
            changed |= map_path(path, from, to);
        }
        for path in &mut self.forward {
            changed |= map_path(path, from, to);
        }
        if let Some(current) = &mut self.current {
            changed |= map_path(current, from, to);
        }
        if changed {
            self.dedup();
        }
        changed
    }

    fn drop_under(&mut self, root: &VaultPath) -> bool {
        let visits = self.visits.len();
        self.visits.retain(|visit| !visit.path.starts_with(root));
        let back = self.back.len();
        self.back.retain(|path| !path.starts_with(root));
        let forward = self.forward.len();
        self.forward.retain(|path| !path.starts_with(root));
        let clear_current = self
            .current
            .as_ref()
            .is_some_and(|path| path.starts_with(root));
        if clear_current {
            self.current = None;
        }
        visits != self.visits.len()
            || back != self.back.len()
            || forward != self.forward.len()
            || clear_current
    }

    fn prune(&mut self, exists: impl Fn(&VaultPath) -> bool) -> bool {
        let visits = self.visits.len();
        self.visits.retain(|visit| exists(&visit.path));
        let back = self.back.len();
        self.back.retain(|path| exists(path));
        let forward = self.forward.len();
        self.forward.retain(|path| exists(path));
        let clear_current = self.current.as_ref().is_some_and(|path| !exists(path));
        if clear_current {
            self.current = None;
        }
        visits != self.visits.len()
            || back != self.back.len()
            || forward != self.forward.len()
            || clear_current
    }

    fn push_back(&mut self, path: VaultPath) {
        push_capped(&mut self.back, path);
    }

    fn push_forward(&mut self, path: VaultPath) {
        push_capped(&mut self.forward, path);
    }

    fn touch(&mut self, path: VaultPath, at: i64) -> bool {
        if self
            .visits
            .first()
            .is_some_and(|visit| visit.path == path && visit.opened_at == at)
        {
            return false;
        }
        self.visits.retain(|visit| visit.path != path);
        self.visits.insert(
            0,
            Visit {
                path,
                opened_at: at,
            },
        );
        self.visits.truncate(MAX_VISITS);
        true
    }

    fn dedup(&mut self) {
        let mut seen = HashSet::new();
        self.visits.retain(|visit| seen.insert(visit.path.clone()));
    }
}

/// Pops the newest entry that is not the note already open.
fn pop_different(stack: &mut Vec<VaultPath>, current: Option<&VaultPath>) -> Option<VaultPath> {
    while let Some(path) = stack.pop() {
        if current != Some(&path) {
            return Some(path);
        }
    }
    None
}

fn push_capped(stack: &mut Vec<VaultPath>, path: VaultPath) {
    if stack.last() == Some(&path) {
        return;
    }
    stack.push(path);
    let extra = stack.len().saturating_sub(MAX_STACK);
    if extra > 0 {
        stack.drain(0..extra);
    }
}

fn map_path(path: &mut VaultPath, from: &VaultPath, to: &VaultPath) -> bool {
    match path.rebase(from, to) {
        Some(moved) if moved != *path => {
            *path = moved;
            true
        }
        _ => false,
    }
}

fn unix_now() -> i64 {
    Utc::now().timestamp()
}

fn local_date(opened_at: i64) -> Option<NaiveDate> {
    DateTime::<Utc>::from_timestamp(opened_at, 0)
        .map(|time| time.with_timezone(&Local).date_naive())
}

fn day_label(opened_at: i64, today: NaiveDate) -> String {
    let Some(date) = local_date(opened_at) else {
        return "Earlier".to_string();
    };
    if date == today {
        "Today".to_string()
    } else if today.pred_opt() == Some(date) {
        "Yesterday".to_string()
    } else if date.year() == today.year() {
        format!("{} {}", date.format("%b"), date.day())
    } else {
        format!("{} {}, {}", date.format("%b"), date.day(), date.year())
    }
}

fn format_clock(opened_at: i64) -> String {
    DateTime::<Utc>::from_timestamp(opened_at, 0)
        .map(|time| time.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Groups `visits` by day, newest first, in the order they already have.
fn group_visits(visits: &[Visit], today: NaiveDate) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (ix, visit) in visits.iter().enumerate() {
        let label = day_label(visit.opened_at, today);
        if let Some((last, indexes)) = groups.last_mut()
            && *last == label
        {
            indexes.push(ix);
        } else {
            groups.push((label, vec![ix]));
        }
    }
    groups
}

struct HistoryModel {
    history: History,
    _subscriptions: Vec<Subscription>,
}

impl HistoryModel {
    fn new(cx: &mut Context<Self>) -> Self {
        let saved = Notia::load_plugin_data(PLUGIN_ID, cx);
        let mut history = History::from_saved(saved);
        let opened = Notia::workspace(cx)
            .read(cx)
            .active_path()
            .cloned()
            .is_some_and(|path| history.note_opened(path, unix_now()));
        let subscriptions = vec![
            cx.subscribe(&Notia::workspace(cx), |this, workspace, event, cx| {
                if matches!(event, WorkspaceEvent::ActiveNoteChanged) {
                    let active = workspace.read(cx).active_path().cloned();
                    this.on_active(active, cx);
                }
            }),
            cx.subscribe(&Notia::vault(cx), |this, _, event, cx| {
                this.on_vault(event, cx);
            }),
        ];
        let model = Self {
            history,
            _subscriptions: subscriptions,
        };
        if opened {
            model.save(cx);
        }
        model
    }

    fn on_active(&mut self, active: Option<VaultPath>, cx: &mut Context<Self>) {
        let at = unix_now();
        let changed = match active {
            Some(path) => self.history.note_opened(path, at),
            None => self.history.note_closed(),
        };
        if changed {
            self.save(cx);
        }
        // The open note changed even when the list did not, and the panel
        // highlights whichever note is open.
        cx.notify();
    }

    fn on_vault(&mut self, event: &VaultEvent, cx: &mut Context<Self>) {
        let changed = match event {
            VaultEvent::Renamed { from, to } => self.history.rebase(from, to),
            VaultEvent::Trashed(path) => self.history.drop_under(path),
            VaultEvent::TreeChanged => {
                let model = Notia::vault(cx);
                let vault = model.read(cx);
                if vault.is_loaded() {
                    let vault = vault.vault().clone();
                    self.history.prune(|path| vault.exists(path))
                } else {
                    false
                }
            }
            VaultEvent::NotesChanged(_) => false,
        };
        if changed {
            self.save(cx);
            cx.notify();
        }
    }

    fn go_back(&mut self, cx: &mut Context<Self>) -> Option<VaultPath> {
        let path = self.history.go_back(unix_now())?;
        self.save(cx);
        cx.notify();
        Some(path)
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) -> Option<VaultPath> {
        let path = self.history.go_forward(unix_now())?;
        self.save(cx);
        cx.notify();
        Some(path)
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        if self.history.clear(unix_now()) {
            self.save(cx);
            cx.notify();
        }
    }

    fn save(&self, cx: &App) {
        let saved = SavedHistory {
            visits: self.history.visits.clone(),
        };
        Notia::save_plugin_data(PLUGIN_ID, &saved, cx);
    }
}

struct HistoryPanel {
    _subscriptions: Vec<Subscription>,
}

impl HistoryPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(model) = HistoryHost::model(cx) {
            subscriptions.push(cx.observe(&model, |_, _, cx| cx.notify()));
        }
        Self {
            _subscriptions: subscriptions,
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
        let vault = Notia::vault(cx).read(cx).vault().clone();
        let (visits, can_back, can_forward, can_clear) = {
            let history = &model.read(cx).history;
            let visits = history
                .visits
                .iter()
                .filter(|visit| vault.exists(&visit.path))
                .cloned()
                .collect::<Vec<_>>();
            (
                visits,
                history.can_go_back(),
                history.can_go_forward(),
                history.can_clear(),
            )
        };
        let active = Notia::workspace(cx).read(cx).active_path().cloned();
        let today = Local::now().date_naive();
        let groups = group_visits(&visits, today);

        let toolbar = h_flex()
            .px_2()
            .pt_2()
            .gap_1()
            .items_center()
            .child(
                Button::new("history-back")
                    .icon(Icon::new(Lucide::ArrowLeft))
                    .ghost()
                    .small()
                    .tooltip("Go back")
                    .disabled(!can_back)
                    .on_click(|_, window, cx| go_back(window, cx)),
            )
            .child(
                Button::new("history-forward")
                    .icon(Icon::new(Lucide::ArrowRight))
                    .ghost()
                    .small()
                    .tooltip("Go forward")
                    .disabled(!can_forward)
                    .on_click(|_, window, cx| go_forward(window, cx)),
            )
            .child(div().flex_1())
            .when(can_clear, |bar| {
                bar.child(
                    Button::new("history-clear")
                        .icon(Icon::new(Lucide::Trash))
                        .ghost()
                        .small()
                        .tooltip("Clear history")
                        .on_click(|_, _, cx| clear_history(cx)),
                )
            });

        let list = if visits.is_empty() {
            ui::empty_message("Notes you open show up here.", cx).into_any_element()
        } else {
            v_flex()
                .children(groups.iter().map(|(label, indexes)| {
                    v_flex()
                        .child(ui::section_header(label.clone(), None, cx))
                        .children(
                            indexes
                                .iter()
                                .map(|ix| visit_row(&visits[*ix], active.as_ref(), cx)),
                        )
                }))
                .into_any_element()
        };

        v_flex().size_full().child(toolbar).child(
            v_flex()
                .id("history")
                .flex_1()
                .min_h_0()
                .pb_2()
                .child(list)
                .overflow_y_scrollbar(),
        )
    }
}

fn visit_row(visit: &Visit, active: Option<&VaultPath>, cx: &App) -> Stateful<Div> {
    let muted = cx.theme().muted_foreground;
    let path = visit.path.clone();
    let open = path.clone();
    let folder = path
        .parent()
        .filter(|parent| !parent.is_root())
        .map(|parent| parent.to_string());
    let current = active == Some(&path);
    ui::row(SharedString::from(format!("history-{path}")), cx)
        .when(current, |row| row.bg(cx.theme().list_active))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .when(current, |name| name.font_weight(FontWeight::MEDIUM))
                        .child(path.file_stem().to_string()),
                )
                .when_some(folder, |row, folder| {
                    row.child(
                        div()
                            .flex_none()
                            .max_w(px(120.))
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .child(folder),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(format_clock(visit.opened_at)),
                ),
        )
        .on_click(move |event, window, cx| {
            let options = if event.modifiers().secondary(cx) {
                OpenOptions::new().in_new_tab()
            } else {
                OpenOptions::new()
            };
            workspace::open_note(open.clone(), options, window, cx);
        })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn note(path: &str) -> VaultPath {
        VaultPath::new(path)
    }

    fn stamp(date: NaiveDate) -> i64 {
        Local
            .from_local_datetime(&date.and_hms_opt(15, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp()
    }

    #[test]
    fn opening_a_note_moves_it_to_the_front_and_builds_a_back_stack() {
        let mut history = History::default();
        history.note_opened(note("a.md"), 1);
        history.note_opened(note("b.md"), 2);
        history.note_opened(note("a.md"), 3);
        history.note_opened(note("a.md"), 3);

        assert_eq!(
            history
                .visits
                .iter()
                .map(|visit| visit.path.as_str())
                .collect::<Vec<_>>(),
            ["a.md", "b.md"]
        );
        assert_eq!(history.visits[0].opened_at, 3);
        assert_eq!(history.back.len(), 2);
        assert!(history.forward.is_empty());
        assert_eq!(history.current.as_ref().unwrap().as_str(), "a.md");
    }

    #[test]
    fn back_and_forward_revisit_notes_until_a_new_one_is_opened() {
        let mut history = History::default();
        history.note_opened(note("a.md"), 1);
        history.note_opened(note("b.md"), 2);
        history.note_opened(note("c.md"), 3);

        assert_eq!(history.go_back(4).unwrap().as_str(), "b.md");
        assert_eq!(history.go_back(5).unwrap().as_str(), "a.md");
        assert!(history.go_back(6).is_none());
        assert_eq!(history.go_forward(7).unwrap().as_str(), "b.md");
        assert_eq!(history.visits[0].path.as_str(), "b.md");

        history.note_opened(note("d.md"), 8);
        assert!(history.forward.is_empty());
        assert_eq!(history.current.as_ref().unwrap().as_str(), "d.md");
        assert_eq!(history.go_back(9).unwrap().as_str(), "b.md");
    }

    #[test]
    fn stacks_drop_the_oldest_entry_past_the_cap() {
        let mut history = History::default();
        for n in 0..=MAX_STACK + 5 {
            history.note_opened(note(&format!("n{n}.md")), n as i64);
        }
        assert_eq!(history.back.len(), MAX_STACK);
        assert_eq!(history.back[0].as_str(), "n5.md");
        assert_eq!(history.visits.len(), MAX_VISITS);
    }

    #[test]
    fn clear_keeps_only_the_open_note() {
        let mut history = History::default();
        history.note_opened(note("a.md"), 1);
        history.note_opened(note("b.md"), 2);
        assert!(history.clear(3));
        assert_eq!(history.visits.len(), 1);
        assert_eq!(history.visits[0].path.as_str(), "b.md");
        assert!(history.back.is_empty());
        assert!(!history.can_clear());
        assert!(!history.clear(4));
    }

    #[test]
    fn rename_trash_and_missing_files_leave_the_lists() {
        let mut history = History::default();
        history.note_opened(note("Keep.md"), 1);
        history.note_opened(note("Projects/Plan.md"), 2);
        history.note_opened(note("Projects/Done.md"), 3);
        assert!(history.rebase(&note("Projects"), &note("Archive")));
        assert_eq!(
            history.current.as_ref().unwrap().as_str(),
            "Archive/Done.md"
        );
        assert!(
            history
                .visits
                .iter()
                .any(|visit| visit.path.as_str() == "Archive/Plan.md")
        );

        assert!(history.drop_under(&note("Archive/Done.md")));
        assert!(history.current.is_none());
        assert!(
            history
                .visits
                .iter()
                .all(|visit| visit.path.as_str() != "Archive/Done.md")
        );

        assert!(history.prune(|path| path.as_str() == "Keep.md"));
        assert_eq!(history.visits.len(), 1);
        assert_eq!(history.visits[0].path.as_str(), "Keep.md");
    }

    #[test]
    fn days_group_today_yesterday_and_older_dates() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        let visits = vec![
            Visit {
                path: note("now.md"),
                opened_at: stamp(today),
            },
            Visit {
                path: note("yesterday.md"),
                opened_at: stamp(today.pred_opt().unwrap()),
            },
            Visit {
                path: note("november.md"),
                opened_at: stamp(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()),
            },
            Visit {
                path: note("last-year.md"),
                opened_at: stamp(NaiveDate::from_ymd_opt(2025, 12, 31).unwrap()),
            },
        ];
        let groups = group_visits(&visits, today);
        let labels: Vec<&str> = groups.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["Today", "Yesterday", "Nov 1", "Dec 31, 2025"]);
    }
}
