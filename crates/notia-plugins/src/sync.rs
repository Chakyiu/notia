//! Syncs the vault with a server the user runs.
//!
//! Notes, images, and every other file go both ways. Files under `.notia`
//! sync only when the user asks for config. The comparison lives in
//! `notia-sync`, which the `notia-sync-server` binary serves.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::{ToastStack, ToastStackState};
use gpui_kit::component::{ActiveTheme as _, Icon, WindowExt as _, button::Button, h_flex};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    Command, Notia, Plugin, PluginContext, PluginManifest, RibbonItem, Setting, SettingsSection,
    SettingsTab, StatusItem,
};
use notia_sync::{MAX_FILE_BYTES, RemoteCursor, SyncOptions, probe, remote_cursor, sync_vault};
use serde::{Deserialize, Deserializer, Serialize};

const PLUGIN_ID: &str = "sync";
const NEED_SERVER: &str = "Add a server in Settings → Sync";
/// How many notices stay in the pile. Older ones drop off the back.
const MAX_NOTICES: usize = 4;
/// Wait after a local change so a burst of saves becomes one sync.
const AFTER_CHANGE: Duration = Duration::from_secs(2);
/// Real-time sync waits only long enough to fold one save's writes together.
const REALTIME_AFTER_CHANGE: Duration = Duration::from_millis(300);
/// After a real-time sync fails, wait before asking the server again.
const REALTIME_RETRY: Duration = Duration::from_secs(5);
const OPEN_DELAY: Duration = Duration::from_secs(3);

pub struct SyncPlugin;

impl Plugin for SyncPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(PLUGIN_ID, "Sync").with_description(
            "Syncs every file in the vault with a server you host. Syncing settings and themes is optional.",
        )
    }

    fn activate(&self, ctx: &mut PluginContext, cx: &mut App) {
        let controller = cx.new(SyncController::new);
        cx.set_global(SyncHost { controller });
        ctx.on_deactivate(|cx| {
            cx.remove_global::<SyncHost>();
        });

        ctx.add_command(
            Command::new("sync-now", "Sync now", |window, cx| {
                request_sync(window, cx)
            })
            .with_icon(Icon::new(Lucide::RefreshCw)),
        );
        ctx.add_ribbon_item(
            RibbonItem::new(
                "sync",
                Icon::new(Lucide::RefreshCw),
                "Sync now",
                |window, cx| request_sync(window, cx),
            )
            .with_order(40),
        );
        ctx.add_status_item(
            StatusItem::new("state", |_, cx| cx.new(SyncStatus::new)).with_order(20),
        );
        ctx.add_settings_tab(
            SettingsTab::new("options", settings_page).with_icon(Icon::new(Lucide::RefreshCw)),
        );
    }
}

struct SyncHost {
    controller: Entity<SyncController>,
}

impl Global for SyncHost {}

struct SyncController {
    phase: SharedString,
    busy: bool,
    queued: bool,
    queued_window: Option<AnyWindowHandle>,
    run_window: Option<AnyWindowHandle>,
    last_sync: Option<Instant>,
    syncing: Option<Task<()>>,
    debounce: Option<Task<()>>,
    check: Option<Task<()>>,
    /// Server cursor last caught up with. Real-time sync transfers files when it moves.
    seen: Option<RemoteCursor>,
    poll: Option<Task<()>>,
    poll_epoch: u64,
    retry_after: Option<Instant>,
    logged_poll_error: Option<String>,
    /// Notices in the pile, oldest first. The newest is the front card.
    notices: Vec<(u64, SharedString)>,
    next_notice: u64,
    notice_tasks: Vec<Task<()>>,
    _ticker: Task<()>,
    _watch: Subscription,
}

