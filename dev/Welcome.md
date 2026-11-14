# Welcome to Notia

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
