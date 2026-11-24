//! Checks GitHub for a newer Notia release and, when asked, installs it.
//!
//! The download has to be the asset this repository published for this
//! computer. The installer then replaces the running copy and restarts.
//! A build from `cargo run` is left alone.

use std::{
    fs, io,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    setting::{SettingField, SettingItem},
};
use gpui_kit::{prelude::FluentBuilder as _, *};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const MAX_RELEASE_BYTES: u64 = 256 * 1024;
const MAX_UPDATE_BYTES: u64 = 128 * 1024 * 1024;

pub(crate) fn on_launch(cx: &mut App) {
    if gpui_kit::is_mobile() {
        return;
    }
    // Register the controller so manual checks work; do not hit GitHub on cold start.
    let _ = controller(cx);
}

pub(crate) fn items() -> Vec<SettingItem> {
    let mut items = vec![check_item()];
    if host_platform().is_some() {
        items.push(auto_item());
    }
    items
}

fn check_item() -> SettingItem {
    SettingItem::new(
        "Updates",
        SettingField::render(|options, window, cx| {
            let controller = controller(cx);
            let watched = controller.clone();
            // The settings page redraws when this state notifies. The state
            // notifies when the shared update controller changes, including a
            // check that started before About was opened.
            let _watch = window.use_keyed_state("notia-update-watch", cx, move |_, cx| {
                UpdateWatch::new(watched, cx)
            });
            let phase = controller.read(cx).phase.clone();
            let busy = phase.is_busy();
            h_flex()
                .gap_2()
                .items_center()
                .flex_wrap()
                .when_some(phase.status(), |row, message| {
                    let color = if matches!(phase, UpdatePhase::Available { .. }) {
                        cx.theme().foreground
                    } else {
                        cx.theme().muted_foreground
                    };
                    row.child(div().text_sm().text_color(color).child(message))
                })
                .when_some(phase.asset(), |row, asset| {
                    let controller = controller.clone();
                    row.child(
                        Button::new("install-update")
                            .outline()
                            .label("Update")
                            .with_size(options.size())
                            .disabled(busy)
                            .on_click(move |_, _, cx| {
                                let asset = asset.clone();
                                controller.update(cx, |this, cx| this.install_now(asset, cx));
                            }),
                    )
                })
                .when_some(phase.page(), |row, url| {
                    row.child(
                        Button::new("open-update")
                            .outline()
                            .icon(IconName::ExternalLink)
                            .label("Open release")
                            .with_size(options.size())
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                })
                .child(
                    Button::new("check-for-updates")
                        .outline()
                        .icon(IconName::RefreshCw)
                        .label(phase.button())
                        .with_size(options.size())
                        .disabled(busy)
                        .on_click({
                            let controller = controller.clone();
                            move |_, _, cx| {
                                controller.update(cx, |this, cx| this.start(false, cx));
                            }
                        }),
                )
        }),
    )
    .keywords(["update", "release", "upgrade"])
}

fn auto_item() -> SettingItem {
    SettingItem::new(
        "Update automatically",
        SettingField::switch(
            |_: &App| auto_update_enabled(),
            |on, cx: &mut App| {
                set_auto_update(on);
                if on {
                    controller(cx).update(cx, |this, cx| this.start(true, cx));
                }
            },
        ),
    )
    .description("When a newer release is published, download it, replace this copy, and restart.")
    .keywords(["update", "upgrade"])
}

struct UpdateWatch {
    _observe: Subscription,
}

impl UpdateWatch {
    fn new(controller: Entity<UpdateController>, cx: &mut Context<Self>) -> Self {
        Self {
            _observe: cx.observe(&controller, |_, _, cx| cx.notify()),
        }
    }
}

struct UpdateHost {
    controller: Entity<UpdateController>,
}

impl Global for UpdateHost {}

fn controller(cx: &mut App) -> Entity<UpdateController> {
    if let Some(host) = cx.try_global::<UpdateHost>() {
        return host.controller.clone();
    }
    let controller = cx.new(UpdateController::new);
    cx.set_global(UpdateHost {
        controller: controller.clone(),
    });
    controller
}

struct UpdateController {
    phase: UpdatePhase,
    task: Option<Task<()>>,
}

impl UpdateController {
    fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            phase: UpdatePhase::Idle,
            task: None,
        }
    }

    /// `automatic` is a check Notia started on its own. A failure to replace a
    /// development build stays in the log instead of a dialog.
    fn start(&mut self, automatic: bool, cx: &mut Context<Self>) {
        if self.phase.is_busy() {
            return;
        }
        self.phase = UpdatePhase::Checking;
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            let phase = cx
                .background_executor()
                .spawn(async { fetch_latest_release() })
                .await;
            let job = this.update(cx, |this, cx| {
                let install = automatic || auto_update_enabled();
                match phase {
                    UpdatePhase::Available {
                        version,
                        url,
                        asset: Some(asset),
                    } if install => {
                        if let Err(error) = install_target() {
                            log::info!("{error}");
                            this.phase = if automatic {
                                UpdatePhase::Available {
                                    version,
                                    url,
                                    asset: Some(asset),
                                }
                            } else {
                                UpdatePhase::Failed(error)
                            };
                            cx.notify();
                            return None;
                        }
                        this.phase = UpdatePhase::Downloading {
                            version: version.clone(),
                        };
                        cx.notify();
                        if automatic {
                            announce(format!("Updating to {version}. Notia will restart."), cx);
                        }
                        Some((version, asset))
                    }
                    other => {
                        this.phase = other;
                        cx.notify();
                        None
                    }
                }
            });
            let Ok(Some((version, asset))) = job else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { install_release(&asset) })
                .await;
            let _ = this.update(cx, |this, cx| finish_install(this, version, result, cx));
        }));
    }

    fn install_now(&mut self, asset: ReleaseAsset, cx: &mut Context<Self>) {
        if self.phase.is_busy() {
            return;
        }
        let Some(version) = self.phase.version().map(str::to_string) else {
            return;
        };
        if let Err(error) = install_target() {
            self.phase = UpdatePhase::Failed(error);
            cx.notify();
            return;
        }
        self.phase = UpdatePhase::Downloading {
            version: version.clone(),
        };
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { install_release(&asset) })
                .await;
            let _ = this.update(cx, |this, cx| finish_install(this, version, result, cx));
        }));
    }
}

