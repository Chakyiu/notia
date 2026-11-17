//! Renders Notia's main screens offscreen and saves them as PNGs, for
//! reviewing the interface and themes without opening a window.
//!
//! ```sh
//! cargo test -p notia --test screenshots
//! ```
//!
//! The images go to `$NOTIA_SCREENSHOTS`, or `target/screenshots`.

use std::{path::PathBuf, sync::Arc, time::Duration};

use gpui_kit::{
    Action, AnyWindowHandle, Bounds, HeadlessAppContext, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, WindowBounds, WindowOptions,
    component::{ThemeMode, WindowExt as _},
    point, px, size,
};
use notia_plugin::{
    Notia, OpenOptions,
    actions::{
        CloseTab, EditMode, HalfEditMode, NewNote, OpenCommandPalette, OpenQuickSwitcher,
        OpenSettings, PreviewMode, RunCommand, ToggleLeftSidebar, ToggleRightSidebar,
    },
    core::{Vault, VaultPath},
    theme::{ColorScheme, ThemeCatalog},
    workspace,
};

const WORD_GOAL: &str = r#"
import { Plugin, ui } from "notia";

export default class WordGoal extends Plugin {
  async onload() {
    this.settings = Object.assign(
      { goal: 500, unit: "words", status: true, label: "Today" },
      await this.loadData(),
    );
    this.addSettingTab({
      icon: "target",
      render: () => [
        ui.setting(
          "Daily goal",
          ui.numberInput(this.settings.goal, (goal) => this.save({ goal }), { min: 50, max: 10000, step: 50 }),
          { description: "How much to write each day." },
        ),
        ui.setting(
          "Count",
          ui.dropdown(this.settings.unit, [["words", "Words"], ["characters", "Characters"]], (unit) =>
            this.save({ unit }),
          ),
        ),
        ui.heading("Status bar"),
        ui.setting("Show progress", ui.toggle(this.settings.status, (status) => this.save({ status })), {
          description: "Shows how close today's notes are to the goal.",
        }),
        ui.setting("Label", ui.textInput(this.settings.label, (label) => this.save({ label }))),
        ui.setting("Progress", ui.button("Start over", () => {})),
      ],
    });
  }

  save(change) {
    Object.assign(this.settings, change);
    this.saveData(this.settings);
  }
}
"#;