impl SyncController {
    fn new(cx: &mut Context<Self>) -> Self {
        let vault = Notia::vault(cx);
        let watch = cx.observe(&vault, |this, _, cx| this.on_vault(cx));
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.on_tick(cx)).is_err() {
                    break;
                }
            }
        });
        let first_wait = if SyncSettings::load(cx).real_time {
            REALTIME_AFTER_CHANGE
        } else {
            OPEN_DELAY
        };
        let debounce = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(first_wait).await;
            let _ = this.update(cx, |this, cx| {
                let settings = SyncSettings::load(cx);
                if settings.auto || settings.real_time {
                    this.start(None, cx);
                }
            });
        });
        Self {
            phase: if SyncSettings::load(cx).configured() {
                "Sync is ready".into()
            } else {
                "".into()
            },
            busy: false,
            queued: false,
            queued_window: None,
            run_window: None,
            last_sync: None,
            syncing: None,
            debounce: Some(debounce),
            check: None,
            seen: None,
            poll: None,
            poll_epoch: 0,
            retry_after: None,
            logged_poll_error: None,
            notices: Vec::new(),
            next_notice: 0,
            notice_tasks: Vec::new(),
            _ticker: ticker,
            _watch: watch,
        }
    }

    /// Adds `message` to the front of the pile, then lets it leave.
    fn show_notice(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let id = self.next_notice;
        self.next_notice += 1;
        self.notices.push((id, message.into()));
        if self.notices.len() > MAX_NOTICES {
            self.notices.drain(0..self.notices.len() - MAX_NOTICES);
        }
        cx.notify();
        self.notice_tasks.push(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| {
                this.notices.retain(|(notice_id, _)| *notice_id != id);
                cx.notify();
            });
        }));
        if self.notice_tasks.len() > MAX_NOTICES * 2 {
            self.notice_tasks
                .drain(0..self.notice_tasks.len() - MAX_NOTICES);
        }
    }

    fn on_vault(&mut self, cx: &mut Context<Self>) {
        let settings = SyncSettings::load(cx);
        if !settings.configured() || !(settings.auto || settings.real_time) {
            return;
        }
        let wait = if settings.real_time {
            REALTIME_AFTER_CHANGE
        } else {
            AFTER_CHANGE
        };
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| this.start(None, cx));
        }));
    }

    fn on_tick(&mut self, cx: &mut Context<Self>) {
        let settings = SyncSettings::load(cx);
        if !settings.configured() || self.busy {
            return;
        }
        if settings.real_time {
            self.watch_server(cx);
            return;
        }
        if !settings.auto {
            return;
        }
        let interval = Duration::from_secs(settings.interval_seconds);
        let due = self
            .last_sync
            .is_none_or(|started| started.elapsed() >= interval);
        if due {
            self.start(None, cx);
        }
    }

    /// Asks the server whether the vault moved. A full sync runs when it did.
    fn watch_server(&mut self, cx: &mut Context<Self>) {
        if self.poll.is_some() || self.retry_after.is_some_and(|time| Instant::now() < time) {
            return;
        }
        let Some(options) = SyncSettings::load(cx).options(cx) else {
            return;
        };
        let epoch = self.poll_epoch;
        self.poll = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { remote_cursor(&options) })
                .await;
            let _ = this.update(cx, |this, cx| this.on_cursor(epoch, result, cx));
        }));
    }

    fn on_cursor(
        &mut self,
        epoch: u64,
        result: Result<RemoteCursor, notia_sync::SyncError>,
        cx: &mut Context<Self>,
    ) {
        self.poll = None;
        if self.poll_epoch != epoch || self.busy || !SyncSettings::load(cx).real_time {
            return;
        }
        if self.retry_after.is_some_and(|time| Instant::now() < time) {
            return;
        }
        match result {
            Ok(cursor) => {
                self.logged_poll_error = None;
                let same = self.seen.as_ref() == Some(&cursor);
                let retrying = self.retry_after.take().is_some();
                if same && !retrying {
                    return;
                }
                self.start(None, cx);
            }
            Err(error) => {
                let message = error.to_string();
                if self.logged_poll_error.as_deref() != Some(message.as_str()) {
                    log::warn!("sync could not check the server: {message}");
                    self.logged_poll_error = Some(message);
                }
            }
        }
    }

    fn on_settings(&mut self, cx: &mut Context<Self>) {
        if !self.busy && !SyncSettings::load(cx).configured() {
            self.phase = "".into();
        }
        cx.notify();
    }

    fn start(&mut self, window: Option<AnyWindowHandle>, cx: &mut Context<Self>) {
        if self.busy {
            self.queued = true;
            if window.is_some() {
                self.queued_window = window;
            }
            return;
        }
        if !SyncSettings::load(cx).configured() {
            if window.is_some() {
                self.phase = NEED_SERVER.into();
                self.show_notice(NEED_SERVER, cx);
            }
            return;
        }
        if let Err(error) = flush_active_note(cx) {
            let message = format!("Sync failed: {error}");
            self.phase = message.clone().into();
            if window.is_some() {
                self.show_notice(message, cx);
            } else {
                cx.notify();
            }
            return;
        }
        let Some(options) = SyncSettings::load(cx).options(cx) else {
            return;
        };
        self.busy = true;
        self.run_window = window;
        self.last_sync = Some(Instant::now());
        self.phase = "Syncing…".into();
        cx.notify();
        self.syncing = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { sync_vault(&options) })
                .await;
            let _ = this.update(cx, |this, cx| this.finish(result, cx));
        }));
    }

    fn finish(
        &mut self,
        result: Result<notia_sync::SyncReport, notia_sync::SyncError>,
        cx: &mut Context<Self>,
    ) {
        self.poll_epoch = self.poll_epoch.wrapping_add(1);
        self.busy = false;
        self.syncing = None;
        let failed = match &result {
            Ok(report) => {
                if let Some(cursor) = report.cursor.clone() {
                    self.seen = Some(cursor);
                }
                !report.errors.is_empty()
            }
            Err(_) => true,
        };
        if SyncSettings::load(cx).real_time {
            self.retry_after = failed.then(|| Instant::now() + REALTIME_RETRY);
        }
        let message = match result {
            Ok(report) => {
                if report.changed_disk() {
                    Notia::vault(cx).update(cx, |vault, cx| vault.rescan(cx));
                }
                let message = report.summary();
                log::info!("{message}");
                message
            }
            Err(error) => {
                log::warn!("sync failed: {error}");
                format!("Sync failed: {error}")
            }
        };
        self.phase = message.clone().into();
        if self.run_window.take().is_some() {
            self.show_notice(message, cx);
        } else {
            cx.notify();
        }
        if self.queued {
            self.queued = false;
            let window = self.queued_window.take();
            self.start(window, cx);
        }
    }
}