fn finish_install(
    this: &mut UpdateController,
    version: String,
    result: Result<(), String>,
    cx: &mut Context<UpdateController>,
) {
    match result {
        Ok(()) => {
            this.phase = UpdatePhase::Installing { version };
            crate::persist_workspace(cx);
            cx.notify();
            cx.quit();
        }
        Err(error) => {
            log::warn!("could not install the update: {error}");
            this.phase = UpdatePhase::Failed(error);
            cx.notify();
        }
    }
}

fn announce(message: String, cx: &mut App) {
    let Some(window) = cx.active_window() else {
        log::info!("{message}");
        return;
    };
    if let Err(error) = window.update(cx, |_, window, cx| window.push_notification(message, cx)) {
        log::info!("{error:#}");
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReleaseAsset {
    name: String,
    url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum UpdatePhase {
    Idle,
    Checking,
    Current,
    Available {
        version: String,
        url: String,
        asset: Option<ReleaseAsset>,
    },
    Downloading {
        version: String,
    },
    Installing {
        version: String,
    },
    Unpublished,
    Failed(String),
}

impl UpdatePhase {
    fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing { .. }
        )
    }

    fn version(&self) -> Option<&str> {
        match self {
            Self::Available { version, .. }
            | Self::Downloading { version }
            | Self::Installing { version } => Some(version),
            _ => None,
        }
    }

    fn asset(&self) -> Option<ReleaseAsset> {
        match self {
            Self::Available {
                asset: Some(asset), ..
            } => Some(asset.clone()),
            _ => None,
        }
    }

    fn page(&self) -> Option<String> {
        match self {
            Self::Available { url, .. } => Some(url.clone()),
            _ => None,
        }
    }

    fn status(&self) -> Option<String> {
        match self {
            Self::Idle | Self::Checking => None,
            Self::Current => Some("This is the latest release.".into()),
            Self::Available { version, .. } => Some(format!("Version {version} is available.")),
            Self::Downloading { version } => Some(format!("Downloading {version}…")),
            Self::Installing { version } => {
                Some(format!("Installing {version}. Notia will restart."))
            }
            Self::Unpublished => Some("No release has been published.".into()),
            Self::Failed(message) => Some(message.clone()),
        }
    }

    fn button(&self) -> &'static str {
        match self {
            Self::Idle => "Check for updates",
            Self::Checking => "Checking…",
            Self::Downloading { .. } | Self::Installing { .. } => "Updating…",
            Self::Failed(_) => "Try again",
            _ => "Check again",
        }
    }
}

struct Platform {
    os: &'static str,
    arch: &'static str,
    extension: &'static str,
}

