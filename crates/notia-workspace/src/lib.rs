//! The Notia window, laid out like Obsidian: a ribbon of plugin buttons,
//! a left and a right sidebar holding plugin panels, browser-like tabs of
//! open notes, a status bar of plugin items, a command palette, a quick
//! switcher and the settings dialog.

extern crate gpui_kit as gpui;

mod attachments;
mod note_editor;
mod palette;
mod reading;
mod settings_dialog;
mod workspace;

use std::rc::Rc;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use notia_plugin::{
    Notia, OpenOptions, PanelPlacement, PluginHost, ViewMode,
    actions::*,
    core::VaultPath,
    workspace::{WorkspaceHandle, set_handle},
};

pub use note_editor::NoteEditor;
pub use workspace::Workspace;

/// A binding whose `secondary` modifier is Command on macOS and Control
/// elsewhere.
pub fn key_binding(
    keystrokes: &str,
    action: impl Action,
    context: Option<&str>,
    cx: &App,
) -> KeyBinding {
    let context = context.map(|context| {
        Rc::new(KeyBindingContextPredicate::parse(context).expect("a valid key context"))
    });
    KeyBinding::load(keystrokes, Box::new(action), context, false, None, cx)
        .expect("a valid keybinding")
}

/// Binds the workspace keys and actions. Call once at startup.
pub fn init(cx: &mut App) {
    let context = Some(WORKSPACE_CONTEXT);
    let bindings = [
        key_binding("secondary-p", OpenCommandPalette, context, cx),
        key_binding("secondary-o", OpenQuickSwitcher, context, cx),
        key_binding("secondary-,", OpenSettings, context, cx),
        key_binding("secondary-n", NewNote, context, cx),
        key_binding("secondary-w", CloseTab, context, cx),
        key_binding("secondary-e", ToggleReadingView, context, cx),
        key_binding("secondary-s", SaveNote, context, cx),
        key_binding("ctrl-tab", NextTab, context, cx),
        key_binding("ctrl-shift-tab", PreviousTab, context, cx),
        key_binding("secondary-\\", ToggleLeftSidebar, context, cx),
        key_binding("secondary-shift-\\", ToggleRightSidebar, context, cx),
    ];
    cx.bind_keys(bindings);

    // Handled at the application level so that the palette, which lives in
    // a dialog outside the workspace's element tree, can dispatch them too.
    cx.on_action(|_: &OpenCommandPalette, cx| with_workspace(cx, Workspace::open_command_palette));
    cx.on_action(|_: &OpenQuickSwitcher, cx| with_workspace(cx, Workspace::open_quick_switcher));
    cx.on_action(|_: &NewNote, cx| {
        with_workspace(cx, |this, window, cx| this.new_note(None, window, cx))
    });
    cx.on_action(|_: &CloseTab, cx| with_workspace(cx, Workspace::close_active_tab));
    cx.on_action(|_: &NextTab, cx| {
        with_workspace(cx, |this, window, cx| this.cycle_tab(true, window, cx))
    });
    cx.on_action(|_: &PreviousTab, cx| {
        with_workspace(cx, |this, window, cx| this.cycle_tab(false, window, cx))
    });
    cx.on_action(|_: &EditMode, cx| {
        with_workspace(cx, |this, window, cx| {
            this.set_view_mode(ViewMode::Editing, window, cx)
        })
    });
    cx.on_action(|_: &HalfEditMode, cx| {
        with_workspace(cx, |this, window, cx| {
            this.set_view_mode(ViewMode::Half, window, cx)
        })
    });
    cx.on_action(|_: &PreviewMode, cx| {
        with_workspace(cx, |this, window, cx| {
            this.set_view_mode(ViewMode::Reading, window, cx)
        })
    });
    cx.on_action(|_: &ToggleReadingView, cx| with_workspace(cx, Workspace::toggle_reading_view));
    cx.on_action(|_: &SaveNote, cx| with_workspace(cx, |this, _, cx| this.save_active(cx)));
    cx.on_action(|_: &ToggleLeftSidebar, cx| {
        with_workspace(cx, |this, _, cx| {
            this.toggle_sidebar(PanelPlacement::Left, cx)
        })
    });
    cx.on_action(|_: &ToggleRightSidebar, cx| {
        with_workspace(cx, |this, _, cx| {
            this.toggle_sidebar(PanelPlacement::Right, cx)
        })
    });
    cx.on_action(|_: &OpenSettings, cx| with_window(cx, settings_dialog::open_settings));
    cx.on_action(|action: &RunCommand, cx| {
        let id = action.id.clone();
        with_window(cx, move |window, cx| run_command(&id, window, cx));
    });
}

