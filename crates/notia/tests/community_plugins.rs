//! Runs community plugins in a headless Notia: loading them lazily, their
//! API, their failures and reloading them.

use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{
    AnyWindowHandle, Bounds, HeadlessAppContext, WindowBounds, WindowOptions, px, size,
};
use notia_plugin::{
    EditorSelection, EditorSnapshot, Notia, OpenOptions, PluginHost,
    core::{Vault, VaultPath},
    workspace,
};

const PROBE: &str = r#"
import { Plugin, ui } from "notia";
import { shout } from "./lib/util.js";

export default class Probe extends Plugin {
  async onload() {
    this.data = (await this.loadData()) ?? { modified: [], errors: [] };
    this.addCommand({
      id: "write",
      name: "Write a probe note",
      callback: () => this.app.vault.create("Probe", shout(`${this.manifest.name} sees ${this.app.vault.getNotes().length} notes`)),
    });
    this.addStatusBarItem().setText("probe ready");
    this.addPanel({ id: "probe", title: "Probe", render: () => [ui.heading("Probe"), "hello"] });
    this.app.vault.on("modify", (path) => {
      this.data.modified.push(path);
      this.saveData(this.data);
    });
    try {
      this.app.vault.read(".notia/plugins.json");
    } catch (error) {
      this.data.errors.push(error.message);
    }
    await this.saveData(this.data);
    setTimeout(() => {
      this.app.vault.append("Timer", "from a timer");
      this.addCommand({ id: "late", name: "Registered late", callback: () => {} });
    }, 50);
  }

  onunload() {
    this.app.vault.create("Unloaded", "bye");
  }
}
"#;

const COMMONJS: &str = r#"
const { Plugin } = require("notia");
module.exports = class extends Plugin {
  onload() {
    this.addCommand({ id: "first", name: "First", callback: () => this.app.vault.create("CommonJS", "first") });
  }
};
"#;

/// Edits the open note through the `Editor` API and records what it read.
const EDITING: &str = r#"
import { Plugin } from "notia";

export default class Editing extends Plugin {
  onload() {
    this.registerEvent(this.app.workspace.on("editor-change", (note) => {
      this.changed = note.text;
    }));
    this.addCommand({
      id: "edit",
      name: "Edit the note",
      editorCallback: (editor, note) => {
        editor.setSelection({ line: 0, ch: 0 }, { line: 0, ch: 5 });
        const selected = editor.getSelection();
        editor.replaceSelection(`**${selected}**`);
        const wrapped = editor.getValue();
        const cursor = editor.getCursor();
        editor.replaceRange("> ", { line: 0, ch: 0 });
        editor.transaction({
          changes: [{ from: { line: 1, ch: 0 }, to: { line: 1, ch: 3 }, text: "two" }],
          selection: { from: { line: 1, ch: 0 }, to: { line: 1, ch: 3 } },
        });
        let overlap = null;
        try {
          editor.transaction({
            changes: [
              { from: { line: 0, ch: 0 }, to: { line: 0, ch: 3 }, text: "" },
              { from: { line: 0, ch: 1 }, to: { line: 0, ch: 4 }, text: "" },
            ],
          });
        } catch (error) {
          overlap = error.message;
        }
        this.saveData({
          path: note.path,
          selected,
          wrapped,
          cursor,
          text: editor.getValue(),
          noteText: this.app.workspace.getActiveNote().text,
          selection: editor.getSelection(),
          line: editor.getLine(1),
          lines: editor.lineCount(),
          word: editor.wordAt({ line: 0, ch: 5 }),
          offset: editor.posToOffset({ line: 1, ch: 3 }),
          overlap,
        });
      },
    });
    this.addCommand({ id: "insert", name: "Insert", callback: () => this.app.workspace.insertText("😀") });
    this.addCommand({ id: "undo", name: "Undo", editorCallback: (editor) => editor.undo() });
    this.addCommand({ id: "redo", name: "Redo", editorCallback: (editor) => editor.redo() });
    this.addCommand({
      id: "more",
      name: "Edit some more",
      editorCallback: (editor) => {
        editor.setValue("alpha\r\nbeta\ngamma");
        const crlf = editor.getLine(0);
        editor.setLine(1, "BETA");
        editor.setCursor(2, 2);
        const cursor = editor.getCursor();
        const range = editor.getRange({ line: 0, ch: 1 }, { line: 1, ch: 2 });
        editor.transaction({ replaceSelection: "-" });
        const after = editor.getCursor();
        const end = editor.offsetToPos(editor.getValue().length);
        editor.setSelection(editor.offsetToPos(0), editor.offsetToPos(5));
        editor.replaceSelection("**alpha**");
        editor.focus();
        const selections = editor.listSelections();
        editor.setSelection({ line: 1, ch: 4 }, { line: 1, ch: 0 });
        this.saveData({
          crlf,
          cursor,
          range,
          after,
          end,
          last: editor.lastLine(),
          text: editor.getValue(),
          selections,
          reversed: ["from", "to", "anchor", "head"].map((which) => editor.getCursor(which)),
        });
      },
    });
    this.addCommand({
      id: "changed",
      name: "Save the last change",
      callback: () => this.saveData({ changed: this.changed }),
    });
  }
}
"#;