fn host_platform() -> Option<Platform> {
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        return None;
    };
    if cfg!(target_os = "macos") {
        Some(Platform {
            os: "macos",
            arch,
            extension: "dmg",
        })
    } else if cfg!(target_os = "linux") {
        if std::env::var_os("APPIMAGE").is_some() {
            Some(Platform {
                os: "linux",
                arch,
                extension: "AppImage",
            })
        } else {
            Some(Platform {
                os: "linux",
                arch,
                extension: "tar.gz",
            })
        }
    } else if cfg!(target_os = "windows") {
        Some(Platform {
            os: "windows",
            arch,
            extension: "zip",
        })
    } else {
        None
    }
}

fn asset_file_name(version: &str, platform: &Platform) -> String {
    format!(
        "Notia-{version}-{}-{}.{}",
        platform.os, platform.arch, platform.extension
    )
}

fn repository() -> Option<(&'static str, &'static str)> {
    let path = repository_url().strip_prefix("https://github.com/")?;
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    if parts.next().is_some() || !is_github_name(owner) || !is_github_name(name) {
        return None;
    }
    Some((owner, name))
}

fn repository_url() -> &'static str {
    REPOSITORY.trim_end_matches('/')
}

fn is_github_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn github_api_latest() -> Option<String> {
    let (owner, name) = repository()?;
    Some(format!(
        "https://api.github.com/repos/{owner}/{name}/releases/latest"
    ))
}

/// A release page is opened only when it stays on this repository.
fn trusted_release_url(url: &str) -> String {
    let releases = format!("{}/releases", repository_url());
    if url == releases || url.starts_with(&format!("{releases}/")) {
        url.to_string()
    } else {
        releases
    }
}

fn trusted_asset_url(url: &str, version: &str, file_name: &str) -> bool {
    let Some((owner, name)) = repository() else {
        return false;
    };
    let expected =
        format!("https://github.com/{owner}/{name}/releases/download/v{version}/{file_name}");
    url == expected
}

fn trusted_download_host(url: &str) -> bool {
    let Some(host) = https_host(url) else {
        return false;
    };
    matches!(
        host,
        "github.com"
            | "objects.githubusercontent.com"
            | "release-assets.githubusercontent.com"
            | "github-releases.githubusercontent.com"
    )
}

fn https_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    if rest.starts_with('[') {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.split('@').next_back()?;
    let host = host.split(':').next()?;
    if host.is_empty() { None } else { Some(host) }
}