fn main() {
    let out = std::env::var_os("NOTIA_SCREENSHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/screenshots")
        });
    std::fs::create_dir_all(&out).expect("create the screenshot folder");

    let folder = tempfile::tempdir().expect("create a vault folder");
    let vault = Vault::open(folder.path()).expect("open the vault");
    notia::welcome::create(&vault).expect("write the starter notes");
    // A community plugin whose manifest lacks an id, to show load errors.
    let unfinished = vault.config_dir().join("plugins/unfinished");
    std::fs::create_dir_all(&unfinished).expect("create a plugin folder");
    std::fs::write(
        unfinished.join("manifest.json"),
        r#"{ "name": "Unfinished" }"#,
    )
    .expect("write a manifest");
    // An enabled community plugin with a settings tab.
    let word_goal = vault.config_dir().join("plugins/word-goal");
    std::fs::create_dir_all(&word_goal).expect("create a plugin folder");
    std::fs::write(
        word_goal.join("manifest.json"),
        r#"{ "id": "word-goal", "name": "Word goal", "version": "1.0.0" }"#,
    )
    .expect("write a manifest");
    std::fs::write(word_goal.join("main.js"), WORD_GOAL).expect("write a plugin");
    std::fs::write(
        vault.config_dir().join("plugins.json"),
        r#"{ "word-goal": true }"#,
    )
    .expect("enable the plugin");

    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(notia::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| notia::init(vault, cx));
    let window = cx
        .update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1280.), px(800.)),
                })),
                focus: false,
                show: false,
                ..notia_workspace::window_options(cx)
            };
            notia_workspace::open_window(options, cx)
        })
        .expect("open the workspace window");

    let mut screens = Screens { cx, window, out };
    screens.appearance("notia", ColorScheme::Light);
    screens.save("01-editing");
    screens.dispatch(ToggleLeftSidebar);
    screens.dispatch(ToggleRightSidebar);
    screens.save("16-sidebars-closed");
    screens.dispatch(ToggleLeftSidebar);
    screens.dispatch(ToggleRightSidebar);
    screens.dispatch(HalfEditMode);
    screens.save("17-half-edit");
    screens.dispatch(PreviewMode);
    screens.save("02-reading");
    screens.appearance("notia", ColorScheme::Dark);
    screens.save("03-reading-dark");
    screens.dispatch(EditMode);
    screens.save("04-editing-dark");

    screens.dispatch(OpenCommandPalette);
    screens.save("05-command-palette");
    screens.close_dialogs();
    screens.dispatch(OpenQuickSwitcher);
    screens.save("06-quick-switcher");
    screens.close_dialogs();
    screens.dispatch(OpenSettings);
    screens.save("07-settings");
    screens.click(257., 341.);
    screens.save("14-plugin-settings");
    screens.click(298., 399.);
    screens.save("15-community-plugin-settings");
    screens.close_dialogs();
    screens.dispatch(OpenSettings);
    // Word goal is the only plugin settings page in this vault, under the divider.
    screens.click(269., 540.);
    screens.save("19-community-plugin-options");
    screens.close_dialogs();

    // A Canvas note: its boxes and connector, on a board of dots.
    std::fs::write(
        folder.path().join("Board.canvas"),
        r#"{"nodes":[{"id":"1","type":"text","x":-240,"y":-70,"width":190,"height":80,"text":"gateway"},
                    {"id":"2","type":"text","x":80,"y":70,"width":190,"height":80,"text":"IP Gateway"}],
             "edges":[{"id":"3","fromNode":"1","toNode":"2"}]}"#,
    )
    .expect("write a canvas");
    screens
        .cx
        .update_window(screens.window, |_, window, cx| {
            workspace::open_note(
                VaultPath::new("Board.canvas"),
                OpenOptions::new().in_new_tab(),
                window,
                cx,
            );
        })
        .expect("open the canvas");
    screens.save("20-canvas");
    let read_board = || {
        std::fs::read_to_string(folder.path().join("Board.canvas")).expect("read the board back")
    };

    // Pressing a box moves it, never pulls a connector.
    screens.drag((419., 319.), (520., 470.));
    let board = read_board();
    assert!(
        !board.contains("\"x\": -240.0"),
        "the box followed the mouse: {board}"
    );

    // A double-click on an empty part of the board offers a text box or a
    // note. Choosing the text box adds it.
    screens.click_at(660., 620., 2);
    screens.click(730., 640.);
    let board = read_board();
    assert_eq!(
        board.matches("\"id\"").count(),
        4,
        "the new box is on the board: {board}"
    );

    // Pulling from the dot on the new box's right edge joins it to a box.
    screens.drag((776., 620.), (800., 480.));
    let board = read_board();
    assert_eq!(
        board.matches("fromNode").count(),
        2,
        "a connector was pulled: {board}"
    );
    screens.save("21-canvas-edited");
    screens.dispatch(CloseTab);

    // A Kanban note: columns of cards.
    std::fs::write(
        folder.path().join("Tasks.kanban"),
        r#"{"columns":[
            {"id":"1","title":"To do","cards":[{"id":"4","text":"**Write the outline**\n- [ ] first pass"},{"id":"5","text":"Sketch the board"}]},
            {"id":"2","title":"Doing","cards":[{"id":"6","text":"Drag cards between columns\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11"}]},
            {"id":"3","title":"Done","cards":[{"id":"7","text":"Open the vault"}]}
        ]}"#,
    )
    .expect("write a kanban");
    screens
        .cx
        .update_window(screens.window, |_, window, cx| {
            workspace::open_note(
                VaultPath::new("Tasks.kanban"),
                OpenOptions::new().in_new_tab(),
                window,
                cx,
            );
        })
        .expect("open the kanban");
    screens.save("22-kanban");
    screens.click(923., 133.);
    screens.save("23-kanban-settings");
    let kanban =
        std::fs::read_to_string(folder.path().join("Tasks.kanban")).expect("read the kanban back");
    assert!(
        kanban.contains("Write the outline")
            && kanban.contains("Drag cards between columns")
            && kanban.contains('\n'),
        "opening the board kept its cards: {kanban}"
    );
    screens.dispatch(CloseTab);

    screens.cx.update(|cx| {
        Notia::workspace(cx).update(cx, |model, cx| {
            model.request_search("tag:#plugins".into(), cx)
        });
    });
    screens.save("08-tag-search");
    screens.show_panel("backlinks:backlinks");
    screens.save("09-backlinks");
    screens.dispatch(RunCommand {
        id: "theme-gallery:change-theme".into(),
    });
    screens.save("10-theme-picker");
    screens.close_dialogs();
    screens.dispatch(NewNote);
    screens.show_panel("file-explorer:files");
    screens.save("11-new-note");
    screens.dispatch(CloseTab);
    screens.dispatch(CloseTab);
    screens.save("12-no-open-note");

    for theme in ["nord", "rose-pine", "catppuccin", "paper", "midnight"] {
        for (scheme, mode) in [
            (ColorScheme::Light, ThemeMode::Light),
            (ColorScheme::Dark, ThemeMode::Dark),
        ] {
            let has_mode = screens.cx.update(|cx| {
                ThemeCatalog::global(cx)
                    .pack(theme)
                    .is_some_and(|pack| pack.has_mode(mode))
            });
            if has_mode {
                screens.appearance(theme, scheme);
                screens.save(&format!("theme-{theme}-{}", mode.name()));
            }
        }
    }
    println!("screenshots saved to {}", screens.out.display());
}

