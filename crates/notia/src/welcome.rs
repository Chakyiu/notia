//! The notes a new vault starts with.

use std::io;

use notia_plugin::core::{Vault, VaultPath};

const WELCOME: &str = r#"# Welcome to Notia

Notia keeps your notes as plain Markdown files in a folder on your computer: this folder is your **vault**. Everything around the editor — the file list, search, the outline, backlinks, the status bar and every theme — is a *plugin* you can turn off in **Settings → Plugins**.

> [!tip] Start here
> Press `⌘P` (or `Ctrl+P`) to open the **command palette** and `⌘O` to jump to any note. The button above this note cycles **edit**, **half edit** (Markdown and preview side by side), and **preview**. `⌘E` does the same.

## Link your thinking

Wrap a note name in double brackets to link it: [[Getting around]] or [[Themes and plugins|how themes work]]. Links to notes that don't exist yet, like [[My first idea]], create the note when you follow them. Every note lists the notes linking to it in the **Backlinks** panel on the right.

Tags work too: #welcome #getting-started

## Write in Markdown

- **Bold**, *italic*, `inline code` and ==highlights==
- [ ] A task to finish
- [x] A finished task

```rust
fn main() {
    println!("Code blocks are highlighted in the reading view.");
}
```

> [!note]
> Callouts like this one are drawn with your theme's colors.

> [!warning] Mind the warning
> And this one stands out.
"#;

const GETTING_AROUND: &str = r#"# Getting around

Back to [[Welcome]].

## The window

- The **ribbon** on the far left holds plugin buttons: the theme picker and settings.
- The **left sidebar** holds the *Files* and *Search* panels; switch them with the icons at its top. The **New canvas** button there, or **Create new canvas** in the palette, opens a *Canvas* note: a board of boxes you drag around and join with connectors.
- The **right sidebar** holds the *Outline*, *Backlinks*, *Tags*, and *History* panels. *History* keeps each saved version of the open note. **Compare** opens the differences in a popup, and **Restore** puts a version back.
- The **status bar** at the bottom shows plugin items such as backlinks and word counts.

Drag a sidebar's edge to resize it, and use the buttons in the top corners to hide it.

## Keyboard

| Action | Shortcut |
| --- | --- |
| Command palette | `⌘P` |
| Quick switcher | `⌘O` |
| New note | `⌘N` |
| Cycle edit, half edit, and preview | the button above the note, or `⌘E` |
| Search in all files | `⌘⇧F` |
| Change theme | `⌘K` then `⌘T` |
| Settings | `⌘,` |

Shortcuts and the commands listed in the palette are changed in **Settings → Command palette**.

On Windows and Linux, use `Ctrl` in place of `⌘`. #getting-started
"#;

const THEMES: &str = r##"# Themes and plugins

## Theme plugins

A theme plugin controls every color of the interface, the syntax colors of the Markdown editor, fonts, corner radii and the typography of notes. Pick one in **Settings → Appearance** or with the **Change theme** command, which previews each theme as you move through the list.

To install a theme, put a folder with a `theme.json` (or a single `.json` file) into `.notia/themes` inside this vault. Notia watches that folder: save the file and the theme updates immediately. This vault already contains an example, **Midnight**, at `.notia/themes/midnight/theme.json`. Open it in any text editor and change a color to try it.

A theme file looks like this:

```json
{
  "id": "midnight",
  "name": "Midnight",
  "themes": [
    {
      "mode": "dark",
      "colors": { "background": "#0f1729", "primary.background": "#f5b14c" },
      "highlight": { "syntax": { "title": { "color": "#f5b14c", "font_weight": 700 } } },
      "notia": { "editor.font.size": 17, "editor.line_height": 1.7, "editor.max_width": 760 }
    }
  ]
}
```

`colors` and `highlight` use the GPUI Component theme keys; anything left out is derived from the colors you set. The optional `notia` block sets the note font, size, line height and readable line length.

## Community plugins

Community plugins are written in JavaScript and installed into the vault, like Obsidian's: each is a folder in `.notia/plugins` with a `manifest.json` and a `main.js`. Notia lists a plugin by reading its manifest and loads its code only once you turn it on in **Settings → Plugins**; turning it off unloads it again. Save a plugin's files and it reloads.

A plugin looks like this:

```js
import { Plugin, Notice } from "notia";

export default class Hello extends Plugin {
  onload() {
    this.addCommand({ id: "hello", name: "Say hello", callback: () => new Notice("Hello!") });
  }
}
```