fn fetch_latest_release() -> UpdatePhase {
    let Some(endpoint) = github_api_latest() else {
        return UpdatePhase::Failed("Updates are published on GitHub.".into());
    };
    let response = match agent(Duration::from_secs(20))
        .get(&endpoint)
        .header("User-Agent", concat!("Notia/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
    {
        Ok(response) => response,
        Err(error) => {
            log::warn!("could not check for updates: {error}");
            return UpdatePhase::Failed("Could not reach GitHub.".into());
        }
    };
    let status = response.status().as_u16();
    // The API reply is JSON, not a redirect to the file.
    let mut response = response;
    match response
        .body_mut()
        .with_config()
        .limit(MAX_RELEASE_BYTES)
        .read_to_vec()
    {
        Ok(body) => interpret_release(status, &body, host_platform().as_ref()),
        Err(error) => {
            log::warn!("could not read the GitHub release: {error}");
            UpdatePhase::Failed("Could not reach GitHub.".into())
        }
    }
}

fn interpret_release(status: u16, body: &[u8], platform: Option<&Platform>) -> UpdatePhase {
    if status == 404 {
        return UpdatePhase::Unpublished;
    }
    if status == 403 || status == 429 {
        return UpdatePhase::Failed("GitHub is limiting update checks. Try again later.".into());
    }
    if status != 200 {
        return UpdatePhase::Failed("GitHub returned an unexpected reply.".into());
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return UpdatePhase::Failed("GitHub returned an unexpected reply.".into());
    };
    let Some(version) = value
        .get("tag_name")
        .and_then(|tag| tag.as_str())
        .and_then(display_version)
    else {
        return UpdatePhase::Failed("GitHub returned an unexpected reply.".into());
    };
    let url = trusted_release_url(
        value
            .get("html_url")
            .and_then(|url| url.as_str())
            .unwrap_or(""),
    );
    if !is_newer(&version, env!("CARGO_PKG_VERSION")) {
        return UpdatePhase::Current;
    }
    let asset = platform.and_then(|platform| choose_asset(&value, &version, platform));
    UpdatePhase::Available {
        version,
        url,
        asset,
    }
}

fn choose_asset(
    release: &serde_json::Value,
    version: &str,
    platform: &Platform,
) -> Option<ReleaseAsset> {
    let file_name = asset_file_name(version, platform);
    let assets = release.get("assets")?.as_array()?;
    for asset in assets {
        let Some(name) = asset.get("name").and_then(|name| name.as_str()) else {
            continue;
        };
        let Some(url) = asset
            .get("browser_download_url")
            .and_then(|url| url.as_str())
        else {
            continue;
        };
        if name == file_name && trusted_asset_url(url, version, &file_name) {
            return Some(ReleaseAsset {
                name: name.to_string(),
                url: url.to_string(),
            });
        }
    }
    None
}

fn display_version(tag: &str) -> Option<String> {
    let version = tag.trim().strip_prefix('v').unwrap_or(tag.trim());
    if version.is_empty()
        || version.len() > 40
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
        || numeric_version(version).is_none()
    {
        return None;
    }
    Some(version.to_string())
}

fn numeric_version(version: &str) -> Option<Vec<u64>> {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    if core.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for part in core.split('.') {
        if part.is_empty() {
            return None;
        }
        parts.push(part.parse().ok()?);
    }
    Some(parts)
}

fn is_newer(latest: &str, current: &str) -> bool {
    let Some(latest) = numeric_version(latest.trim().trim_start_matches('v')) else {
        return false;
    };
    let Some(current) = numeric_version(current.trim().trim_start_matches('v')) else {
        return false;
    };
    let len = latest.len().max(current.len());
    for ix in 0..len {
        let left = latest.get(ix).copied().unwrap_or(0);
        let right = current.get(ix).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into()
}

fn install_release(asset: &ReleaseAsset) -> Result<(), String> {
    let dest = install_target()?;
    let platform =
        host_platform().ok_or_else(|| "This computer has no installable build.".to_string())?;
    let version = version_of(asset);
    if version.is_empty()
        || asset.name != asset_file_name(version, &platform)
        || !trusted_asset_url(&asset.url, version, &asset.name)
    {
        return Err("The update is not a release of this app.".into());
    }
    let bytes = download(&asset.url)?;
    let temp = temp_dir()?;
    let staged = match unpack(&bytes, &platform, &temp) {
        Ok(path) => path,
        Err(error) => {
            let _ = fs::remove_dir_all(&temp);
            return Err(error);
        }
    };
    if let Err(error) = spawn_replacer(&dest, &staged, &temp) {
        let _ = fs::remove_dir_all(&temp);
        return Err(error);
    }
    Ok(())
}

fn version_of(asset: &ReleaseAsset) -> &str {
    asset
        .name
        .strip_prefix("Notia-")
        .and_then(|rest| rest.split('-').next())
        .unwrap_or("")
}

fn download(start: &str) -> Result<Vec<u8>, String> {
    if !trusted_download_host(start) {
        return Err("The update is not a release of this app.".into());
    }
    let client = agent(Duration::from_secs(120));
    let mut url = start.to_string();
    for _ in 0..5 {
        let mut response = client
            .get(&url)
            .header("User-Agent", concat!("Notia/", env!("CARGO_PKG_VERSION")))
            .call()
            .map_err(|error| {
                log::warn!("could not download the update: {error}");
                "Could not download the update.".to_string()
            })?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            url = next_url(&url, location)
                .filter(|next| trusted_download_host(next))
                .ok_or_else(|| "The update is not a release of this app.".to_string())?;
            continue;
        }
        if status != 200 {
            log::warn!("the update download returned HTTP {status}");
            return Err("Could not download the update.".into());
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_UPDATE_BYTES)
            .read_to_vec()
            .map_err(|error| {
                log::warn!("could not read the update: {error}");
                "Could not download the update.".to_string()
            })?;
        if body.is_empty() {
            return Err("Could not download the update.".into());
        }
        if body.len() as u64 > MAX_UPDATE_BYTES {
            return Err("The update is larger than Notia will install.".into());
        }
        return Ok(body);
    }
    Err("Could not download the update.".into())
}

fn next_url(current: &str, location: &str) -> Option<String> {
    let location = location.trim();
    if location.starts_with("https://") {
        return Some(location.to_string());
    }
    if let Some(path) = location.strip_prefix('/') {
        let origin = current.split(['?', '#']).next()?;
        let origin = origin.strip_prefix("https://")?;
        let host = origin.split('/').next()?;
        return Some(format!("https://{host}/{path}"));
    }
    None
}

fn unpack(bytes: &[u8], platform: &Platform, temp: &Path) -> Result<PathBuf, String> {
    match platform.extension {
        "dmg" => unpack_dmg(bytes, temp),
        "zip" => unpack_named(bytes, ArchiveKind::Zip, "notia.exe", temp),
        "tar.gz" => unpack_named(bytes, ArchiveKind::TarGz, "notia", temp),
        "AppImage" => {
            let path = temp.join("Notia.AppImage");
            fs::write(&path, bytes)
                .map_err(|error| format!("could not store the download: {error}"))?;
            Ok(path)
        }
        _ => Err("This computer has no installable build.".into()),
    }
}

enum ArchiveKind {
    Zip,
    TarGz,
}

fn unpack_named(
    bytes: &[u8],
    kind: ArchiveKind,
    file_name: &str,
    temp: &Path,
) -> Result<PathBuf, String> {
    let payload = match kind {
        ArchiveKind::Zip => read_zip(bytes, file_name)?,
        ArchiveKind::TarGz => read_tar_gz(bytes, file_name)?,
    };
    let path = temp.join(file_name);
    fs::write(&path, payload).map_err(|error| format!("could not store the download: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o755));
    }
    Ok(path)
}

fn read_zip(bytes: &[u8], file_name: &str) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| "The download is not the Notia archive.".to_string())?;
    if zip.len() > 64 {
        return Err("The download is not the Notia archive.".into());
    }
    let mut found = None;
    for index in 0..zip.len() {
        let mut file = zip
            .by_index(index)
            .map_err(|_| "The download is not the Notia archive.".to_string())?;
        if !file.is_file() {
            continue;
        }
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            continue;
        }
        let Some(name) = file.enclosed_name() else {
            continue;
        };
        if !is_payload(&name, file_name) {
            continue;
        }
        if found.is_some() {
            return Err("The download is not the Notia archive.".into());
        }
        let mut payload = Vec::new();
        file.by_ref()
            .take(MAX_UPDATE_BYTES + 1)
            .read_to_end(&mut payload)
            .map_err(|_| "The download is not the Notia archive.".to_string())?;
        if payload.len() as u64 > MAX_UPDATE_BYTES || payload.is_empty() {
            return Err("The update is larger than Notia will install.".into());
        }
        found = Some(payload);
    }
    found.ok_or_else(|| "The download does not contain Notia.".to_string())
}

