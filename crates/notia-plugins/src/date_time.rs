use chrono::Local;
use gpui_kit::*;
use notia_plugin::{Command, Notia, Plugin, PluginContext, PluginManifest, workspace};

pub struct DateTimePlugin;

impl Plugin for DateTimePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("date-time", "Date and time")
            .with_description("Inserts the current date or time at the cursor.")
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        let has_note = |cx: &App| Notia::workspace(cx).read(cx).active_path().is_some();
        ctx.add_command(
            Command::new("insert-date", "Insert current date", |window, cx| {
                workspace::insert_text(&Local::now().format("%Y-%m-%d").to_string(), window, cx);
            })
            .available_when(has_note),
        );
        ctx.add_command(
            Command::new("insert-time", "Insert current time", |window, cx| {
                workspace::insert_text(&Local::now().format("%H:%M").to_string(), window, cx);
            })
            .available_when(has_note),
        );
    }
}
