//! Notia, a plugin-based note taking app.
//!
//! Usage: `notia [VAULT_FOLDER]`. A folder on the command line opens that
//! vault. Otherwise Notia opens the vault chosen last time, or, on the first
//! start, asks where the vault should live.
//!
//! On iOS the vault lives in the app's Documents folder. There is no folder
//! picker: the system does not give an app the desktop's files.

use std::path::PathBuf;

fn vault_argument() -> Option<PathBuf> {
    std::env::args_os().nth(1).map(PathBuf::from)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,notia=info"))
        .init();

    application().with_assets(notia::Assets).run(launch);
}

#[cfg(target_os = "ios")]
fn application() -> gpui_kit::Application {
    gpui_kit::Application::with_platform(gpui_ios::current_platform(false))
}

#[cfg(not(target_os = "ios"))]
fn application() -> gpui_kit::Application {
    gpui_kit::application()
}

fn launch(cx: &mut gpui_kit::App) {
    let argument = vault_argument();
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();

    if gpui_kit::is_mobile() {
        let folder = notia::remembered_vault().unwrap_or_else(notia::mobile_vault_dir);
        if let Err(error) = notia::open_at(&folder, true, cx) {
            log::error!("{error}");
            notia::open_failure(error, cx);
        }
    } else {
        match argument {
            Some(folder) => {
                if let Err(error) = notia::open_at(&folder, true, cx) {
                    eprintln!("{error}");
                    cx.quit();
                }
            }
            None => match notia::remembered_vault() {
                Some(folder) => {
                    if let Err(error) = notia::open_at(&folder, true, cx) {
                        log::error!("{error}");
                        notia::open_setup(cx);
                    }
                }
                None => notia::open_setup(cx),
            },
        }
    }
    cx.activate(true);
}