struct Screens {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    out: PathBuf,
}

impl Screens {
    /// Lets deferred work, timers and animations finish, then draws a frame.
    fn settle(&mut self) {
        for _ in 0..4 {
            self.cx.run_until_parked();
            self.cx.advance_clock(Duration::from_millis(500));
            self.cx
                .update_window(self.window, |_, window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                })
                .expect("render a frame");
        }
    }

    fn save(&mut self, name: &str) {
        self.settle();
        let image = self
            .cx
            .capture_screenshot(self.window)
            .expect("capture the window");
        image
            .save(self.out.join(format!("{name}.png")))
            .expect("save the screenshot");
    }

    fn dispatch(&mut self, action: impl Action) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_action(Box::new(action), cx)
            })
            .expect("dispatch the action");
        self.settle();
    }

    /// Clicks at a point of the window, in window coordinates.
    fn click(&mut self, x: f32, y: f32) {
        self.click_at(x, y, 1);
    }

    /// A click of `count` clicks at a point; two is a double-click.
    fn click_at(&mut self, x: f32, y: f32, count: usize) {
        let position = point(px(x), px(y));
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: count,
                        first_mouse: false,
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: count,
                    }),
                    cx,
                );
            })
            .expect("click");
        self.settle();
    }

    /// Presses at one point, moves to another with the button held, lets go.
    fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
        let steps = [
            point(px(from.0), px(from.1)),
            point(px((from.0 + to.0) / 2.), px((from.1 + to.1) / 2.)),
            point(px(to.0), px(to.1)),
        ];
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: steps[0],
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position: steps[0],
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                );
                for position in &steps[1..] {
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position: *position,
                            pressed_button: Some(MouseButton::Left),
                            modifiers: Modifiers::default(),
                        }),
                        cx,
                    );
                    window.refresh();
                    window.draw(cx).clear(cx);
                }
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position: *steps.last().unwrap(),
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    }),
                    cx,
                );
            })
            .expect("drag");
        self.settle();
    }

    fn show_panel(&mut self, id: &str) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                workspace::show_panel(id, window, cx)
            })
            .expect("show the panel");
        self.settle();
    }

    fn close_dialogs(&mut self) {
        self.cx
            .update_window(self.window, |_, window, cx| window.close_all_dialogs(cx))
            .expect("close the dialogs");
        self.settle();
    }

    fn appearance(&mut self, theme: &str, scheme: ColorScheme) {
        let theme = theme.to_string();
        self.cx.update(|cx| {
            ThemeCatalog::update_appearance(cx, |appearance| {
                appearance.theme = theme;
                appearance.scheme = scheme;
            })
        });
        self.settle();
    }
}
