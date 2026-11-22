# Writing community plugins

This document describes how to build, package, and list a community plugin
for Notia. It is derived from the code (the sources are cited throughout),
and the JavaScript API it documents is also shipped as TypeScript
definitions: Notia writes a copy of them to `.notia/plugins/notia.d.ts` when
you run the command **Open the community plugins folder** — keep it beside
your code for editor completions (`crates/notia-js/notia.d.ts`).

A community plugin is a folder with a `manifest.json` and an entry module
(usually `main.js`) whose default export is a class extending `Plugin`. Notia
reads the folders in `<vault>/.notia/plugins` (and folders symlinked there,
so a plugin can live in its own repository), but it runs the code of a plugin
only while it is enabled in **Settings → Plugins**.

- [1. manifest.json schema](#1-manifestjson-schema)
- [2. The JS API surface](#2-the-js-api-surface)
- [3. Lifecycle](#3-lifecycle)
- [4. Trust model](#4-trust-model)
- [5. Packaging and listing](#5-packaging-and-listing)
- [6. A minimal working example](#6-a-minimal-working-example)
- [Not supported](#not-supported)

---

## 1. manifest.json schema

Reads and validation of an installed plugin's manifest live in
`crates/notia-js/src/manifest.rs` (`read_manifest`); the archive-install path
in `crates/notia-workspace/src/plugin_catalog.rs:1262`
(`parse_plugin_manifest`) validates a subset of the same fields.

| Field         | Required | Type   | Rules |
| ------------- | -------- | ------ | ----- |
| `id`          | yes      | string | Trimmed; must not be empty. At most 64 bytes, only ASCII letters, digits, `-` and `_`; may not start with `-`, and may not be `.` or `..`. May not contain `:`, `/`, `\`, or whitespace. Used as the command prefix (`<id>:<command>`), the folder key in the plugins folder, and the catalog key. |
| `name`        | yes      | string | Trimmed; must not be empty. Shown in Settings → Plugins and used as a settings page name by default. |
| `version`     | no       | string | Free text when installed, but the update check only compares versions that are at most 40 characters of ASCII letters, digits, `.` `-` `+` (a leading `v` is ignored when comparing). |
| `author`      | no       | string | Shown in Settings → Plugins. |
| `description` | no       | string | Shown in Settings → Plugins. |
| `main`        | no       | string | The entry module, relative to the plugin folder. Defaults to `main.js`. Must be a relative path inside the folder; an empty value is rejected. |

Additional rules and behaviour:

- **Unknown fields are ignored**, not rejected — Obsidian's `minAppVersion`
  and friends are fine. The whole JSON object is what the plugin sees as
  `this.manifest`, except that Notia inserts one field:
  `dir` — the plugin folder relative to the vault, `.notia/plugins/<folder>`.
- The manifest is limited to 64 KiB when downloaded for the store
  (`MAX_MANIFEST_BYTES`, `plugin_catalog.rs:37`).
- `main` is treated as a vault-style path, so a value like `src/main.js` is
  kept, while `..` segments are dropped before use.
- Duplicate ids: if two folders in the plugins folder declare the same `id`,
  only the first (in folder-name order) is registered; the other is ignored
  with a log warning (`loader.rs:128`).

Error messages a user can see:

| Situation | Message |
| --------- | ------- |
| Manifest is not valid JSON (e.g. `id` is a number) | `manifest.json is invalid: <serde detail>` |
| `id` empty / bad characters | `“<id>” is not a valid plugin id: use letters, digits and dashes` |
| `name` empty | `manifest.json has no name` |
| `main` empty | `manifest.json has an empty main` |
| Store install: no usable `id` | `This repository's manifest.json has no usable id.` |
| Store install: no `name` | `This repository's manifest.json has no name.` |
| Store install: empty `main` | `This repository's manifest.json has an empty main.` |
| Store install: catalog id ≠ manifest id | `The list names this plugin “<catalog id>”, and the manifest id is “<manifest id>”.` |
| Store install: entry module missing | `This repository is missing <main>.` |

An unusable manifest does not crash anything: the folder is still listed in
Settings → Plugins as a broken entry, with the error as its description
(`loader.rs:270`).

## 2. The JS API surface

Plugins run in a QuickJS engine, one context per plugin. The API comes from
the `notia` module, whose source is `API_SOURCE` in
`crates/notia-js/src/runtime.rs:43`; everything the module re-exports is
defined in `crates/notia-js/src/prelude.js`, and each call goes through one
host function into the Rust code in `crates/notia-js/src/host.rs`. Both
module styles work:

```js
import { Plugin, Notice, ui, requestUrl } from "notia";
export default class extends Plugin { ... }
```

```js
const { Plugin } = require("notia");
module.exports = class extends Plugin { ... };
```

**Module loading** (`runtime.rs:449`, `PluginModules`): only `notia` and
relative paths (`./`, `../`) that stay inside the plugin folder resolve.
A relative import tries `path`, `path.js`, then `path/index.js`. Anything
else — bare package names, absolute paths, paths that leave the folder —
fails with *"plugins can import "notia" and files in their own folder"*.

**Standard globals provided by the prelude**: `setTimeout`, `setInterval`,
`clearTimeout`, `clearInterval`, `queueMicrotask`, and a frozen `console`
(`log`/`info`/`debug`/`warn`/`error` — lines go to Notia's log under the
`notia_js` target, so `console.log` is invisible in the UI). There is **no**
`fetch`, `XMLHttpRequest`, Node API, or `localStorage`.

**Hard runtime limits** (`runtime.rs:33-34`):

| Limit | Value |
| ----- | ----- |
| Memory per plugin | 256 MiB |
| One call into the plugin (onload, a callback, an event, a render, a timer) | stopped after 5 s — the error is `Stopped after running for more than 5 seconds.` |

A pending promise (for example an `onload` waiting on a timer) does not count
against the limit; only synchronous JS execution does.

### 2.1 The `Plugin` class

All methods below exist on the class your plugin extends. "Sync" means the
host call returns before your code continues. `loadData`/`saveData` are
`async` functions as Obsidian plugins know them, although the underlying
call is synchronous. Nothing here is documented that is not in
`API_SOURCE` / `prelude.js`.

| Member | Signature | Sync/Async | Notes |
| ------ | --------- | ---------- | ----- |
| `constructor(app, manifest)` | — | sync | `manifest` is your manifest.json plus the injected `dir` field. |
| `onload()` | `() => void \| Promise<void>` | sync (may be async) | Called when the plugin is enabled. Anything you register after it returns is still added to the plugin (`extend_plugin`). |
| `onunload()` | `() => void` | sync | Called on disable and before a reload. Timers are cleared and contributions removed regardless. |
| `addCommand` | `(command: Command) => Command` | sync | `Command`: `{ id, name, icon?, hotkey?, callback? }` or `editorCallback?(editor, note)` instead of `callback`. `id` must not contain `:` (error: *"a command needs an id without ":""*); empty `name` is rejected (*"a command needs a name"*). With `editorCallback` the command is only listed while a note is open. `icon` and `hotkey` are optional. |
| `addRibbonIcon` | `(icon, title, callback) => void` | sync | `icon` is a Lucide icon name. |
| `addStatusBarItem` | `() => StatusBarItem` | sync | Returns `{ setText(text) }`; chainable. |
| `addPanel` | `(options: PanelOptions) => PanelHandle` | sync | `PanelOptions`: `{ id, title, icon?, side?, render }`. `side` is `"left"`, `"right"` (default), or `"editor"` (a bar above the note instead of a sidebar). `render` must be a function. `id` must be unique within the plugin and must not contain `:`. Returns `{ refresh(), show() }`. |
| `addSettingTab` | `(options: SettingTabOptions) => SettingTabHandle` | sync | `{ title?, icon?, render }`. `title` defaults to the plugin name. Returns `{ refresh() }`. |
| `registerEvent` | `(ref) => ref` | sync | Pass-through: listeners are removed on unload anyway. |
| `registerInterval` | `(id) => id` | sync | Pass-through: intervals are cleared on unload anyway. |
| `loadData` | `() => Promise<any>` | async | Reads `data.json` in the plugin folder; `null` when there is none; fails if it is not valid JSON. |
| `saveData` | `(data) => Promise<void>` | async | Pretty-prints `data.json`; written atomically (temp file + rename). |
| `readFile` | `(name) => string \| null` | sync | A text file in the plugin folder; `null` when missing. At most 1 MiB (`MAX_PLUGIN_FILE`, `host.rs:725`); non-UTF-8 or bigger fails. |
| `writeFile` | `(name, text) => void` | sync | Writes inside the plugin folder (subfolders are created). 1 MiB limit; written atomically. |
| `openFile` | `(name) => void` | sync | Opens a file in the plugin folder with the system editor. |
| `setThemeColors` | `(colors) => { warnings }` | sync | Keys are theme color names (`"background"`, `"primary.background"`, `"highlight.<name>.color"`); values `#hex` strings or `null`. An empty object clears this plugin's overrides. Unknown names/colors come back in `warnings` and are left unchanged. |
| `setFileIcons` | `(icons \| null) => { warnings }` | sync | `{ folder?, folderOpen?, folderColor?, noteColor?, fileColor?, folders? }`; Lucide names, `#hex` or theme color names. `null` clears the replacement. |

`this.app.vault` — all synchronous; paths are vault-relative, `.md` may be
omitted, and hidden files (any path starting with `.`) are rejected with
*"plugins cannot access hidden files like …"* (`host.rs:699`, `note_path`):

| Member | Signature | Notes |
| ------ | --------- | ----- |
| `getName()` | `() => string` | |
| `getNotes()` | `() => NoteInfo[]` | Every note sorted by path: `{ path, name, tags, words, modified }`; `modified` is ms since 1970 or `null`. |
| `read` | `(path) => string` | The note's text on disk. |
| `exists` | `(path) => boolean` | |
| `create` | `(path, text?) => string` | Returns the path; fails if the note exists (*"…" already exists*). |
| `modify` | `(path, text) => void` | Fails if the note does not exist (*"there is no note …"*). |
| `append` | `(path, text) => void` | Adds the text on a new line at the end, creating the note if needed. |
| `on` | `(event, callback) => EventRef` | Events: `"modify"` → `(path)`, `"rename"` → `(path, oldPath)`, `"delete"` → `(path)`, `"change"` → `()`. `EventRef.off()` removes the listener. |

`this.app.workspace`:

| Member | Signature | Notes |
| ------ | --------- | ----- |
| `getActiveNote()` | `() => ActiveNote \| null` | `{ path, name, text, line, mode }`; `mode` is `"editing"`, `"half"` or `"preview"`; `line` is the 0-based cursor line; `text` includes unsaved edits. |
| `getActiveEditor()` | `() => Editor \| null` | See below. |
| `openNote` | `(path, options?) => void` | `options`: `{ line?, newTab? }`. Fails when the note does not exist. |
| `openLink` | `(target, options?) => void` | Opens what a `[[link]]` points at, creating the note if needed. |
| `newNote` | `(folder?) => void` | Creates and opens a note in `folder`, or the configured folder. |
| `insertText` | `(text) => void` | Inserts at the cursor of the active note and focuses it. |
| `revealLine` | `(line) => void` | |
| `search` | `(query) => void` | Runs the vault search, e.g. `"tag:#ideas"`. |
| `runCommand` | `(id) => void` | Ids without a `:` are namespaced to your own plugin. |
| `on` | `(event, callback) => EventRef` | `"file-open"` → `(note \| null)`, `"editor-change"` → `(note)`. |

`this.app.metadata` — all synchronous, all take vault paths:

| Member | Signature |
| ------ | --------- |
| `getHeadings` | `(path) => { level, text, line }[]` |
| `getLinks` | `(path) => { target, subpath, display, line, embed, resolved }[]` |
| `getBacklinks` | `(path) => { source, line, context }[]` |
| `getTags` | `() => { tag, count }[]` (no `#`, most used first) |
| `getNotesWithTag` | `(tag) => string[]` |
| `resolveLink` | `(target, from?) => string \| null` |

### 2.2 The `Editor`

`app.workspace.getActiveEditor()` and `command.editorCallback(editor, note)`
return an editor for the open note, modelled on Obsidian's `Editor`
(`prelude.js`, `makeEditor`; host side `editor_call` in `host.rs:551`).
Positions are `{ line, ch }`, 0-based, with `ch` counted like JavaScript
string indices (UTF-16 code units). Reads always see the text including
edits made by any editor; each edit is one undo step and is applied once
your code returns. The editor is a frozen object; all methods are sync.

`getDoc`, `refresh` (does nothing), `getValue`, `setValue`,
`getLine`, `setLine`, `lineCount`, `lastLine`, `getSelection`,
`somethingSelected`, `getRange`, `replaceSelection`, `replaceRange`,
`getCursor(which = "head")` (`"from" | "to" | "head" | "anchor"`),
`listSelections` (one selection), `setCursor(position | line, ch?)`,
`setSelection(anchor, head?)`, `setSelections(ranges, main?)` (uses
`ranges[main]`), `transaction({ changes?, replaceSelection?, selection? })`
(changes must not overlap), `posToOffset`, `offsetToPos`, `wordAt`,
`focus`, `undo`, `redo`.

Only `undo`, `redo` and `focus` exist as editor commands on the host side;
anything else raises *"editors have no command "…""*.

### 2.3 `requestUrl`

```js
const response = await requestUrl({
  url: "https://example.com/api",
  method: "POST",                       // default: GET, or POST when body is set
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ q: 1 }),       // a string; at most 2 MiB
  timeout: 30_000,                      // ms; default 120 s, clamped to 1–180 s
  onChunk: (text) => { ... },           // optional streaming
});
// response: { status, headers, text }   — a 404 still resolves
```

Implemented in `crates/notia-js/src/http.rs` and run off the UI thread. The
promise rejects only when the request cannot be made or read; every HTTP
status, including 4xx/5xx, resolves.

Limits and rules (`http.rs:9-17`, `prepare` at `http.rs:54`):

| Rule | Value / behaviour |
| ---- | ----------------- |
| Schemes | `http://` and `https://` only; anything else: *"the address must start with http:// or https://"*. |
| Concurrent requests per plugin | 8 (`MAX_IN_FLIGHT`); beyond that: *"too many requests at once"*. |
| URL length | 8 KiB; no whitespace or control characters. |
| Body | 2 MiB (`MAX_BODY`); a non-empty body and no explicit method means `POST`. |
| Methods | `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD` only. |
| Request headers | At most 40; names at most 128 characters of ASCII letters, digits, `-`, `_`; values at most 8 KiB with no CR/LF. |
| `User-Agent` | Added as `Notia/<version>` unless you set your own. |
| Timeout | Default 120 s; any positive value is clamped to 1 000–180 000 ms. Applies to the whole exchange including redirects. |
| Response | At most 4 MiB of **text** (`MAX_RESPONSE`); non-UTF-8 bodies fail with *"the reply is not text"*. Headers come back lower-cased (up to 40 of them). |
| Redirects | At most 4 hops; every hop must stay on http/https; `301/302/303` are turned into `GET` without a body. |
| Errors | Connection errors say `could not reach the server: …` with the URL's query string scrubbed out of the log. |
| Streaming | With `onChunk` set, each piece of a successful (2xx) body is delivered as it arrives (valid UTF-8 boundaries); the promise still resolves with the whole `text`. |

### 2.4 `Notice` and `ui`

`new Notice(message)` shows a notification.

`ui` builds the elements a panel's or settings tab's `render` returns
(`prelude.js`; the Rust side decodes them in `crates/notia-js/src/view.rs`).
Children may be elements, strings, or `null`/`undefined`/booleans (left out).
Every callback (button `onClick`, `onChange`, …) must be a function.

| Builder | Element |
| ------- | ------- |
| `column(children, options?)`, `row(children, options?)` | Layout rows/columns. `row` options: `{ wrap?: boolean, spread?: boolean }` — `wrap: false` keeps one line; `spread` pushes the first and last child apart. |
| `composer(children)` | A bordered field (a prompt and its controls). |
| `heading(text)` | A small section heading. In a settings tab it starts a new section. |
| `text(text, options?)` | Options `{ muted?, bold?, small?, danger? }`. A bare string child is rendered as text. |
| `icon(name, options?)` | A Lucide icon without a button; `{ muted? }`. |
| `markdown(source)` | Renders Markdown. |
| `button(label, onClick, options?)` | Options `{ icon?, variant?: "outline" \| "primary" \| "ghost", tooltip? }`; a tooltip with an empty label makes an icon button. |
| `list(items, options?)` | Items are `{ title, subtitle?, icon?, onClick? }` or strings; `{ compact? }` draws tighter rows. |
| `chip(label, onClose?)` | A short label; setting `onClose` draws an × that removes it. |
| `separator()`, `empty(text)` | A rule line; a centered muted message. |
| `setting(name, control, options?)` | A settings row; options `{ description? }`. `control` is one of the controls below or any element. Only at the top level of a settings tab. |
| `toggle(value, onChange)` | A switch. |
| `textInput(value, onChange, options?)` | `{ placeholder?, secret? }`; `secret` hides the value (for tokens). |
| `textArea(value, onChange, options?)` | `{ id?, rows?, maxRows?, placeholder?, plain?, onSubmit? }`; `id` keeps the field across renders; `onSubmit` fires on Enter (Shift+Enter inserts a line); `plain` drops the border for use inside `composer`. |
| `numberInput(value, onChange, options?)` | `{ min?, max?, step? }`. |
| `dropdown(value, options, onChange)` | Options: strings, `[value, label]` pairs, or `{ value, label }`. |
| `tabBar(tabs, actions?)` | Tabs `{ label, icon?, active?, onSelect?, onClose? }`. |
| `dock(body, footer, header?)` | A scrolling body with a pinned footer (and optional header) for sidebar panels. |

A `render` may return one element, an array of them, a string, or
`null`/`undefined`/a boolean. Callbacks are matched by index between
renders, so a render must keep its controls in the same order for handlers
to keep firing (`view.rs`, `Handlers`).

## 3. Lifecycle

Where things live (`loader.rs`, `manifest.rs`, `notia-plugin/src/host.rs`):

- Plugins folder: `<vault>/.notia/plugins/<id-or-folder>/`.
- Enabled choices: `<vault>/.notia/plugins.json` — a JSON object
  `{ "<plugin id>": true|false }`, written by Notia when you toggle a
  plugin. It is Notia's file, not yours.
- Plugin state: `data.json` in the plugin folder (`loadData`/`saveData`).
- Type definitions: `notia.d.ts` written into the plugins folder by
  **Open the community plugins folder**.

**Discovery and lazy load.** On startup (and on every reload), Notia lists
the folders in the plugins folder — skipping names that start with `.`, and
following symlinks — and reads only each `manifest.json`
(`manifest.rs::discover`). **No plugin code runs at this point.** A plugin's
`main.js` is loaded and its `onload` runs only when it is enabled
(`loader.rs:239`: *registering it reads nothing but its manifest; its code
is loaded when it is enabled*). A community plugin is never enabled by
default.

**Activation.** When you enable a plugin in Settings → Plugins, Notia
creates the QuickJS context, evaluates the entry module, calls `onload`
(may be async), then hands everything registered during `onload` to the
plugin host. Things registered later — from a timer, for example — are
added as well (`runtime.rs::extend_plugin`). The plugin subscribes to
vault and workspace events it asked for; events that fire while the plugin's
own code is running are queued and delivered when it returns
(`runtime.rs::emit`).

**Events delivered** (`runtime.rs::attach`):

| Event | Payload |
| ----- | ------- |
| `workspace:file-open` | `(note \| null)` — the active note changed |
| `workspace:editor-change` | `(note)` — the active note's text changed |
| `vault:modify` | `(path)` — per changed note |
| `vault:rename` | `(path, oldPath)` |
| `vault:delete` | `(path)` — the note was trashed |
| `vault:change` | `()` — anything in the vault changed |

**Deactivation.** Disabling a plugin (or the whole community-plugin loader)
calls `onunload`, clears its timers, removes its commands, ribbon icons,
panels, status items, settings tabs, navigation icons and theme/file-icon
overrides, and removes its event listeners. Nothing of the plugin's
contributions survives being disabled.

**Reload.** A file watcher watches the plugins folder (recursively, and also
the targets of symlinked plugin folders). Any change to a non-hidden,
non-data file triggers a reload after a 300 ms settle period
(`loader.rs::watch`); reload re-reads manifests and re-registers changed
plugins, which re-runs `onload` of the enabled ones. `data.json`,
`sessions.json`, `*.notia-tmp` and `.css` files are *data files*: saving
them does not reload the plugin, and neither does anything inside a
folder named `index` (`manifest.rs::is_data_file`). A manifest
plus modification-time fingerprint avoids reloading when nothing changed.

**What persists across a reload** is the enabled state (in
`plugins.json`), the code on disk, and whatever your plugin saved through
`saveData`/`writeFile`. There is no other storage: nothing is written
outside the plugin folder and the vault.

**Errors.** A JS failure is logged with the plugin id and shown as a
notification (`<plugin name>: <first line>`); the same failure within
10 s is not shown again (`runtime.rs::report`). A plugin whose `onload`
never returns — an infinite loop — is stopped by the 5 s limit and marked
failed, as `crates/notia/tests/community_plugins.rs` verifies with its
`slow` case. A plugin whose manifest cannot be read is listed as broken
with the reason.

## 4. Trust model

Stated plainly, for authors and reviewers:

- **Enabling a plugin is full trust.** There is no permission allowlist, no
  capability negotiation, and no prompts. An enabled plugin can:
  - read, create, modify and append to **every note in the vault** except
    hidden files (paths starting with `.`, which includes Notia's own
    `.notia/` configuration) — `host.rs::note_path`;
  - make HTTP requests to **any** `http`/`https` host, subject only to the
    size/time caps in §2.3 — there is no host allowlist;
  - edit the open note through the editor API, insert text, open notes and
    links, run commands, and change the theme and file icons.
- A plugin can therefore exfiltrate vault contents (`app.vault.read` →
  `requestUrl`) the moment it is enabled. The size caps limit how fast,
  not whether. Reviewers should treat every `requestUrl` call as a data
  exit and check what is sent.
- **Install path.** The Plugin store downloads a zip archive of the
  repository branch named by the catalog (`codeload.github.com/<repo>/zip/refs/heads/<branch>`,
  at most 8 MiB). The extraction step
  (`plugin_catalog.rs:1295`, `extract_plugin`):
  - rejects or skips unsafe entries: symlinked entries are skipped, paths
    containing backslashes are skipped, and any absolute or `..` component
    fails the install with *"The plugin archive has an unsafe path."*
    (`write_tree`, `plugin_catalog.rs:1459`);
  - keeps at most 200 files of at most 2 MiB each and 16 MiB unpacked;
  - drops dotfiles and packaging noise (see §5) before writing;
  - installs by staging a `.install-<id>` folder, preserving the previous
    `data.json`, moving the old folder aside, then swapping in — a failed
    install leaves the previous version in place (`swap_in`).
  The store refuses an archive whose `manifest.json` id does not match the
  catalog entry id.
- **What is not verified today**: archives are not signed and not
  digest-checked; the catalog itself is fetched over plain HTTPS from one
  GitHub raw URL with no pinning. Author identity is whatever the manifest
  says. If you are reviewing a plugin before it is listed, verify the
  repository yourself.
- Local development bypasses the store entirely: put (or symlink) a folder
  in `.notia/plugins` and enable it. The same trust applies.

## 5. Packaging and listing

### 5.1 Building an archive the installer accepts

The installer takes any zip whose files, after the steps below, contain a
readable `manifest.json` and the entry module named by its `main`
(default `main.js`).

- **Layout.** Files at the archive root, *or* everything under a single
  top-level folder: if every entry shares one root folder — GitHub's
  "Download ZIP" layout, `<repo>-<branch>/…` — that folder is stripped
  (`extract_plugin`'s `strip_root`).
- **Per-file caps**: 2 MiB per file, at most 200 files, 16 MiB unpacked,
  8 MiB compressed, at most 2 000 entries. Exceeding any of these fails
  with *"A file in the plugin is too large."* or *"The plugin is too large."*
- **Dropped before install** (`keep_file`, `plugin_catalog.rs:1368`):
  - every path component starting with `.` (so `.git`, `.github`, and
    hidden files), plus these directories: `node_modules`, `test`,
    `tests`, `target`, `.cursor`, `.impeccable`;
  - these files at any level: `README.md`, `LICENSE`, `LICENSE.md`,
    `PRODUCT.md`, `package.json`, `package-lock.json`, `jsconfig.json`,
    `tsconfig.json`, `notia.d.ts`, `data.json`, `.DS_Store`,
    `.gitignore`.
  Note that `data.json` is stripped so an install cannot clobber the
  user's settings; the installer then re-copies the existing `data.json`
  into the new version.
- **Skipped entries**: symlinks, and any path containing `\`.
- **Content checks**: `manifest.json` must parse and satisfy §1; its `id`
  must equal the store entry's id; the file named by `main` must be
  present.

So a minimal acceptable archive is exactly:

```
manifest.json
main.js
```

plus any other files your plugin reads with `readFile` or imports
relatively.

### 5.2 Getting into the catalog

The catalog is `community-plugins.json` in the
[notia-manifest](https://github.com/Chakyiu/notia-manifest) repository —
check that repo for the current contribution process rather than
duplicating it here. Its file is a JSON array; at most 100 entries are used
and the rest are skipped (`plugin_catalog.rs:907`, `parse_catalog`).

| Field | Required | Rules |
| ----- | -------- | ----- |
| `id` | yes | Same rule as the manifest's `id` (§1); must be unique in the list — a duplicate is skipped with a log line. |
| `name` | yes | Trimmed non-empty, at most 80 characters. |
| `author` | no | At most 80 characters. |
| `description` | no | At most 400 characters. |
| `repo` | yes | Exactly `owner/name`; both parts at most 100 characters of ASCII letters, digits, `.` `-` `_`, and not `.` or `..`. |
| `branch` | no | Defaults to `main`. At most 100 characters of ASCII letters, digits, `-` `_` `.` `/`; may not start with `-` or `/`, end with `/` or `.`, or contain `//`, `..`, or `@`. |

Beyond the syntax check, an entry is only listed when the repository's
`manifest.json` on that branch exists, parses, and has the same `id`
(`remote_manifest`); otherwise the entry is skipped with
*"plugin list entry "<id>" does not match its repository manifest"*. The
remote manifest's `version` is also read (when it satisfies the version
rule in §1) and shown in the store.

**Updates** are offered when the remote version is newer than the
installed one (`remote_is_newer`): versions are compared component-wise as
numbers, ignoring a leading `v`, so `0.2.10 > 0.2.9` and `v1.0` == `1.0`.
A version that is empty or malformed disables the comparison (an empty
installed version counts as older).

The store fetches three things from the repo, each size-capped:
`manifest.json` (64 KiB), the zip archive (8 MiB), and `README.md`
(256 KiB, rendered in the store page).

## 6. A minimal working example

This pair of files is a complete, installable plugin. Every API call in it
is cross-checked against `crates/notia/tests/community_plugins.rs`, which
builds and runs real plugins of exactly this shape headless.

**`manifest.json`**

```json
{
  "id": "note-counter",
  "name": "Note Counter",
  "version": "0.1.0",
  "author": "You",
  "description": "Shows how many notes the vault holds, and appends tallies.",
  "main": "main.js"
}
```

**`main.js`**

```js
import { Plugin, ui } from "notia";

export default class NoteCounter extends Plugin {
  async onload() {
    this.data = (await this.loadData()) ?? { runs: 0 };
    this.data.runs += 1;
    await this.saveData(this.data);

    // A status bar item: how many notes the vault holds.
    this.addStatusBarItem().setText(`${this.app.vault.getNotes().length} notes`);

    // A sidebar panel with a heading, a line of text, and a button.
    this.addPanel({
      id: "counter",
      title: "Note Counter",
      icon: "hash",
      render: () => [
        ui.heading("Note Counter"),
        ui.text(`Loaded ${this.data.runs} times; ${this.app.vault.getNotes().length} notes.`),
        ui.button("Add a tally", () => {
          this.app.vault.append("Counter", "tally"); // creates Counter.md if needed
          this.app.workspace.openNote("Counter");
        }),
      ],
    });

    // A command in the palette: "Note Counter: Add a tally".
    this.addCommand({
      id: "tally",
      name: "Add a tally",
      hotkey: "ctrl-alt-t",
      callback: () => this.app.vault.append("Counter", "tally"),
    });
  }

  onunload() {
    // Nothing to clean up: contributions and timers are removed for you.
  }
}
```

To try it without the store: put both files in
`<vault>/.notia/plugins/note-counter/`, run **Reload community plugins**,
and enable **Note Counter** in Settings → Plugins.

What each piece relies on (all verified by the test suite's `probe` /
`commonjs` / `fetch` / `options` / `painter` plugins):

- `import { Plugin, ui } from "notia"` and a default-exported subclass —
  the loader rejects a module without one
  (*"main.js must export a plugin class: export default class extends Plugin {}"*).
- `loadData` returns `null` on first run; `saveData` writes `data.json`
  next to `main.js`.
- `addStatusBarItem().setText(...)`, `addPanel` with an array `render`
  returning `ui.heading` / `ui.text` / `ui.button`, and `addCommand` with
  `id`/`name`/`callback` — the same calls the `probe` test plugin makes,
  including the panel rendering and the command running headless.
- `vault.append(path, text)` creating the note when it does not exist —
  the host-side behaviour in `vault_call`.
- `hotkey: "ctrl-alt-t"` — passed to the keybinding system as written;
  modifier names are `cmd`, `ctrl`, `alt`, `shift`, `secondary` (⌘ on
  macOS, Ctrl elsewhere).

**Verification status of this example:** the Notia checkout used to write
this document had no Rust toolchain available (no `cargo`, `rustc`, or
`rustup` on PATH), so the example was **not executed**. It is verified by
reading: every call above exists in `prelude.js`/`host.rs`, and the same
shapes run and pass in `crates/notia/tests/community_plugins.rs`. To
confirm on your machine:

```sh
cargo test -p notia --test community_plugins
```

## Not supported

Documented so authors do not search for them (no runtime behaviour was
changed to produce this list):

- No `fetch`, `XMLHttpRequest`, WebSockets, or service workers; network
  access is `requestUrl` only, which returns **text** bodies (4 MiB cap) —
  binary downloads are not available.
- No Node built-ins, no `import` of npm packages, no `localStorage`;
  persistence is `data.json` and files in the plugin folder.
- No sub-windows or dialogs of the plugin's own; UI is panels, settings
  tabs, the status bar, the ribbon, and notices.
- `editor.exec`-style commands are limited to `focus`, `undo`, `redo`;
  there is no `replaceRanges`-style bulk API beyond `transaction`, and a
  note has exactly one selection.
- HTTP redirects never cross schemes, and there is no way to read response
  headers beyond the first 40, each at most 8 KiB.
- `console.log` goes to the app log, not to any visible console.

Nothing in this list looked like a defect worth a separate issue; the
notable absence of archive signature/digest verification is already
covered by the parent backlog (self-update verification gap).
