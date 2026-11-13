//! Runs community plugins in a headless Notia: loading them lazily, their
//! API, their failures and reloading them.

use std::{fs, path::Path, sync::Arc, time::Duration};

use gpui_kit::{
    AnyWindowHandle, Bounds, HeadlessAppContext, WindowBounds, WindowOptions, px, size,
};
use notia_plugin::{PluginHost, core::Vault};

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
        r#"{ "probe": true, "commonjs": true, "broken": true, "slow": true }"#,
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