fn request_sync(window: &mut Window, cx: &mut App) {
    let Some(host) = cx.try_global::<SyncHost>() else {
        return;
    };
    let controller = host.controller.clone();
    let window = window.window_handle();
    controller.update(cx, |this, cx| this.start(Some(window), cx));
}

fn test_connection(_window: &mut Window, cx: &mut App) {
    let Some(host) = cx.try_global::<SyncHost>() else {
        return;
    };
    let controller = host.controller.clone();
    let Some(options) = SyncSettings::load(cx).options(cx) else {
        controller.update(cx, |this, cx| this.show_notice(NEED_SERVER, cx));
        return;
    };
    controller.update(cx, |this, cx| {
        this.check = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { probe(&options) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let message = match result {
                    Ok(0) => "Connected. This vault has no files on the server yet.".to_string(),
                    Ok(1) => "Connected. The server has 1 file for this vault.".to_string(),
                    Ok(count) => format!("Connected. The server has {count} files for this vault."),
                    Err(error) => format!("Could not connect: {error}"),
                };
                if !this.busy {
                    this.phase = message.clone().into();
                }
                this.show_notice(message, cx);
            });
        }));
    });
}

fn flush_active_note(cx: &mut App) -> Result<(), String> {
    let Some(note) = Notia::workspace(cx).read(cx).active_note().cloned() else {
        return Ok(());
    };
    // A canvas or kanban writes its own file. The text published for it used
    // to be empty, and saving that here replaced the board.
    if !note.path().is_markdown() {
        return Ok(());
    }
    let path = note.path().clone();
    let text = note.text().to_string();
    let unchanged = Notia::vault(cx)
        .read(cx)
        .read_note(&path)
        .is_ok_and(|disk| disk == text);
    if unchanged {
        return Ok(());
    }
    Notia::vault(cx)
        .update(cx, |vault, cx| vault.save_note(&path, &text, cx))
        .map_err(|error| format!("could not save {path} before syncing: {error}"))
}

struct SyncStatus {
    _watch: Subscription,
    /// Kept across frames so the pile can measure itself and remember a hover.
    stack: ToastStackState,
}

impl SyncStatus {
    fn new(cx: &mut Context<Self>) -> Self {
        let controller = cx.global::<SyncHost>().controller.clone();
        Self {
            _watch: cx.observe(&controller, |_, _, cx| cx.notify()),
            stack: ToastStackState::default(),
        }
    }
}

impl Render for SyncStatus {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (phase, notices) = cx
            .try_global::<SyncHost>()
            .map(|host| {
                let controller = host.controller.read(cx);
                (controller.phase.clone(), controller.notices.clone())
            })
            .unwrap_or_default();
        let toast =
            (!notices.is_empty()).then(|| sync_notice(&notices, self.stack.clone(), window, cx));
        div()
            .id("sync-status")
            .when(!phase.is_empty(), |this| {
                this.cursor_pointer()
                    .child(phase)
                    .on_click(|_, window, cx| request_sync(window, cx))
            })
            .children(toast)
    }
}

