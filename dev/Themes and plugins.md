# Themes and plugins

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