fn read_tar_gz(bytes: &[u8], file_name: &str) -> Result<Vec<u8>, String> {
    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|_| "The download is not the Notia archive.".to_string())?;
    let mut found = None;
    let mut count = 0usize;
    for entry in entries {
        count += 1;
        if count > 64 {
            return Err("The download is not the Notia archive.".into());
        }
        let entry = entry.map_err(|_| "The download is not the Notia archive.".to_string())?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let Ok(path) = entry.path() else {
            continue;
        };
        if !is_payload(path.as_ref(), file_name) {
            continue;
        }
        if found.is_some() {
            return Err("The download is not the Notia archive.".into());
        }
        if entry.size() > MAX_UPDATE_BYTES {
            return Err("The update is larger than Notia will install.".into());
        }
        let mut payload = Vec::new();
        entry
            .take(MAX_UPDATE_BYTES + 1)
            .read_to_end(&mut payload)
            .map_err(|_| "The download is not the Notia archive.".to_string())?;
        if payload.is_empty() || payload.len() as u64 > MAX_UPDATE_BYTES {
            return Err("The download does not contain Notia.".into());
        }
        found = Some(payload);
    }
    found.ok_or_else(|| "The download does not contain Notia.".to_string())
}

fn is_payload(path: &Path, file_name: &str) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    if name != file_name {
        return false;
    }
    let parts: Vec<_> = path.components().collect();
    !parts.is_empty()
        && parts.len() <= 3
        && parts
            .iter()
            .all(|part| matches!(part, Component::Normal(_)))
}

#[cfg(target_os = "macos")]
fn unpack_dmg(bytes: &[u8], temp: &Path) -> Result<PathBuf, String> {
    let dmg = temp.join("update.dmg");
    fs::write(&dmg, bytes).map_err(|error| format!("could not store the download: {error}"))?;
    let mount = temp.join("mnt");
    fs::create_dir(&mount).map_err(|error| format!("could not open the disk image: {error}"))?;
    let attached = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount)
        .arg(&dmg)
        .output()
        .map_err(|error| format!("could not open the disk image: {error}"))?;
    if !attached.status.success() {
        let detail = String::from_utf8_lossy(&attached.stderr);
        let detail = detail.trim();
        let message = if detail.is_empty() {
            "could not open the disk image".to_string()
        } else {
            format!("could not open the disk image: {detail}")
        };
        return Err(message);
    }
    let source = mount.join("Notia.app");
    let staged = temp.join("Notia.app");
    let copied = source.join("Contents/MacOS/notia").is_file()
        && Command::new("ditto")
            .arg(&source)
            .arg(&staged)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
    detach_dmg(&mount);
    let _ = fs::remove_file(&dmg);
    if copied && staged.join("Contents/MacOS/notia").is_file() {
        Ok(staged)
    } else {
        Err("The disk image does not contain Notia.".into())
    }
}

