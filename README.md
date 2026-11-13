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
sudo apt-get install pkg-config libwayland-dev libxkbcommon-dev libx11-dev libxcb1-dev libvulkan-dev libssl-dev cmake g++ libfontconfig1-dev
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
| Today's daily note | `⌘⇧T` |
| Change theme | `⌘K` then `⌘T` |
| Toggle the left sidebar | `⌘\` |
| Toggle the right sidebar | `⌘⇧\` |
| Settings | `⌘,` |
| Quit | `⌘Q` |

The command palette lists everything else, including inserting the current date or time and opening yesterday's or tomorrow's daily note.

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
| Daily notes | Today's note in the `Daily` folder, created when needed |
| Date and time | Inserts the current date or time |
| Theme gallery | Extra themes and the theme picker |

Community plugins are JavaScript, installed like Obsidian's. Each one is a folder in `.notia/plugins` with a `manifest.json` and a `main.js`. Notia lists a plugin from its manifest and loads the code only after you enable it. Turning it off unloads it. Saving its files reloads it. The API is in `crates/notia-js/notia.d.ts`, and Notia copies that file to `.notia/plugins/notia.d.ts` for editor completions.

```js
import { Plugin, Notice } from "notia";

export default class Hello extends Plugin {
  onload() {
    this.addCommand({
      id: "hello",
      name: "Say hello",
      callback: () => new Notice("Hello!"),
    });
  }
}
```

A plugin can add commands, ribbon buttons, status bar items, and sidebar panels, and it can read and write notes.

Rust plugins implement the `Plugin` trait and register through a `PluginContext`. The core plugins in `crates/notia-plugins` are written only against that public API.

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