fn main() {
    env_logger::builder()
        .parse_env(env_logger::Env::default().default_filter_or("notia_js=debug"))
        .init();
    let folder = tempfile::tempdir().expect("create a vault folder");
    let root = folder.path();
    let vault = Vault::open(root).expect("open the vault");
    notia::welcome::create(&vault).expect("write the starter vault");

    let plugins = root.join(".notia/plugins");
    write_plugin(
        &plugins,
        "probe",
        &[
            ("main.js", PROBE),
            (
                "lib/util.js",
                "export const shout = (text) => text.toUpperCase();",
            ),
        ],
    );
    write_plugin(&plugins, "commonjs", &[("main.js", COMMONJS)]);
    write_plugin(&plugins, "editing", &[("main.js", EDITING)]);
    write_plugin(
        &plugins,
        "broken",
        &[("main.js", "export default class { onload( { }")],
    );
    write_plugin(
        &plugins,
        "slow",
        &[(
            "main.js",
            "export default class { onload() { for (;;) {} } }",
        )],
    );
    write_plugin(
        &plugins,
        "disabled",
        &[("main.js", "throw new Error('never loaded');")],
    );
    fs::create_dir_all(plugins.join("bad-manifest")).unwrap();
    fs::write(
        plugins.join("bad-manifest/manifest.json"),
        r#"{ "id": 42 }"#,
    )
    .unwrap();
    fs::write(
        root.join(".notia/plugins.json"),
        r#"{ "probe": true, "commonjs": true, "editing": true, "broken": true, "slow": true }"#,
    )
    .unwrap();

    let mut app = App::start(vault);

    // Every folder is listed; the enabled plugins are loaded.
    app.read(|host| {
        for id in [
            "probe",
            "commonjs",
            "broken",
            "slow",
            "disabled",
            "bad-manifest",
        ] {
            assert!(
                host.plugins().any(|(manifest, _)| manifest.id() == id),
                "{id} is listed"
            );
        }
        assert!(
            host.is_enabled("probe") && host.error("probe").is_none(),
            "probe loaded: {:?}",
            host.error("probe")
        );
        assert!(!host.is_enabled("disabled") && host.error("disabled").is_none());
        assert!(
            host.error("broken")
                .is_some_and(|error| error.contains("main.js")),
            "{:?}",
            host.error("broken")
        );
        assert!(
            host.error("slow")
                .is_some_and(|error| error.contains("Stopped after")),
            "{:?}",
            host.error("slow")
        );
        assert!(
            host.error("bad-manifest")
                .is_some_and(|error| error.contains("manifest.json is invalid"))
        );
    });

    // Commands, relative imports, and the vault API.
    app.run_command("probe:write");
    assert_eq!(read(root, "Probe.md"), "PROBE SEES 4 NOTES");
    app.run_command("commonjs:first");
    assert_eq!(read(root, "CommonJS.md"), "first");

    // Timers, and contributions registered after onload.
    app.advance(Duration::from_millis(100));
    assert_eq!(read(root, "Timer.md"), "from a timer");
    app.read(|host| assert!(host.command("probe:late").is_some()));

    // Events and data.json; hidden files are out of reach.
    let data: serde_json::Value = serde_json::from_str(&read(&plugins, "probe/data.json")).unwrap();
    let modified = data["modified"].to_string();
    assert!(
        modified.contains("Probe.md") && modified.contains("Timer.md"),
        "{modified}"
    );
    assert!(
        data["errors"][0].as_str().unwrap().contains("hidden files"),
        "{data}"
    );

    // Disabling unloads the plugin and removes what it registered.
    app.update(|cx| PluginHost::set_enabled("probe", false, cx));
    assert_eq!(read(root, "Unloaded.md"), "bye");
    app.read(|host| {
        assert!(host.command("probe:write").is_none() && host.command("probe:late").is_none())
    });

    // Changed code is reloaded.
    fs::write(
        plugins.join("commonjs/main.js"),
        COMMONJS.replace("first", "second"),
    )
    .unwrap();
    app.run_command("community-plugins:reload");
    app.read(|host| {
        assert!(host.command("commonjs:first").is_none());
        assert!(host.command("commonjs:second").is_some());
    });

    // The editor API: reads include the plugin's own edits, positions count
    // UTF-16 code units, and each edit is one undo step.
    let note = VaultPath::new("Editing.md");
    app.update(|cx| {
        Notia::vault(cx)
            .update(cx, |vault, cx| {
                vault.save_note(&note, "Héllo wörld 😀\none line\n", cx)
            })
            .expect("write a note to edit");
    });
    app.open(&note);
    app.run_command("editing:edit");
    let edited = "> **Héllo** wörld 😀\ntwo line\n";
    let seen: serde_json::Value =
        serde_json::from_str(&read(&plugins, "editing/data.json")).unwrap();
    assert_eq!(seen["path"], "Editing.md");
    assert_eq!(seen["selected"], "Héllo");
    assert_eq!(seen["wrapped"], "**Héllo** wörld 😀\none line\n");
    assert_eq!(seen["cursor"], serde_json::json!({ "line": 0, "ch": 9 }));
    assert_eq!(seen["text"], edited);
    assert_eq!(seen["noteText"], edited);
    assert_eq!(seen["selection"], "two");
    assert_eq!(seen["line"], "two line");
    assert_eq!(seen["lines"], 3);
    assert_eq!(
        seen["word"],
        serde_json::json!({ "from": { "line": 0, "ch": 4 }, "to": { "line": 0, "ch": 9 } })
    );
    assert_eq!(seen["offset"], 24);
    assert!(
        seen["overlap"]
            .as_str()
            .is_some_and(|error| error.contains("overlap")),
        "{seen}"
    );
    let editor = app.editor();
    assert_eq!(editor.text.as_ref(), edited);
    // Bytes: "> **Héllo** wörld 😀\n" is 25 long.
    assert_eq!(editor.selection, EditorSelection::new(25, 28));

    app.run_command("editing:insert");
    let inserted = "> **Héllo** wörld 😀\ntwo😀 line\n";
    assert_eq!(app.editor().text.as_ref(), inserted);
    app.run_command("editing:undo");
    assert_eq!(app.editor().text.as_ref(), edited);
    app.run_command("editing:redo");
    assert_eq!(app.editor().text.as_ref(), inserted);
    app.run_command("editing:undo");
    assert_eq!(app.editor().text.as_ref(), edited);
    app.run_command("editing:changed");
    let seen: serde_json::Value =
        serde_json::from_str(&read(&plugins, "editing/data.json")).unwrap();
    assert_eq!(seen["changed"], edited);
    app.advance(Duration::from_secs(2));
    assert_eq!(read(root, "Editing.md"), edited);

    app.run_command("editing:more");
    let more = "**alpha**\r\nBETA\nga-mma";
    let seen: serde_json::Value =
        serde_json::from_str(&read(&plugins, "editing/data.json")).unwrap();
    let pos = |line: u32, ch: u32| serde_json::json!({ "line": line, "ch": ch });
    assert_eq!(seen["crlf"], "alpha");
    assert_eq!(seen["cursor"], pos(2, 2));
    assert_eq!(seen["range"], "lpha\r\nBE");
    assert_eq!(seen["after"], pos(2, 3));
    assert_eq!(seen["end"], pos(2, 6));
    assert_eq!(seen["last"], 2);
    assert_eq!(seen["text"], more);
    assert_eq!(
        seen["selections"],
        serde_json::json!([{ "anchor": pos(0, 9), "head": pos(0, 9) }])
    );
    assert_eq!(
        seen["reversed"],
        serde_json::json!([pos(1, 0), pos(1, 4), pos(1, 4), pos(1, 0)])
    );
    let editor = app.editor();
    assert_eq!(editor.text.as_ref(), more);
    // Bytes: "**alpha**\r\n" is 11 long.
    assert_eq!(editor.selection, EditorSelection::new(15, 11));

    // A plugin folder linked from outside the vault reloads when it changes.
    #[cfg(unix)]
    {
        let elsewhere = tempfile::tempdir().expect("create a folder outside the vault");
        let linked = |name: &str| {
            format!(
                r#"import {{ Plugin }} from "notia";
export default class extends Plugin {{
  onload() {{ this.addCommand({{ id: "{name}", name: "{name}", callback: () => {{}} }}); }}
}}"#
            )
        };
        write_plugin(elsewhere.path(), "linked", &[("main.js", &linked("first"))]);
        std::os::unix::fs::symlink(elsewhere.path().join("linked"), plugins.join("linked"))
            .unwrap();
        assert!(app.wait_until(|host| {
            host.plugins()
                .any(|(manifest, _)| manifest.id() == "linked")
        }));
        app.update(|cx| PluginHost::set_enabled("linked", true, cx));
        app.read(|host| assert!(host.command("linked:first").is_some()));
        // Let the rest of the events about the new link pass, so that only
        // the change below can reload the plugin.
        app.idle(Duration::from_secs(2));
        fs::write(elsewhere.path().join("linked/main.js"), linked("second")).unwrap();
        assert!(
            app.wait_until(|host| host.command("linked:second").is_some()),
            "saving a linked plugin did not reload it"
        );
    }

    // Turning off the loader removes every community plugin.
    app.update(|cx| PluginHost::set_enabled("community-plugins", false, cx));
    app.read(|host| {
        assert!(
            !host
                .plugins()
                .any(|(manifest, _)| manifest.id() == "commonjs")
        );
        assert!(host.command("commonjs:second").is_none());
    });
    println!("community plugins work");
}