/// A pile of cards above the settings panel, at the bottom right and clear of
/// the panel's close button. Dialogs paint at priority 100. Window
/// notifications paint at 180, and tooltips at 200.
fn sync_notice(
    notices: &[(u64, SharedString)],
    stack: ToastStackState,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let mut pile = ToastStack::new("sync-notices", stack)
        .placement(Anchor::BottomRight)
        .w(px(360.));
    for (id, message) in notices {
        let id = *id;
        pile = pile.item(
            SharedString::from(format!("sync-notice-{id}")),
            div()
                .id(("sync-notice", id))
                .w_full()
                .px_4()
                .py_3()
                .rounded(theme.radius)
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
                .text_sm()
                .child(message.clone())
                .on_click(move |_, _, cx| {
                    let Some(host) = cx.try_global::<SyncHost>() else {
                        return;
                    };
                    let controller = host.controller.clone();
                    controller.update(cx, |this, cx| {
                        this.notices.retain(|(notice_id, _)| *notice_id != id);
                        cx.notify();
                    });
                }),
        );
    }
    let size = window.viewport_size();
    let corner = point(size.width - px(16.), size.height - px(48.));
    deferred(
        anchored()
            .snap_to_window()
            .anchor(Anchor::BottomRight)
            .position(corner)
            .child(pile),
    )
    // Above dialogs (`POPUP_PRIORITY` is 100) and below tooltips (200).
    .with_priority(150)
}

fn settings_page(_: &mut App) -> Vec<SettingsSection> {
    vec![
        SettingsSection::new()
            .with_title("Server")
            .with_description(
                "Run notia-sync-server on a computer you control, then put its address and token here. Notes, images, and every other file sync both ways. A file edited on two computers keeps both copies.",
            )
            .settings([
                Setting::text(
                    "Address",
                    |cx| SyncSettings::load(cx).server_url.into(),
                    |value, cx| edit(cx, |settings| settings.server_url = value.to_string()),
                )
                .with_description("Such as http://192.168.1.20:8787."),
                Setting::secret(
                    "Token",
                    |cx| SyncSettings::load(cx).token.into(),
                    |value, cx| edit(cx, |settings| settings.token = value.to_string()),
                )
                .with_description(
                    "The secret the server prints. The field hides it until you show it. It stays on this computer, and anyone with it can read and change the vault on the server.",
                ),
                Setting::text(
                    "Vault id",
                    |cx| SyncSettings::load(cx).vault_id.into(),
                    |value, cx| edit(cx, |settings| settings.vault_id = value.to_string()),
                )
                .with_description(
                    "Computers sync together when they use the same id. Leave this empty to use the vault folder's name.",
                ),
                Setting::custom("Connection", |_, _| {
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("sync-now")
                                .outline()
                                .label("Sync now")
                                .on_click(|_, window, cx| request_sync(window, cx)),
                        )
                        .child(
                            Button::new("sync-test")
                                .outline()
                                .label("Test connection")
                                .on_click(|_, window, cx| test_connection(window, cx)),
                        )
                        .into_any_element()
                }),
            ]),
        SettingsSection::new()
            .with_title("Files")
            .setting(
                Setting::toggle(
                    "Include config files",
                    |cx| SyncSettings::load(cx).include_config,
                    |value, cx| edit(cx, |settings| settings.include_config = value),
                )
                .with_description(
                    "Also sync settings, appearance, layout, themes, and plugin data in .notia. Community plugins, this token, and the trash stay on this computer. Some settings apply the next time Notia opens.",
                ),
            ),
        SettingsSection::new()
            .with_title("When")
            .settings([
                Setting::toggle(
                    "Sync automatically",
                    |cx| SyncSettings::load(cx).auto,
                    |value, cx| {
                        edit(cx, |settings| settings.auto = value);
                        if value {
                            if let Some(host) = cx.try_global::<SyncHost>() {
                                let controller = host.controller.clone();
                                controller.update(cx, |this, cx| this.start(None, cx));
                            }
                        }
                    },
                )
                .with_description(
                    "Sync a few seconds after files change, and on the interval below, while Notia is open.",
                ),
                Setting::toggle(
                    "Real-time sync",
                    |cx| SyncSettings::load(cx).real_time,
                    |value, cx| {
                        edit(cx, |settings| settings.real_time = value);
                        if value {
                            if let Some(host) = cx.try_global::<SyncHost>() {
                                let controller = host.controller.clone();
                                controller.update(cx, |this, cx| {
                                    this.retry_after = None;
                                    this.start(None, cx);
                                });
                            }
                        }
                    },
                )
                .with_description(
                    "Sync as soon as a file is saved, and check the server every second for changes from other computers.",
                ),
                Setting::number(
                    "Interval",
                    1.0..=86_400.0,
                    1.0,
                    |cx| SyncSettings::load(cx).interval_seconds as f64,
                    |seconds, cx| {
                        edit(cx, |settings| {
                            settings.interval_seconds = interval_seconds(seconds);
                        });
                    },
                )
                .with_description(
                    "Seconds between syncs. Real-time sync checks the server every second instead.",
                ),
            ]),
    ]
}

