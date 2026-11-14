# Notia

Notia is a note-taking app. Notes are plain Markdown files in a folder on your computer, called a vault. The file list, search, outline, backlinks, status bar, and themes are plugins, and each one can be turned off.

```
cargo run -p notia
cargo run -p notia -- ~/Notes
```

A folder on the command line opens that vault. Otherwise Notia opens the vault from last time, or asks where a new one should live. An empty vault starts with a few notes that explain the window.

## Build

Notia needs Rust 1.85 or newer, for the 2024 edition.

On Debian and Ubuntu, the GUI libraries are:

```
sudo apt-get install pkg-config libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libx11-dev libxcb1-dev libvulkan-dev libssl-dev cmake g++ libfontconfig1-dev
```

Release images go to `dist/`:

```
scripts/package.sh              # this operating system
scripts/package.sh macos        # Notia.app inside a .dmg
scripts/package.sh linux        # .tar.gz, and an AppImage when appimagetool is installed
scripts/package.sh windows      # .zip of notia.exe
scripts/package.sh all          # every image this machine can produce
```

A macOS, Windows, or Linux build links that system's libraries, so each image is produced on that system. From macOS or Windows, the Linux image is built in Docker when Docker is running. The Windows image is built by running `scripts/package.sh windows` on Windows. The macOS app requires macOS 12 or newer.

Pushing a version tag builds all three and publishes them as a GitHub release. The tag has to match `version` in `Cargo.toml`, so version `0.1.0` is tag `v0.1.0`.

```
git tag v0.1.0
git push origin v0.1.0
```

The release contains:

| File | System |
| --- | --- |
| `Notia-<version>-macos-<arch>.dmg` | macOS, Apple silicon on the GitHub runner |
| `Notia-<version>-linux-<arch>.tar.gz` | Linux. The binary links the system Wayland, X11, and font libraries |
| `Notia-<version>-windows-<arch>.zip` | Windows |

The macOS app is signed ad hoc, and the Windows executable is unsigned, so the system asks for confirmation before opening them. Running the workflow by hand, without a tag, builds the same images and keeps them as workflow artifacts.

## Writing

Notes are `.md` files. `[[Getting around]]` links to a note, and `[[Themes and plugins|how themes work]]` shows different text. Following a link to a note that does not exist creates it. `#tags` are indexed. The editor also handles tasks, `==highlights==`, fenced code, and callouts such as `> [!tip]`.

Pasted images are stored in `attachments/`. Deleted files move to `.trash` in the vault. Vault settings, installed themes, and community plugins live in `.notia/`.

The button above a note cycles edit, half edit (Markdown and preview side by side), and preview. `⌘E` does the same.

Search accepts `"phrases"`, `tag:#name`, and `path:folder`.

## Keyboard

On Windows and Linux, Ctrl takes the place of `⌘`. Next and previous tab use Ctrl on every system.

| Action | Shortcut |
| --- | --- |
| Command palette | `⌘P` |
| Quick switcher | `⌘O` |
| New note | `⌘N` |
| Save | `⌘S` |
| Close tab | `⌘W` |
| Next tab | `Ctrl+Tab` |
| Previous tab | `Ctrl+Shift+Tab` |
| Edit, half edit, and preview | `⌘E` |
| Search in all files | `⌘⇧F` |
| Change theme | `⌘K` then `⌘T` |
| Toggle the left sidebar | `⌘\` |
| Toggle the right sidebar | `⌘⇧\` |
| Settings | `⌘,` |
| Quit | `⌘Q` |

The command palette lists everything else, including inserting the current date or time.

## Plugins

Core plugins are Rust and ship with the app. Turn them off in **Settings → Plugins**.

| Plugin | What it adds |
| --- | --- |
| Files | The vault tree: open, create, rename, and delete notes and folders |
| File icons | A folder or file icon beside each item |
| Search | Search across note names and text |
| Outline | Headings of the active note |
| Backlinks | Notes that link here, and the links this note makes |
| Tags | Every tag in the vault |
| Word count | Words and characters in the status bar |
| Date and time | Inserts the current date or time |
| Theme gallery | Nord, Rosé Pine, Catppuccin, and Paper |

Community plugins are JavaScript, installed like Obsidian's. Each one is a folder in `.notia/plugins`, or a link there to a folder elsewhere, with a `manifest.json` and a `main.js`. Notia lists a plugin from its manifest and loads the code only after you enable it. Turning it off unloads it. Saving its files reloads it, including through a link. The API is in `crates/notia-js/notia.d.ts`, and Notia copies that file to `.notia/plugins/notia.d.ts` for editor completions.

```js
import { Plugin, Notice, ui } from "notia";

export default class Hello extends Plugin {
  async onload() {
    this.settings = { greeting: "Hello!", ...(await this.loadData()) };
    this.addCommand({
      id: "hello",
      name: "Say hello",
      callback: () => new Notice(this.settings.greeting),
    });
    this.addSettingTab({
      render: () => [
        ui.setting("Greeting", ui.textInput(this.settings.greeting, (greeting) => {
          this.settings.greeting = greeting;
          this.saveData(this.settings);
        })),
      ],
    });
  }
}
```

A plugin can add commands, ribbon buttons, status bar items, sidebar panels, bars above the open note, and a page of options in Settings. It can read and write notes, and change the open note through an `Editor` like Obsidian's: `app.workspace.getActiveEditor()`, or the first argument of a command's `editorCallback`.

A plugin is written only against this API, so it can live in a repository of its own. Daily notes does: it opens a note for each day, creating it when needed, and **Open today's daily note** is `⌘⇧T`. Link its folder into a vault and keep a copy of `notia.d.ts` beside its `main.js`:

```
ln -s ~/code/my-plugin ~/Notes/.notia/plugins/my-plugin
```

Rust plugins implement the `Plugin` trait and register through a `PluginContext`. The core plugins in `crates/notia-plugins` are written only against that public API. A Rust plugin's settings page is a `SettingsTab`, and `Notia::save_plugin_data` keeps its values in `.notia/plugin-data`.

## Themes

A theme sets the interface colors, syntax colors, fonts, corner radii, and note typography. The built-in themes are Notia, Nord, Rosé Pine, Catppuccin, and Paper. Pick one in **Settings → Appearance**, or with **Change theme**, which previews each theme as you move through the list.

To add one, put a folder with a `theme.json` (or a single `.json` file) in `.notia/themes`. Notia watches that folder and applies a saved change immediately. A new vault includes an example, Midnight, at `.notia/themes/midnight/theme.json`.

## Layout

| Crate | Role |
| --- | --- |
| `notia` | The application binary |
| `notia-core` | Vault, note parsing, link index, and search. No UI |
| `notia-theme` | Theme files and applying them |
| `notia-plugin` | The plugin API |
| `notia-workspace` | The window: ribbon, sidebars, tabs, palette, and settings |
| `notia-plugins` | The core plugins and bundled themes |
| `notia-js` | Community plugins, run in QuickJS |

The interface is [GPUI](https://gpui.rs) through [gpui-kit](https://crates.io/crates/gpui-kit).

```
cargo test
cargo test -p notia --test screenshots   # renders the main screens to PNGs; needs a GPU
```

Set `RUST_LOG` to change logging. The default filter is `warn,notia=info`.

## License

[MIT](LICENSE).
