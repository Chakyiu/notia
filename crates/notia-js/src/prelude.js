// The JavaScript half of the plugin API. It runs first in each plugin's own
// QuickJS context, defines what `import ... from "notia"` returns, and talks
// to Notia through a single host function that takes and returns JSON.
"use strict";

(() => {
  const host = globalThis.__notiaHost;
  delete globalThis.__notiaHost;

  const call = (method, ...args) => JSON.parse(host(method, JSON.stringify(args)));

  const describe = (error) =>
    error instanceof Error
      ? [`${error.name}: ${error.message}`, error.stack].filter(Boolean).join("\n")
      : String(error);

  const report = (error) => call("error", describe(error));

  // Notia cannot see the promise a callback returns, so async failures are
  // reported from here.
  const settle = (result) => {
    if (result && typeof result.then === "function") {
      result.then(undefined, report);
    }
  };

  const expectFunction = (value, what) => {
    if (typeof value !== "function") {
      throw new TypeError(`${what} must be a function`);
    }
    return value;
  };

  // Commands, ribbon buttons and timers, which Notia refers to by number.
  const callbacks = new Map();
  let nextCallback = 1;
  const remember = (callback, what) => {
    const id = nextCallback++;
    callbacks.set(id, expectFunction(callback, what));
    return id;
  };

  const listeners = new Map();
  const listen = (event, callback) => {
    expectFunction(callback, "an event listener");
    let set = listeners.get(event);
    if (!set) {
      set = new Set();
      listeners.set(event, set);
      call("listen", event);
    }
    set.add(callback);
    return { off: () => set.delete(callback) };
  };

  const timers = new Map();
  let nextTimer = 1;
  const startTimer = (callback, delay, args, repeat) => {
    const id = nextTimer++;
    timers.set(id, { callback: expectFunction(callback, "a timer callback"), args, repeat });
    call("setTimer", id, Math.max(0, Number(delay) || 0), repeat);
    return id;
  };
  const stopTimer = (id) => {
    if (timers.delete(id)) {
      call("clearTimer", id);
    }
  };
  globalThis.setTimeout = (callback, delay, ...args) => startTimer(callback, delay, args, false);
  globalThis.setInterval = (callback, delay, ...args) => startTimer(callback, delay, args, true);
  globalThis.clearTimeout = stopTimer;
  globalThis.clearInterval = stopTimer;
  globalThis.queueMicrotask ??= (callback) => {
    Promise.resolve().then(expectFunction(callback, "a microtask"));
  };

  const format = (values) =>
    values
      .map((value) => {
        if (typeof value === "string") return value;
        if (value instanceof Error) return describe(value);
        try {
          return JSON.stringify(value) ?? String(value);
        } catch {
          return String(value);
        }
      })
      .join(" ");
  globalThis.console = Object.freeze({
    log: (...values) => call("log", "info", format(values)),
    info: (...values) => call("log", "info", format(values)),
    debug: (...values) => call("log", "debug", format(values)),
    warn: (...values) => call("log", "warn", format(values)),
    error: (...values) => call("log", "error", format(values)),
  });

  // An element, or a column of them. Used by `dock`, which accepts an array.
  const block = (value) => {
    if (Array.isArray(value)) return { type: "column", children: value };
    if (value == null || typeof value === "boolean") return { type: "column", children: [] };
    if (typeof value !== "object") return { type: "text", text: String(value) };
    return value;
  };

  // A length in px, or `undefined` when the caller left it out. Out of range clamps.
  const px = (value, max) => {
    if (value == null || value === "") return undefined;
    const n = Number(value);
    if (!Number.isFinite(n)) return undefined;
    return Math.min(max, Math.max(0, n));
  };
  const count = (value, min, max, fallback) => {
    const n = Math.trunc(Number(value));
    if (!Number.isFinite(n)) return fallback;
    return Math.min(max, Math.max(min, n));
  };
  // `undefined` drops the key from the JSON, which is how "leave the default" is spelled.
  const lengthOption = (node, key, value, max) => {
    const n = px(value, max);
    if (n === undefined) delete node[key];
    else node[key] = n;
  };

  const ui = Object.freeze({
    column: (children = [], options = {}) => {
      const node = { ...options, type: "column", children };
      lengthOption(node, "gap", options.gap, 256);
      lengthOption(node, "minHeight", options.minHeight, 1024);
      return node;
    },
    /**
     * `columns` (1–31) gives every child an equal track, so cell n lines up
     * with cell n of another row of the same width. Leave it out and this
     * stays a flex row. `rowHeight` is every cell's height, in px, and
     * applies with `columns`. `align: "start"` left-aligns; the default
     * centers. `spread` is what a host that ignores `columns` still honors.
     */
    row: (children = [], options = {}) => {
      const node = { ...options, type: "row", children };
      lengthOption(node, "gap", options.gap, 256);
      lengthOption(node, "rowHeight", options.rowHeight, 1024);
      if (options.columns == null || options.columns === "") delete node.columns;
      else node.columns = count(options.columns, 1, 31, 1);
      if (options.align === "start") node.align = "start";
      else delete node.align;
      return node;
    },
    /** A bordered field. A chat puts the question and its controls in one. */
    composer: (children = []) => ({ type: "composer", children }),
    heading: (text) => ({ type: "heading", text: String(text) }),
    text: (text, options = {}) => ({ ...options, type: "text", text: String(text) }),
    /** A Lucide icon, with no button around it. */
    icon: (name, options = {}) => ({
      type: "icon",
      name: String(name ?? ""),
      muted: options.muted === true,
    }),
    markdown: (source) => ({ type: "markdown", source: String(source) }),
    button: (label, onClick, options = {}) => {
      const requested = options.dot;
      const dotted = requested === true || (typeof requested === "string" && requested !== "");
      const tone = typeof requested === "string" ? requested : options.dotTone;
      const node = {
        ...options,
        type: "button",
        label: String(label),
        onClick: expectFunction(onClick, "a button's onClick"),
      };
      if (options.muted === true) node.muted = true;
      else delete node.muted;
      if (dotted) {
        node.dot = true;
        // `accent` is the default, so it does not need to be sent.
        if (tone === "primaryForeground") node.dotTone = tone;
        else delete node.dotTone;
      } else {
        delete node.dot;
        delete node.dotTone;
      }
      return node;
    },
    list: (items = [], options = {}) => ({
      type: "list",
      items,
      compact: options.compact === true,
    }),
    /**
     * A short label, such as a passage added to the chat. `onClose` draws
     * an × that removes it.
     */
    chip: (label, onClose) => ({
      type: "chip",
      label: String(label),
      onClose: typeof onClose === "function" ? onClose : undefined,
    }),
    separator: () => ({ type: "separator" }),
    empty: (text) => ({ type: "empty", text: String(text) }),
    // Settings tabs: a named row with one of the controls below.
    setting: (name, control, options = {}) => ({ ...options, type: "setting", name: String(name), control }),
    toggle: (value, onChange) => ({
      type: "toggle",
      value: Boolean(value),
      onChange: expectFunction(onChange, "a toggle's onChange"),
    }),
    textInput: (value, onChange, options = {}) => ({
      type: "text_input",
      value: String(value ?? ""),
      placeholder: options.placeholder == null ? "" : String(options.placeholder),
      // Hidden text, with a button that shows the value. For keys and tokens.
      secret: options.secret === true,
      onChange: expectFunction(onChange, "a text input's onChange"),
    }),
    /**
     * A multi-line field. `options.id` keeps the field across renders, and
     * `options.rows` sets how tall it is.
     */
    textArea: (value, onChange, options = {}) => ({
      type: "text_area",
      value: String(value ?? ""),
      rows: Number(options.rows) || 14,
      maxRows: Number(options.maxRows) > 0 ? Number(options.maxRows) : undefined,
      id: options.id == null ? "" : String(options.id),
      placeholder: options.placeholder == null ? "" : String(options.placeholder),
      plain: options.plain === true,
      onChange: expectFunction(onChange, "a text area's onChange"),
      onSubmit: typeof options.onSubmit === "function" ? options.onSubmit : undefined,
    }),
    numberInput: (value, onChange, options = {}) => ({
      ...options,
      type: "number_input",
      value: Number(value) || 0,
      onChange: expectFunction(onChange, "a number input's onChange"),
    }),
    /**
     * A bar of chats. Each tab has a label, and may close. `actions` sit at
     * the end, such as new chat and history.
     */
    tabBar: (tabs = [], actions = []) => ({
      type: "tab_bar",
      tabs: [...tabs].map((tab) => ({
        label: String(tab?.label ?? ""),
        icon: tab?.icon == null ? undefined : String(tab.icon),
        active: tab?.active === true,
        onSelect: typeof tab?.onSelect === "function" ? tab.onSelect : undefined,
        onClose: typeof tab?.onClose === "function" ? tab.onClose : undefined,
      })),
      actions,
    }),
    /**
     * A scrolling region with a footer that stays put. A chat panel uses it
     * so the message list scrolls and the field stays at the bottom.
     * `body` and `footer` are elements or arrays of them. `header` stays
     * above the list, such as a tab bar.
     */
    dock: (body, footer, header) => ({
      type: "dock",
      body: block(body),
      footer: block(footer),
      header: header == null || header === false ? undefined : block(header),
    }),
    dropdown: (value, options, onChange) => ({
      type: "dropdown",
      value: String(value ?? ""),
      options: [...(options ?? [])].map((option) => {
        const [choice, label] = Array.isArray(option)
          ? option
          : option !== null && typeof option === "object"
            ? [option.value, option.label]
            : [option];
        return { value: String(choice), label: String(label ?? choice) };
      }),
      onChange: expectFunction(onChange, "a dropdown's onChange"),
    }),
  });

  // Turns what a view's render function returned into JSON. Functions
  // become numbers that index the view's handlers.
  const serialize = (tree, handlers) => {
    const element = (value) =>
      value !== null && typeof value === "object" ? value : ui.text(String(value));
    const present = (value) => value !== null && value !== undefined && typeof value !== "boolean";
    if (Array.isArray(tree)) tree = ui.column(tree);
    if (!present(tree)) tree = ui.column([]);
    return JSON.stringify(element(tree), (key, value) => {
      if (typeof value === "function") {
        handlers.push(value);
        return handlers.length - 1;
      }
      if ((key === "children" || key === "items") && Array.isArray(value)) {
        return value.filter(present).map(key === "children" ? element : (item) =>
          item !== null && typeof item === "object" ? item : { title: String(item) });
      }
      if (key === "control" && present(value)) return element(value);
      return value;
    });
  };

  // Panels and settings tabs, by id. Settings tabs are `settings:<n>`,
  // which no panel id can be.
  const views = new Map();

  // Counts calls from Notia. An editor reads its note again in each call,
  // since the user may have typed in between.
  let turn = 0;

  const clamp = (value, min, max) => Math.min(max, Math.max(min, Math.trunc(Number(value)) || 0));

  // Positions are 0-based lines and columns, both counted like string
  // indices. `state` is what "editor.get" returned.
  const lineStarts = (state) => {
    if (!state.lineStarts) {
      const starts = [0];
      for (let at = state.text.indexOf("\n"); at !== -1; at = state.text.indexOf("\n", at + 1)) {
        starts.push(at + 1);
      }
      state.lineStarts = starts;
    }
    return state.lineStarts;
  };

  // Where a line's text ends, before its "\n" or "\r\n".
  const lineEnd = (state, line) => {
    const starts = lineStarts(state);
    if (line + 1 >= starts.length) return state.text.length;
    const newline = starts[line + 1] - 1;
    return newline > starts[line] && state.text[newline - 1] === "\r" ? newline - 1 : newline;
  };

  const toOffset = (state, pos) => {
    const starts = lineStarts(state);
    const line = clamp(pos?.line, 0, starts.length - 1);
    return starts[line] + clamp(pos?.ch, 0, lineEnd(state, line) - starts[line]);
  };

  const toPos = (state, offset) => {
    const starts = lineStarts(state);
    offset = clamp(offset, 0, state.text.length);
    let [low, high] = [0, starts.length - 1];
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      if (starts[middle] <= offset) low = middle;
      else high = middle - 1;
    }
    return { line: low, ch: offset - starts[low] };
  };

  // `changes` as offsets into the text before them, applied to `text`.
  const applyChanges = (text, changes) => {
    let result = "";
    let copied = 0;
    for (const change of [...changes].sort((a, b) => a.from - b.from || a.to - b.to)) {
      result += text.slice(copied, change.from) + change.text;
      copied = change.to;
    }
    return result + text.slice(copied);
  };

  const insertAtCursor = Symbol("insertAtCursor");

  // An open note's text, like Obsidian's `Editor`. Edits go to Notia at
  // once as offsets, and Notia includes them in what the editor reads next.
  const makeEditor = (first) => {
    const id = first.id;
    let cached = { turn, state: first };
    const state = () => {
      if (cached?.turn !== turn) {
        const next = call("editor.get", id);
        if (!next) throw new Error(`${first.path} is no longer open`);
        cached = { turn, state: next };
      }
      return cached.state;
    };
    const offset = (pos) => toOffset(state(), pos);
    const pos = (offset) => toPos(state(), offset);
    const selected = () => {
      const { anchor, head } = state();
      return [Math.min(anchor, head), Math.max(anchor, head)];
    };
    const span = (from, to) => {
      const [a, b] = [offset(from), offset(to ?? from)];
      return [Math.min(a, b), Math.max(a, b)];
    };
    // `selection`, if any, is in offsets of the text after the changes.
    const send = (changes, selection = null) => {
      call("editor.edit", id, { changes, selection });
      cached = null;
    };

    const editor = {
      getDoc: () => editor,
      refresh: () => {},
      getValue: () => state().text,
      setValue: (text) => send([{ from: 0, to: state().text.length, text: String(text) }]),
      getLine: (line) => {
        const starts = lineStarts(state());
        const n = Math.trunc(Number(line));
        return n >= 0 && n < starts.length ? state().text.slice(starts[n], lineEnd(state(), n)) : "";
      },
      setLine: (line, text) => {
        const starts = lineStarts(state());
        const n = Math.trunc(Number(line));
        if (n >= 0 && n < starts.length) {
          send([{ from: starts[n], to: lineEnd(state(), n), text: String(text) }]);
        }
      },
      lineCount: () => lineStarts(state()).length,
      lastLine: () => lineStarts(state()).length - 1,
      getSelection: () => state().text.slice(...selected()),
      somethingSelected: () => state().anchor !== state().head,
      getRange: (from, to) => state().text.slice(...span(from, to)),
      replaceSelection: (text) => {
        const [from, to] = selected();
        text = String(text);
        const end = from + text.length;
        send([{ from, to, text }], { anchor: end, head: end });
      },
      replaceRange: (text, from, to) => {
        const [start, end] = span(from, to);
        send([{ from: start, to: end, text: String(text) }]);
      },
      getCursor: (which = "head") => {
        const [from, to] = selected();
        if (which === "from") return pos(from);
        if (which === "to") return pos(to);
        return pos(which === "anchor" ? state().anchor : state().head);
      },
      listSelections: () => [{ anchor: pos(state().anchor), head: pos(state().head) }],
      setCursor: (position, ch) => {
        const at = offset(typeof position === "number" ? { line: position, ch } : position);
        send([], { anchor: at, head: at });
      },
      setSelection: (anchor, head) => send([], { anchor: offset(anchor), head: offset(head ?? anchor) }),
      setSelections: (ranges, main = 0) => {
        const range = ranges?.[main] ?? ranges?.[0];
        if (range) editor.setSelection(range.anchor, range.head);
      },
      transaction: (transaction = {}) => {
        const changes = (transaction.changes ?? []).map((change) => {
          const [from, to] = span(change.from, change.to);
          return { from, to, text: String(change.text ?? "") };
        });
        let selection = null;
        if (transaction.replaceSelection != null) {
          const [from, to] = selected();
          const text = String(transaction.replaceSelection);
          changes.push({ from, to, text });
          const end = from + text.length;
          selection = { anchor: end, head: end };
        }
        if (transaction.selection) {
          const after = { text: applyChanges(state().text, changes) };
          const { from, to } = transaction.selection;
          selection = { anchor: toOffset(after, from), head: toOffset(after, to ?? from) };
        }
        send(changes, selection);
      },
      posToOffset: (position) => offset(position),
      offsetToPos: (at) => pos(at),
      wordAt: (position) => {
        const { text } = state();
        const word = /[\p{L}\p{N}_]/u;
        let from = offset(position);
        let to = from;
        while (from > 0 && word.test(text[from - 1])) from -= 1;
        while (to < text.length && word.test(text[to])) to += 1;
        return from === to ? null : { from: pos(from), to: pos(to) };
      },
      focus: () => call("editor.exec", id, "focus"),
      undo: () => call("editor.exec", id, "undo"),
      redo: () => call("editor.exec", id, "redo"),
      [insertAtCursor]: (text) => {
        const { head } = state();
        const end = head + text.length;
        send([{ from: head, to: head, text }], { anchor: end, head: end });
      },
    };
    return Object.freeze(editor);
  };

  const activeEditor = () => {
    const state = call("editor.get", null);
    return state ? makeEditor(state) : null;
  };

  const app = Object.freeze({
    vault: Object.freeze({
      getName: () => call("vault.name"),
      getNotes: () => call("vault.notes"),
      read: (path) => call("vault.read", String(path)),
      exists: (path) => call("vault.exists", String(path)),
      create: (path, text = "") => call("vault.create", String(path), String(text)),
      modify: (path, text) => call("vault.modify", String(path), String(text)),
      append: (path, text) => call("vault.append", String(path), String(text)),
      on: (event, callback) => listen(`vault:${event}`, callback),
    }),
    workspace: Object.freeze({
      getActiveNote: () => call("workspace.activeNote"),
      getActiveEditor: () => activeEditor(),
      openNote: (path, options = {}) => call("workspace.openNote", String(path), options),
      openLink: (target, options = {}) => call("workspace.openLink", String(target), options),
      newNote: (folder = null) => call("workspace.newNote", folder === null ? null : String(folder)),
      insertText: (text) => {
        const editor = activeEditor();
        if (editor) {
          editor[insertAtCursor](String(text));
          editor.focus();
        }
      },
      revealLine: (line) => call("workspace.revealLine", Number(line) || 0),
      search: (query) => call("workspace.search", String(query)),
      runCommand: (id) => call("workspace.runCommand", String(id)),
      on: (event, callback) => listen(`workspace:${event}`, callback),
    }),
    metadata: Object.freeze({
      getHeadings: (path) => call("metadata.headings", String(path)),
      getLinks: (path) => call("metadata.links", String(path)),
      getBacklinks: (path) => call("metadata.backlinks", String(path)),
      getTags: () => call("metadata.tags"),
      getNotesWithTag: (tag) => call("metadata.notesWithTag", String(tag)),
      resolveLink: (target, from = null) =>
        call("metadata.resolveLink", String(target), from === null ? null : String(from)),
    }),
  });

  let ribbonIcons = 0;
  let settingTabs = 0;

  class Plugin {
    constructor(app, manifest) {
      this.app = app;
      this.manifest = manifest;
    }

    onload() {}

    onunload() {}

    addCommand(command) {
      const { id, name, icon = null, hotkey = null, callback, editorCallback } = command ?? {};
      const run =
        typeof editorCallback === "function"
          ? () => {
              const editor = activeEditor();
              return editor ? editorCallback(editor, app.workspace.getActiveNote()) : undefined;
            }
          : callback;
      call("addCommand", {
        id: String(id ?? ""),
        name: String(name ?? ""),
        icon,
        hotkey,
        needsNote: typeof editorCallback === "function",
        callback: remember(run, "a command's callback"),
      });
      return command;
    }

    addRibbonIcon(icon, title, callback) {
      call("addRibbonIcon", {
        id: `ribbon-${++ribbonIcons}`,
        icon: String(icon),
        title: String(title),
        callback: remember(callback, "a ribbon icon's callback"),
      });
    }

    addStatusBarItem() {
      const id = call("addStatusBarItem");
      const item = {
        setText(text) {
          call("setStatusBarText", id, String(text));
          return item;
        },
      };
      return item;
    }

    addPanel({ id, title, icon = null, side = "right", render } = {}) {
      id = String(id ?? "");
      expectFunction(render, "a panel's render");
      call("addPanel", { id, title: String(title ?? id), icon, side });
      views.set(id, { render, handlers: [] });
      return Object.freeze({
        refresh: () => call("refreshPanel", id),
        show: () => call("showPanel", id),
      });
    }

    addSettingTab({ title = null, icon = null, render } = {}) {
      expectFunction(render, "a settings tab's render");
      const id = `settings:${++settingTabs}`;
      call("addSettingTab", { id, title: title === null ? null : String(title), icon });
      views.set(id, { render, handlers: [] });
      return Object.freeze({ refresh: () => call("refreshSettings", id) });
    }

    registerEvent(ref) {
      return ref;
    }

    registerInterval(id) {
      return id;
    }

    async loadData() {
      return call("loadData");
    }

    async saveData(data) {
      call("saveData", data ?? null);
    }

    /**
     * Reads a text file in the plugin folder, or `null` when it does not
     * exist. The name is relative to the folder.
     */
    readFile(name) {
      return call("readFile", String(name));
    }

    /** Writes a text file in the plugin folder. */
    writeFile(name, text) {
      call("writeFile", String(name), String(text ?? ""));
    }

    /** Opens a file in the plugin folder with the system editor. */
    openFile(name) {
      call("openFile", String(name));
    }

    /**
     * Replaces colors of the active theme. Keys are theme color names, such
     * as `"primary.background"`, or `"highlight.syntax.title.color"`. An
     * empty object clears the replacements. Returns `{ warnings }`.
     */
    setThemeColors(colors) {
      return call("setThemeColors", colors ?? {});
    }

    /**
     * Replaces the icons in the file list. `null` clears them. `folder` and
     * `folderOpen` are Lucide names. Colors are `#hex` or a theme color
     * name. `folders` sets an icon for one path. Returns `{ warnings }`.
     */
    setFileIcons(icons) {
      return call("setFileIcons", icons ?? null);
    }
  }

  class Notice {
    constructor(message) {
      call("notice", String(message));
    }
  }

  // Promises returned by `requestUrl`, resolved when the host finishes.
  const pendingRequests = new Map();

  /**
   * Fetches an http or https address. The promise resolves with
   * `{ status, headers, text }` and rejects if the address is refused or
   * the server cannot be reached. A status such as 404 still resolves.
   * `onChunk` receives each piece of a successful body as it arrives.
   */
  const requestUrl = (options) => {
    const request = options ?? {};
    const headers = {};
    if (request.headers != null && typeof request.headers === "object") {
      for (const [key, value] of Object.entries(request.headers)) {
        if (value != null) headers[String(key)] = String(value);
      }
    }
    return new Promise((resolve, reject) => {
      const id = nextCallback++;
      pendingRequests.set(id, {
        resolve,
        reject,
        onChunk: typeof request.onChunk === "function" ? request.onChunk : null,
      });
      try {
        call("requestUrl", id, {
          url: String(request.url ?? ""),
          method: request.method == null ? "" : String(request.method),
          headers,
          body: request.body == null ? "" : String(request.body),
          timeout: request.timeout == null ? null : Number(request.timeout),
          stream: typeof request.onChunk === "function",
        });
      } catch (error) {
        pendingRequests.delete(id);
        reject(error);
      }
    });
  };

  const api = Object.freeze({ Plugin, Notice, ui, requestUrl });
  Object.defineProperty(globalThis, Symbol.for("notia"), { value: api });

  // Plugins bundled as CommonJS, like most Obsidian plugins, assign
  // `module.exports` and `require("notia")`.
  globalThis.module = { exports: {} };
  globalThis.exports = globalThis.module.exports;
  globalThis.require = (name) => {
    if (name === "notia") return api;
    throw new Error(`Cannot find module "${name}": plugins can only require "notia"`);
  };

  let plugin = null;

  const start = (namespace, manifestJson) => {
    turn += 1;
    const manifest = Object.freeze(JSON.parse(manifestJson));
    const commonjs = globalThis.module.exports;
    const PluginClass = [namespace?.default, commonjs?.default, commonjs].find(
      (value) => typeof value === "function",
    );
    if (!PluginClass) {
      throw new TypeError(
        `${manifest.main ?? "main.js"} must export a plugin class: export default class extends Plugin {}`,
      );
    }
    plugin = new PluginClass(app, manifest);
    return Promise.resolve().then(() => plugin.onload());
  };

  const dispatch = (kind, target, payload) => {
    turn += 1;
    switch (kind) {
      case "callback": {
        const callback = callbacks.get(Number(target));
        if (callback) settle(callback());
        return undefined;
      }
      case "timer": {
        const id = Number(target);
        const timer = timers.get(id);
        if (!timer) return undefined;
        if (!timer.repeat) timers.delete(id);
        settle(timer.callback(...timer.args));
        return undefined;
      }
      case "event": {
        const args = JSON.parse(payload);
        for (const listener of [...(listeners.get(target) ?? [])]) {
          try {
            settle(listener(...args));
          } catch (error) {
            report(error);
          }
        }
        return undefined;
      }
      case "render": {
        const view = views.get(target);
        if (!view) throw new Error(`there is no view "${target}"`);
        const handlers = [];
        const json = serialize(view.render(), handlers);
        view.handlers = handlers;
        return json;
      }
      case "ui": {
        const [index, ...args] = JSON.parse(payload);
        const handler = views.get(target)?.handlers[Number(index)];
        if (handler) settle(handler(...args));
        return undefined;
      }
      case "request-chunk": {
        const pending = pendingRequests.get(Number(target));
        if (!pending?.onChunk) return undefined;
        try {
          pending.onChunk(payload);
        } catch (error) {
          report(error);
        }
        return undefined;
      }
      case "request": {
        const pending = pendingRequests.get(Number(target));
        pendingRequests.delete(Number(target));
        if (!pending) return undefined;
        const result = JSON.parse(payload);
        if (result.ok) pending.resolve(result.response);
        else pending.reject(new Error(result.error || "The request failed."));
        return undefined;
      }
      case "unload": {
        const loaded = plugin;
        plugin = null;
        timers.clear();
        if (loaded) settle(loaded.onunload());
        return undefined;
      }
      default:
        throw new Error(`unknown dispatch "${kind}"`);
    }
  };

  Object.defineProperty(globalThis, "__notiaStart", { value: start });
  Object.defineProperty(globalThis, "__notiaDispatch", { value: dispatch });
})();
