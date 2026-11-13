use chrono::{Days, Local, NaiveDate};
use gpui_kit::component::{IconName, WindowExt as _};
use gpui_kit::*;
use notia_plugin::{
    Command, Notia, OpenOptions, Plugin, PluginContext, PluginManifest, RibbonItem,
    core::VaultPath, workspace,
};

/// The folder daily notes live in.
const FOLDER: &str = "Daily";

pub struct DailyNotesPlugin;

impl Plugin for DailyNotesPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("daily-notes", "Daily notes").with_description(
            "Opens a note for today in the Daily folder, creating it when needed.",
        )
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        ctx.add_ribbon_item(
            RibbonItem::new(
                "today",
                IconName::Calendar,
                "Open today's daily note",
                |window, cx| open_daily_note(0, window, cx),
            )
            .with_order(20),
        );
        ctx.add_command(
            Command::new("today", "Open today's daily note", |window, cx| {
                open_daily_note(0, window, cx)
            })
            .with_icon(IconName::Calendar)
            .with_keybinding("secondary-shift-t"),
        );
        ctx.add_command(Command::new(
            "yesterday",
            "Open yesterday's daily note",
            |window, cx| open_daily_note(-1, window, cx),
        ));
        ctx.add_command(Command::new(
            "tomorrow",
            "Open tomorrow's daily note",
            |window, cx| open_daily_note(1, window, cx),
        ));
    }
}

fn daily_note_path(date: NaiveDate) -> VaultPath {
    VaultPath::new(format!("{FOLDER}/{}.md", date.format("%Y-%m-%d")))
}

fn open_daily_note(offset_days: i64, window: &mut Window, cx: &mut App) {
    let today = Local::now().date_naive();
    let date = if offset_days >= 0 {
        today.checked_add_days(Days::new(offset_days as u64))
    } else {
        today.checked_sub_days(Days::new(offset_days.unsigned_abs()))
    }
    .unwrap_or(today);
    let path = daily_note_path(date);

    let vault = Notia::vault(cx);
    if !vault.read(cx).vault().exists(&path) {
        let content = format!("# {}\n\n", date.format("%A, %B %-d, %Y"));
        if let Err(error) = vault.update(cx, |vault, cx| vault.save_note(&path, &content, cx)) {
            window.push_notification(format!("Could not create the daily note: {error}"), cx);
            return;
        }
    }
    workspace::open_note(path, OpenOptions::new(), window, cx);
}