Plugins can add commands, ribbon buttons, status bar items and sidebar panels, read and write notes, and follow the open note. `.notia/plugins/notia.d.ts` describes the whole API.

## Built-in plugins

The core plugins are written in Rust: they implement the `Plugin` trait and register their commands, panels and themes through a `PluginContext`. See [[Ideas/Plugin ideas]] for a few to build either way.

#themes #plugins
"##;

const PLUGIN_IDEAS: &str = r#"# Plugin ideas

Related: [[Themes and plugins]]

- A *graph view* panel that draws [[Welcome]] and its links
- A *random note* ribbon button
- A *templates* plugin that inserts a template at the cursor

#plugins #ideas
"#;

const MIDNIGHT: &str = r##"{
  "id": "midnight",
  "name": "Midnight",
  "version": "1.0.0",
  "author": "Your vault",
  "description": "An example theme installed in this vault's .notia/themes folder. Edit it and save to see it change.",
  "themes": [
    {
      "mode": "dark",
      "radius": 8,
      "colors": {
        "background": "#0f1729",
        "foreground": "#d6deeb",
        "border": "#1d2b45",
        "muted.background": "#16223a",
        "muted.foreground": "#8b9bb8",
        "accent.background": "#1b2945",
        "accent.foreground": "#e8eefa",
        "primary.background": "#f5b14c",
        "primary.foreground": "#0f1729",
        "link": "#f5b14c",
        "selection.background": "#f5b14c33",
        "list.hover.background": "#1b2945",
        "list.active.background": "#f5b14c26",
        "list.active.border": "#f5b14c00",
        "popover.background": "#121d33",
        "sidebar.background": "#0b1322",
        "sidebar.foreground": "#b7c3d9",
        "sidebar.border": "#1d2b45",
        "tab_bar.background": "#0b1322",
        "tab.foreground": "#6f80a0",
        "tab.active.background": "#0f1729",
        "tab.active.foreground": "#e8eefa",
        "title_bar.background": "#0b1322",
        "status_bar.background": "#0b1322",
        "base.red": "#ef6b73",
        "base.green": "#7fdbb6",
        "base.blue": "#82aaff",
        "base.yellow": "#f5b14c",
        "base.magenta": "#c792ea",
        "base.cyan": "#7fdbca"
      },
      "highlight": {
        "editor.background": "#0f1729",
        "editor.foreground": "#d6deeb",
        "syntax": {
          "title": { "color": "#f5b14c", "font_weight": 700 },
          "punctuation.special": { "color": "#4b5d80" },
          "punctuation.list_marker": { "color": "#82aaff" },
          "emphasis": { "font_style": "italic" },
          "emphasis.strong": { "font_weight": 700 },
          "link_text": { "color": "#82aaff" },
          "link_uri": { "color": "#4b5d80" },
          "text.code.span": { "color": "#7fdbca" },
          "comment": { "color": "#5a6b8c", "font_style": "italic" },
          "keyword": { "color": "#c792ea" },
          "function": { "color": "#82aaff" },
          "string": { "color": "#ecc48d" },
          "number": { "color": "#f78c6c" },
          "type": { "color": "#ffcb8b" }
        }
      },
      "notia": {
        "editor.font.size": 17,
        "editor.line_height": 1.7,
        "editor.max_width": 760
      }
    }
  ]
}
"##;

/// Writes the starter notes and the example theme into an empty vault.
pub fn create(vault: &Vault) -> io::Result<()> {
    vault.write_note(&VaultPath::new("Welcome.md"), WELCOME)?;
    vault.write_note(&VaultPath::new("Getting around.md"), GETTING_AROUND)?;
    vault.write_note(&VaultPath::new("Themes and plugins.md"), THEMES)?;
    vault.write_note(&VaultPath::new("Ideas/Plugin ideas.md"), PLUGIN_IDEAS)?;
    let config = vault.config_dir();
    std::fs::create_dir_all(config.join("themes/midnight"))?;
    std::fs::write(config.join("themes/midnight/theme.json"), MIDNIGHT)?;
    std::fs::create_dir_all(config.join("plugins"))?;
    std::fs::write(
        config.join("plugins/notia.d.ts"),
        notia_js::TYPE_DEFINITIONS,
    )?;
    std::fs::write(config.join("plugins.json"), "{}")?;
    // Open the welcome note the first time the vault is shown.
    std::fs::write(
        config.join("workspace.json"),
        r#"{ "tabs": ["Welcome.md"], "active_tab": 0 }"#,
    )
}
