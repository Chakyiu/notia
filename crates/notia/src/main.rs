//! Notia, a plugin-based note taking app.
//!
//! Usage: `notia [VAULT_FOLDER]`. A folder on the command line opens that
//! vault. Otherwise Notia opens the vault chosen last time, or, on the first
//! start, asks where the vault should live.

use std::path::PathBuf;

fn vault_argument() -> Option<PathBuf> {
    std::env::args_os().nth(1).map(PathBuf::from)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,notia=info"))
        .init();

    let argument = vault_argument();
    gpui_kit::application()
        .with_assets(notia::Assets)
        .run(move |cx| {
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

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
            cx.activate(true);
        });
}