#[cfg(target_os = "macos")]
fn detach_dmg(mount: &Path) {
    let _ = Command::new("hdiutil")
        .args(["detach", "-quiet"])
        .arg(mount)
        .status();
}

#[cfg(not(target_os = "macos"))]
fn unpack_dmg(_bytes: &[u8], _temp: &Path) -> Result<PathBuf, String> {
    Err("This computer cannot open a macOS disk image.".into())
}

fn install_target() -> Result<PathBuf, String> {
    if let Some(appimage) = std::env::var_os("APPIMAGE").filter(|path| !path.is_empty()) {
        return writable_file(PathBuf::from(appimage));
    }
    let exe = std::env::current_exe()
        .map_err(|error| format!("could not find this copy of Notia: {error}"))?;
    if is_cargo_target(&exe) {
        return Err("This copy was built from source, so it was not replaced.".into());
    }
    if let Some(app) = mac_bundle(&exe) {
        return writable_dir(app);
    }
    writable_file(exe)
}

fn is_cargo_target(path: &Path) -> bool {
    let text = path.to_string_lossy().replace('\\', "/");
    text.contains("/target/debug/") || text.contains("/target/release/")
}

fn mac_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let app = contents.parent()?;
    if app.extension()? != "app" {
        return None;
    }
    Some(app.to_path_buf())
}

fn writable_dir(path: PathBuf) -> Result<PathBuf, String> {
    if can_write_dir(&path) {
        Ok(path)
    } else {
        Err("Notia cannot replace itself. Install the release by hand.".into())
    }
}

fn writable_file(path: PathBuf) -> Result<PathBuf, String> {
    match path.parent() {
        Some(parent) if can_write_dir(parent) => Ok(path),
        _ => Err("Notia cannot replace itself. Install the release by hand.".into()),
    }
}

fn can_write_dir(dir: &Path) -> bool {
    let probe = dir.join(format!(".notia-update-{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn temp_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("notia-update-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .map_err(|error| format!("could not prepare the update: {error}"))?;
    }
    fs::create_dir_all(&dir).map_err(|error| format!("could not prepare the update: {error}"))?;
    Ok(dir)
}

fn spawn_replacer(dest: &Path, staged: &Path, temp: &Path) -> Result<(), String> {
    let dest_text = script_path(dest)?;
    let staged_text = script_path(staged)?;
    let temp_text = script_path(temp)?;
    let pid = std::process::id();
    let app = dest.is_dir();
    // Beside the staging folder, so deleting that folder does not delete the
    // script while it is still running.
    let script_path = temp.with_extension(if cfg!(windows) { "ps1" } else { "sh" });
    let script = if cfg!(windows) {
        windows_script(pid, &dest_text, &staged_text, &temp_text)
    } else {
        unix_script(pid, &dest_text, &staged_text, &temp_text, app)
    };
    fs::write(&script_path, script)
        .map_err(|error| format!("could not prepare the update: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700));
    }
    spawn_detached(&script_path)
}

fn unix_script(pid: u32, dest: &str, staged: &str, temp: &str, app: bool) -> String {
    let replace = if app {
        format!(
            "ditto \"{staged}\" \"{dest}\"\nxattr -dr com.apple.quarantine \"{dest}\" || true\nopen \"{dest}\"\n"
        )
    } else {
        format!(
            "mv -f \"{staged}\" \"{dest}\"\nchmod 755 \"{dest}\"\n\"{dest}\" >/dev/null 2>&1 &\n"
        )
    };
    format!(
        "#!/bin/sh\nwhile kill -0 {pid} 2>/dev/null; do\n    sleep 0.2\ndone\n{replace}rm -rf \"{temp}\"\nrm -f \"$0\"\n"
    )
}

fn windows_script(pid: u32, dest: &str, staged: &str, temp: &str) -> String {
    format!(
        "$dest = '{dest}'\n$staged = '{staged}'\n$temp = '{temp}'\nwhile (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 200 }}\nMove-Item -LiteralPath $staged -Destination $dest -Force\nStart-Process -FilePath $dest\nRemove-Item -LiteralPath $temp -Recurse -Force\nRemove-Item -LiteralPath $PSCommandPath -Force\n"
    )
}

fn script_path(path: &Path) -> Result<String, String> {
    let Some(text) = path.to_str() else {
        return Err("Notia cannot replace itself from this path.".into());
    };
    let rejected = text.chars().any(|ch| {
        ch.is_control() || ch == '\'' || (!cfg!(windows) && matches!(ch, '"' | '$' | '`' | '\\'))
    });
    if text.is_empty() || rejected {
        return Err("Notia cannot replace itself from this path.".into());
    }
    Ok(text.to_string())
}

fn spawn_detached(script: &Path) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(script);
        command
    } else {
        let mut command = Command::new("/bin/sh");
        command.arg(script);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0000_0200 | 0x0800_0000);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not start the installer: {error}"))
}