fn write_plugin(plugins: &Path, id: &str, files: &[(&str, &str)]) {
    let dir = plugins.join(id);
    fs::create_dir_all(dir.join("lib")).unwrap();
    let manifest = format!(r#"{{ "id": "{id}", "name": "{id}", "version": "0.1.0" }}"#);
    fs::write(dir.join("manifest.json"), manifest).unwrap();
    for (name, text) in files {
        fs::write(dir.join(name), text).unwrap();
    }
}

fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name)).unwrap_or_else(|error| panic!("read {name}: {error}"))
}

struct App {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
}

impl App {
    fn start(vault: Vault) -> Self {
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
        let mut app = Self { cx, window };
        app.settle();
        app
    }

    fn settle(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .expect("render a frame");
        self.cx.run_until_parked();
    }

    fn advance(&mut self, duration: Duration) {
        self.cx.advance_clock(duration);
        self.settle();
    }

    fn update(&mut self, f: impl FnOnce(&mut gpui_kit::App)) {
        self.cx.update(f);
        self.settle();
    }

    fn read<R>(&mut self, f: impl FnOnce(&PluginHost) -> R) -> R {
        self.cx.update(|cx| f(PluginHost::global(cx)))
    }

    /// Lets real time pass for the file watcher, moving the clock along.
    fn idle(&mut self, real: Duration) {
        let until = Instant::now() + real;
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
            self.advance(Duration::from_millis(400));
        }
    }

    /// Waits up to ten seconds of real time for the file watcher.
    fn wait_until(&mut self, mut done: impl FnMut(&PluginHost) -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            self.idle(Duration::from_millis(100));
            if self.read(&mut done) {
                return true;
            }
        }
        false
    }

    fn open(&mut self, path: &VaultPath) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                workspace::open_note(path.clone(), OpenOptions::new(), window, cx)
            })
            .expect("open a note");
        self.settle();
    }

    /// The active note's editor.
    fn editor(&mut self) -> EditorSnapshot {
        self.cx
            .update(|cx| workspace::active_editor(cx))
            .expect("a note is open")
    }

    fn run_command(&mut self, id: &str) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                let command = PluginHost::global(cx)
                    .command(id)
                    .unwrap_or_else(|| panic!("no command {id}"));
                command.run(window, cx);
            })
            .expect("run the command");
        self.settle();
    }
}