fn edit(cx: &mut App, change: impl FnOnce(&mut SyncSettings)) {
    let mut settings = SyncSettings::load(cx);
    let connection = settings.connection();
    change(&mut settings);
    settings.interval_seconds = settings.interval_seconds.clamp(1, 86_400);
    let reset = connection != settings.connection();
    Notia::save_plugin_data(PLUGIN_ID, &settings, cx);
    if let Some(host) = cx.try_global::<SyncHost>() {
        let controller = host.controller.clone();
        controller.update(cx, |this, cx| {
            if reset {
                this.seen = None;
                this.retry_after = None;
                this.poll_epoch = this.poll_epoch.wrapping_add(1);
            }
            this.on_settings(cx);
        });
    }
}

#[derive(Clone, Serialize)]
struct SyncSettings {
    #[serde(default)]
    server_url: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    vault_id: String,
    #[serde(default = "default_auto")]
    auto: bool,
    #[serde(default)]
    real_time: bool,
    interval_seconds: u64,
    #[serde(default)]
    include_config: bool,
}

fn default_auto() -> bool {
    true
}

fn default_interval() -> u64 {
    30
}

/// Keeps a typed value between 1 second and one day.
fn interval_seconds(seconds: f64) -> u64 {
    if seconds.is_finite() {
        (seconds.round() as u64).clamp(1, 86_400)
    } else {
        default_interval()
    }
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            token: String::new(),
            vault_id: String::new(),
            auto: default_auto(),
            real_time: false,
            interval_seconds: default_interval(),
            include_config: false,
        }
    }
}

impl<'de> Deserialize<'de> for SyncSettings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stored {
            #[serde(default)]
            server_url: String,
            #[serde(default)]
            token: String,
            #[serde(default)]
            vault_id: String,
            #[serde(default = "default_auto")]
            auto: bool,
            #[serde(default)]
            real_time: bool,
            interval_seconds: Option<u64>,
            /// Older settings stored the interval in minutes.
            interval_minutes: Option<u64>,
            #[serde(default)]
            include_config: bool,
        }
        let stored = Stored::deserialize(deserializer)?;
        let interval_seconds = stored
            .interval_seconds
            .or_else(|| {
                stored
                    .interval_minutes
                    .map(|minutes| minutes.saturating_mul(60))
            })
            .unwrap_or_else(default_interval)
            .clamp(1, 86_400);
        Ok(Self {
            server_url: stored.server_url,
            token: stored.token,
            vault_id: stored.vault_id,
            auto: stored.auto,
            real_time: stored.real_time,
            interval_seconds,
            include_config: stored.include_config,
        })
    }
}

impl SyncSettings {
    fn load(cx: &App) -> Self {
        Notia::load_plugin_data(PLUGIN_ID, cx)
    }

    fn configured(&self) -> bool {
        !self.server_url.trim().is_empty() && !self.token.trim().is_empty()
    }

    /// The fields that choose which vault on which server this computer talks to.
    fn connection(&self) -> (String, String, String, bool) {
        (
            self.server_url.trim().to_string(),
            self.token.trim().to_string(),
            self.vault_id.trim().to_string(),
            self.include_config,
        )
    }

    fn options(&self, cx: &App) -> Option<SyncOptions> {
        if !self.configured() {
            return None;
        }
        let vault = Notia::vault(cx).read(cx);
        let vault_id = if self.vault_id.trim().is_empty() {
            vault.name()
        } else {
            self.vault_id.trim().to_string()
        };
        Some(SyncOptions {
            root: vault.vault().root().to_path_buf(),
            server_url: self.server_url.trim().to_string(),
            token: self.token.trim().to_string(),
            vault_id,
            include_config: self.include_config,
            max_bytes: MAX_FILE_BYTES,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::SyncSettings;

    #[test]
    fn real_time_stays_off_until_it_is_chosen() {
        let existing: SyncSettings = serde_json::from_str(
            r#"{"server_url":"http://127.0.0.1:8787","token":"secret","interval_minutes":1}"#,
        )
        .unwrap();
        assert!(!existing.real_time);
        assert!(existing.auto);
        assert_eq!(existing.interval_seconds, 60);

        let chosen: SyncSettings =
            serde_json::from_str(r#"{"real_time":true,"interval_seconds":15}"#).unwrap();
        assert!(chosen.real_time);
        assert_eq!(chosen.interval_seconds, 15);
    }
}
