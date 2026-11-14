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

  const ui = Object.freeze({
    column: (children = [], options = {}) => ({ ...options, type: "column", children }),
    row: (children = [], options = {}) => ({ ...options, type: "row", children }),
    heading: (text) => ({ type: "heading", text: String(text) }),
    text: (text, options = {}) => ({ ...options, type: "text", text: String(text) }),
    markdown: (source) => ({ type: "markdown", source: String(source) }),
    button: (label, onClick, options = {}) => ({
      ...options,
      type: "button",
      label: String(label),
      onClick: expectFunction(onClick, "a button's onClick"),
    }),
    list: (items = []) => ({ type: "list", items }),
    separator: () => ({ type: "separator" }),
    empty: (text) => ({ type: "empty", text: String(text) }),
  });

  // Turns what a panel's render function returned into JSON. Functions
  // become numbers that index the panel's handlers.
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
      return value;
    });
  };

  const panels = new Map();

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
      openNote: (path, options = {}) => call("workspace.openNote", String(path), options),
      openLink: (target, options = {}) => call("workspace.openLink", String(target), options),
      newNote: (folder = null) => call("workspace.newNote", folder === null ? null : String(folder)),
      insertText: (text) => call("workspace.insertText", String(text)),
      selection: () => call("workspace.selection"),
      replaceSelection: (text, expandBefore = 0, expandAfter = 0) =>
        call("workspace.replaceSelection", String(text), Number(expandBefore) || 0, Number(expandAfter) || 0),
      undo: () => call("workspace.undo"),
      redo: () => call("workspace.redo"),
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
              const note = app.workspace.getActiveNote();
              return note ? editorCallback(note) : undefined;
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
      panels.set(id, { render, handlers: [] });
      return Object.freeze({
        refresh: () => call("refreshPanel", id),
        show: () => call("showPanel", id),
      });
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
  }

  class Notice {
    constructor(message) {
      call("notice", String(message));
    }
  }

  const api = Object.freeze({ Plugin, Notice, ui });
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
        const panel = panels.get(target);
        if (!panel) throw new Error(`there is no panel "${target}"`);
        const handlers = [];
        const json = serialize(panel.render(), handlers);
        panel.handlers = handlers;
        return json;
      }
      case "ui": {
        const handler = panels.get(target)?.handlers[Number(payload)];
        if (handler) settle(handler());
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