/// The options of a workspace window centered on the main display.
pub fn window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(720.), px(480.))),
        ..TitleBar::window_options()
    }
}

/// Opens the workspace window for the vault set up by `notia_plugin::init`.
pub fn open_window(options: WindowOptions, cx: &mut App) -> anyhow::Result<AnyWindowHandle> {
    let title: SharedString = Notia::vault(cx).read(cx).name().into();
    let (window, workspace) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Workspace::new(window, cx))
    })?;
    cx.set_global(WorkspaceSlot {
        window,
        workspace: workspace.downgrade(),
    });
    set_handle(Rc::new(Handle), cx);
    window.update(cx, |_, window, _| {
        window.activate_window();
        window.set_window_title(&title);
    })?;
    Ok(window)
}

struct WorkspaceSlot {
    window: AnyWindowHandle,
    workspace: WeakEntity<Workspace>,
}

impl Global for WorkspaceSlot {}

/// Runs `f` with the window once the current dispatch has finished.
fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    let Some(window) = cx.try_global::<WorkspaceSlot>().map(|slot| slot.window) else {
        return;
    };
    cx.defer(move |cx| {
        _ = window.update(cx, |_, window, cx| f(window, cx));
    });
}

/// Runs `f` on the workspace once the current dispatch has finished, so it
/// never re-enters an update already in progress.
fn with_workspace(
    cx: &mut App,
    f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) {
    let Some(workspace) = cx
        .try_global::<WorkspaceSlot>()
        .map(|slot| slot.workspace.clone())
    else {
        return;
    };
    with_window(cx, move |window, cx| {
        _ = workspace.update(cx, |this, cx| f(this, window, cx));
    });
}

fn run_command(id: &str, window: &mut Window, cx: &mut App) {
    match PluginHost::global(cx).command(id) {
        Some(command) => command.run(window, cx),
        None => log::warn!("no enabled plugin provides the command {id}"),
    }
}

/// What plugins call through `notia_plugin::workspace`.
struct Handle;

impl WorkspaceHandle for Handle {
    fn open_note(&self, path: VaultPath, options: OpenOptions, _: &mut Window, cx: &mut App) {
        with_workspace(cx, move |this, window, cx| {
            this.open_note(path, options, window, cx)
        });
    }

    fn open_link(
        &self,
        target: &str,
        from: Option<VaultPath>,
        new_tab: bool,
        _: &mut Window,
        cx: &mut App,
    ) {
        let target = target.to_string();
        with_workspace(cx, move |this, window, cx| {
            this.open_link(&target, from, new_tab, window, cx)
        });
    }

    fn reveal_line(&self, line: usize, _: &mut Window, cx: &mut App) {
        with_workspace(cx, move |this, window, cx| {
            this.reveal_line(line, window, cx)
        });
    }

    fn insert_text(&self, text: &str, _: &mut Window, cx: &mut App) {
        let text = text.to_string();
        with_workspace(cx, move |this, window, cx| {
            this.insert_text(&text, window, cx)
        });
    }

    fn selection(&self, cx: &App) -> notia_plugin::workspace::SelectionText {
        cx.try_global::<WorkspaceSlot>()
            .and_then(|slot| slot.workspace.upgrade())
            .map(|workspace| workspace.read(cx).selection(cx))
            .unwrap_or_default()
    }

    fn replace_selection(
        &self,
        text: &str,
        expand_before: usize,
        expand_after: usize,
        _: &mut Window,
        cx: &mut App,
    ) {
        let text = text.to_string();
        with_workspace(cx, move |this, window, cx| {
            this.replace_selection(&text, expand_before, expand_after, window, cx)
        });
    }

    fn undo(&self, _: &mut Window, cx: &mut App) {
        with_workspace(cx, |this, window, cx| this.undo(window, cx));
    }

    fn redo(&self, _: &mut Window, cx: &mut App) {
        with_workspace(cx, |this, window, cx| this.redo(window, cx));
    }

    fn new_note(&self, folder: Option<VaultPath>, _: &mut Window, cx: &mut App) {
        with_workspace(cx, move |this, window, cx| {
            this.new_note(folder, window, cx)
        });
    }

    fn show_panel(&self, panel_id: &str, _: &mut Window, cx: &mut App) {
        let panel_id = panel_id.to_string();
        with_workspace(cx, move |this, _, cx| this.show_panel(&panel_id, cx));
    }

    fn run_command(&self, command_id: &str, _: &mut Window, cx: &mut App) {
        let command_id = command_id.to_string();
        with_window(cx, move |window, cx| run_command(&command_id, window, cx));
    }
}