fn auto_update_enabled() -> bool {
    flag_in(&config_file())
}

fn set_auto_update(enabled: bool) {
    if let Err(error) = store_flag(&config_file(), enabled) {
        log::warn!("could not save the update setting: {error}");
    }
}

fn config_file() -> PathBuf {
    app_config_dir().join("config.json")
}

/// The same folder `notia` uses to remember which vault to open.
fn app_config_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let Some(home) = home else {
        return PathBuf::from(".notia");
    };
    if cfg!(any(target_os = "macos", target_os = "ios")) {
        home.join("Library/Application Support/Notia")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or(home)
            .join("Notia")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("notia")
    }
}

fn flag_in(file: &Path) -> bool {
    read_config(file)
        .and_then(|value| value.get("auto_update").and_then(|flag| flag.as_bool()))
        .unwrap_or(false)
}

fn store_flag(file: &Path, enabled: bool) -> io::Result<()> {
    let Some(mut value) = read_config(file) else {
        return Err(io::Error::other(
            "the app settings could not be read, so the update setting was left unchanged",
        ));
    };
    value["auto_update"] = serde_json::Value::Bool(enabled);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(&value).map_err(io::Error::other)?;
    fs::write(file, json)
}

/// `None` when the file exists and is not a JSON object, so a later write
/// cannot wipe the remembered vault.
fn read_config(file: &Path) -> Option<serde_json::Value> {
    match fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text)
            .ok()
            .filter(|value: &serde_json::Value| value.is_object()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(serde_json::json!({})),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Platform, UpdatePhase, asset_file_name, display_version, flag_in, interpret_release,
        is_cargo_target, is_newer, is_payload, read_tar_gz, read_zip, script_path, store_flag,
        trusted_asset_url, trusted_download_host,
    };
    use std::{
        io::{Cursor, Write},
        path::Path,
    };

    fn platform(os: &'static str, arch: &'static str, extension: &'static str) -> Platform {
        Platform {
            os,
            arch,
            extension,
        }
    }

    #[test]
    fn version_order() {
        assert!(is_newer("0.2.4", "0.2.3"));
        assert!(is_newer("v0.10.0", "0.2.3"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.2", "0.1.9"));
        assert!(!is_newer("0.2.3", "0.2.3"));
        assert!(!is_newer("v0.2.3", "0.2.3"));
        assert!(!is_newer("0.2.2", "0.2.3"));
        assert!(!is_newer("0.2.3-beta", "0.2.3"));
        assert!(!is_newer("0.2", "0.2.0"));
        assert_eq!(display_version("v9.0.0").as_deref(), Some("9.0.0"));
        assert!(display_version("not-a-version").is_none());
    }

    #[test]
    fn release_response_picks_this_computers_asset() {
        let body = br#"{
            "tag_name":"v9.0.0",
            "html_url":"https://github.com/Chakyiu/notia/releases/tag/v9.0.0",
            "assets":[
                {"name":"Notia-9.0.0-macos-aarch64.dmg","browser_download_url":"https://github.com/Chakyiu/notia/releases/download/v9.0.0/Notia-9.0.0-macos-aarch64.dmg"},
                {"name":"Notia-9.0.0-linux-x86_64.tar.gz","browser_download_url":"https://example.com/Notia-9.0.0-linux-x86_64.tar.gz"},
                {"name":"Notia-9.0.0-windows-x86_64.zip","browser_download_url":"https://github.com/Chakyiu/notia/releases/download/v9.0.0/Notia-9.0.0-windows-x86_64.zip"}
            ]
        }"#;
        let macos = platform("macos", "aarch64", "dmg");
        match interpret_release(200, body, Some(&macos)) {
            UpdatePhase::Available {
                version,
                url,
                asset: Some(asset),
            } => {
                assert_eq!(version, "9.0.0");
                assert_eq!(url, "https://github.com/Chakyiu/notia/releases/tag/v9.0.0");
                assert_eq!(asset.name, "Notia-9.0.0-macos-aarch64.dmg");
            }
            other => panic!("expected the macOS asset, got {other:?}"),
        }
        let linux = platform("linux", "x86_64", "tar.gz");
        match interpret_release(200, body, Some(&linux)) {
            UpdatePhase::Available { asset: None, .. } => {}
            other => panic!("a foreign download url has to be ignored, got {other:?}"),
        }
        assert!(matches!(
            interpret_release(
                200,
                br#"{"tag_name":"v0.0.1","html_url":"https://github.com/Chakyiu/notia/releases/tag/v0.0.1"}"#,
                None
            ),
            UpdatePhase::Current
        ));
        assert_eq!(interpret_release(404, b"", None), UpdatePhase::Unpublished);
        assert!(trusted_asset_url(
            "https://github.com/Chakyiu/notia/releases/download/v9.0.0/Notia-9.0.0-macos-aarch64.dmg",
            "9.0.0",
            "Notia-9.0.0-macos-aarch64.dmg"
        ));
        assert!(!trusted_asset_url(
            "https://github.com/Chakyiu/notia/releases/download/v9.0.0/other.dmg",
            "9.0.0",
            "Notia-9.0.0-macos-aarch64.dmg"
        ));
        assert!(trusted_download_host(
            "https://objects.githubusercontent.com/github-production-release-asset/abc"
        ));
        assert!(!trusted_download_host("https://example.com/notia.dmg"));
        assert!(!trusted_download_host(
            "http://github.com/Chakyiu/notia/releases/download/v9.0.0/a.dmg"
        ));
    }

    #[test]
    fn archives_yield_the_binary_and_skip_paths_that_escape() {
        assert!(is_payload(
            Path::new("Notia-9.0.0-linux-x86_64/notia"),
            "notia"
        ));
        assert!(!is_payload(Path::new("../notia"), "notia"));
        assert!(!is_payload(Path::new("/tmp/notia"), "notia"));

        let mut zip_bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut zip_bytes);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("../notia.exe", options).unwrap();
            writer.write_all(b"escape").unwrap();
            writer
                .start_file("Notia-9.0.0-windows-x86_64/notia.exe", options)
                .unwrap();
            writer.write_all(b"binary").unwrap();
            writer.finish().unwrap();
        }
        assert_eq!(
            read_zip(zip_bytes.get_ref(), "notia.exe").unwrap(),
            b"binary"
        );

        let mut raw = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut raw);
            let mut header = tar::Header::new_gnu();
            header.set_path("Notia-9.0.0-linux-aarch64/notia").unwrap();
            header.set_size(4);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, &b"note"[..]).unwrap();
            let mut other = tar::Header::new_gnu();
            other
                .set_path("Notia-9.0.0-linux-aarch64/readme.txt")
                .unwrap();
            other.set_size(3);
            other.set_cksum();
            builder.append(&other, &b"bad"[..]).unwrap();
            builder.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&raw).unwrap();
        let compressed = gz.finish().unwrap();
        assert_eq!(read_tar_gz(&compressed, "notia").unwrap(), b"note");
    }

    #[test]
    fn source_builds_and_quoted_paths_are_not_replaced() {
        assert!(is_cargo_target(Path::new("/work/notia/target/debug/notia")));
        assert!(is_cargo_target(Path::new(
            r"C:\work\notia\target\release\notia.exe"
        )));
        assert!(!is_cargo_target(Path::new(
            "/Applications/Notia.app/Contents/MacOS/notia"
        )));
        assert!(script_path(Path::new("/Applications/Notia.app")).is_ok());
        assert!(script_path(Path::new("/tmp/notia update/notia")).is_ok());
        assert!(script_path(Path::new("/tmp/notia';rm")).is_err());
        assert_eq!(
            asset_file_name("9.0.0", &platform("windows", "x86_64", "zip")),
            "Notia-9.0.0-windows-x86_64.zip"
        );
    }

    #[test]
    fn the_automatic_flag_is_stored_beside_the_remembered_vault() {
        let dir = std::env::temp_dir().join(format!(
            "notia-update-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        std::fs::write(&file, r#"{"vault":"/Notes","vaults":["/Notes"]}"#).unwrap();
        assert!(!flag_in(&file));
        store_flag(&file, true).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved["auto_update"], true);
        assert_eq!(saved["vault"], "/Notes");
        assert!(flag_in(&file));
        store_flag(&file, false).unwrap();
        assert!(!flag_in(&file));
        std::fs::write(&file, "not json").unwrap();
        assert!(store_flag(&file, true).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
