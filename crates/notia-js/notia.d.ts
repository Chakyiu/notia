// The plugin API of Notia, for community plugins written in JavaScript or
// TypeScript. Reference it from a plugin for editor completions:
//
//   /// <reference path="../notia.d.ts" />
//
// A plugin is a folder in `<vault>/.notia/plugins` with a `manifest.json`
// and a `main.js` whose default export extends `Plugin`. Notia reads only
// the manifest until the plugin is enabled in Settings → Plugins, then loads
// `main.js` and calls `onload`. Disabling the plugin calls `onunload` and
// removes everything it added. Saving its files reloads it.

declare module "notia" {
  /** The contents of `manifest.json`, plus the plugin's folder. */
  export interface PluginManifest {
    id: string;
    name: string;
    version?: string;
    author?: string;
    description?: string;
    /** The entry module, relative to the plugin folder. Defaults to `main.js`. */
    main?: string;
    /** The plugin folder, relative to the vault: `.notia/plugins/<folder>`. */
    dir: string;
    [key: string]: unknown;
  }

  /**
   * The base class of plugins. Register commands, ribbon icons, status bar
   * items and panels in `onload`; Notia removes them when the plugin is
   * disabled.
   */
  export class Plugin {
    constructor(app: App, manifest: PluginManifest);
    readonly app: App;
    readonly manifest: PluginManifest;

    /** Called when the plugin is enabled. May be async. */
    onload(): void | Promise<void>;
    /** Called when the plugin is disabled or reloaded. */
    onunload(): void;

    /** Adds a command to the command palette. */
    addCommand(command: Command): Command;
    /**
     * Adds a button to the ribbon, the strip of icons on the left.
     * @param icon A Lucide icon name, such as `"dices"`: see https://lucide.dev/icons.
     */
    addRibbonIcon(icon: string, title: string, callback: () => unknown): void;
    /** Adds a text item to the status bar. */
    addStatusBarItem(): StatusBarItem;
    /** Adds a panel to a sidebar, drawn from the elements `render` returns. */
    addPanel(options: PanelOptions): PanelHandle;

    /** Returns `ref`. Event listeners are removed on unload anyway. */
    registerEvent<T>(ref: T): T;
    /** Returns `id`. Intervals are cleared on unload anyway. */
    registerInterval(id: number): number;

    /** Reads `data.json` in the plugin folder, or `null` when there is none. */
    loadData(): Promise<any>;
    /** Writes `data` to `data.json` in the plugin folder. */
    saveData(data: unknown): Promise<void>;
  }

  /** Shows a notification. */
  export class Notice {
    constructor(message: string);
  }

  export interface Command {
    /** Unique within the plugin. */
    id: string;
    name: string;
    /** A Lucide icon name. */
    icon?: string;
    /**
     * Keystrokes such as `"secondary-shift-k"`, where `secondary` is ⌘ on
     * macOS and Ctrl elsewhere. Modifiers: `cmd`, `ctrl`, `alt`, `shift`,
     * `secondary`.
     */
    hotkey?: string;
    callback?: () => unknown;
    /**
     * Instead of `callback`: called with the active note, and listed only
     * while a note is open.
     */
    editorCallback?: (note: ActiveNote) => unknown;
  }

  export interface StatusBarItem {
    setText(text: string): StatusBarItem;
  }

  export interface PanelOptions {
    /** Unique within the plugin. */
    id: string;
    title: string;
    /** A Lucide icon name, shown in the sidebar header. */
    icon?: string;
    /**
     * Defaults to `"right"`. `"editor"` draws the panel as a bar above the
     * open note instead of docking it in a sidebar.
     */
    side?: "left" | "right" | "editor";
    /**
     * Returns what the panel shows. Notia calls it when the panel is first
     * shown, and again after `refresh()`.
     */
    render(): Element | Element[] | string | null;
  }

  export interface PanelHandle {
    /** Renders the panel again. */
    refresh(): void;
    /** Opens the panel's sidebar and selects the panel. */
    show(): void;
  }

  /** The building blocks of panels. */
  export const ui: {
    column(children: Child[], options?: object): Element;
    row(children: Child[], options?: object): Element;
    /** A small section heading, like those of the core panels. */
    heading(text: string): Element;
    text(text: string, options?: { muted?: boolean; bold?: boolean; small?: boolean }): Element;
    markdown(source: string): Element;
    button(
      label: string,
      onClick: () => unknown,
      options?: {
        icon?: string;
        variant?: "outline" | "primary" | "ghost";
        /** Shown on hover. An empty label with an icon is an icon button. */
        tooltip?: string;
      },
    ): Element;
    list(items: (ListItem | string)[]): Element;
    separator(): Element;
    /** A centered, muted message for when there is nothing to show. */
    empty(text: string): Element;
  };

  /** An element made by `ui`. */
  export interface Element {
    type: string;
  }

  /** Children may also be strings, and `null`, `undefined` or booleans, which are left out. */
  export type Child = Element | string | null | undefined | boolean;

  export interface ListItem {
    title: string;
    subtitle?: string;
    /** A Lucide icon name. */
    icon?: string;
    onClick?: () => unknown;
  }

  export interface App {
    vault: Vault;
    workspace: Workspace;
    metadata: Metadata;
  }

  /** An event listener registration. */
  export interface EventRef {
    off(): void;
  }

  /**
   * The notes of the vault. Paths are relative to the vault, like
   * `Folder/Note.md`; `.md` may be left out. Hidden files are out of reach.
   */
  export interface Vault {
    getName(): string;
    /** Every note, sorted by path. */
    getNotes(): NoteInfo[];
    /** The note's text on disk. */
    read(path: string): string;
    exists(path: string): boolean;
    /** Creates a note and returns its path. Fails if it exists. */
    create(path: string, text?: string): string;
    /** Replaces the text of an existing note. */
    modify(path: string, text: string): void;
    /** Adds text on a new line at the end of a note, creating it if needed. */
    append(path: string, text: string): void;

    /** A note was created or changed, by Notia or on disk. */
    on(event: "modify", callback: (path: string) => unknown): EventRef;
    on(event: "rename", callback: (path: string, oldPath: string) => unknown): EventRef;
    on(event: "delete", callback: (path: string) => unknown): EventRef;
    /** Anything in the vault changed. */
    on(event: "change", callback: () => unknown): EventRef;
  }

  export interface NoteInfo {
    path: string;
    /** The file name without `.md`. */
    name: string;
    tags: string[];
    words: number;
    /** Milliseconds since 1970. */
    modified: number | null;
  }

  export interface ActiveNote {
    path: string;
    name: string;
    /** The text in the editor, including unsaved edits. */
    text: string;
    /** The 0-based line of the cursor. */
    line: number;
    mode: "editing" | "half" | "preview";
  }

  export interface OpenOptions {
    /** The 0-based line to place the cursor on. */
    line?: number;
    newTab?: boolean;
  }

  export interface Workspace {
    getActiveNote(): ActiveNote | null;
    openNote(path: string, options?: OpenOptions): void;
    /** Opens what a `[[link]]` points at, creating the note if needed. */
    openLink(target: string, options?: { newTab?: boolean }): void;
    /** Creates a note in `folder`, or the configured folder, and opens it. */
    newNote(folder?: string | null): void;
    /** Inserts text at the cursor of the active note. */
    insertText(text: string): void;
    /** Moves the cursor of the active note to a 0-based line. */
    revealLine(line: number): void;
    /** Searches the vault, like `"tag:#ideas"` or `"meeting"`. */
    search(query: string): void;
    /** Runs a command by id; ids without `plugin:` are this plugin's. */
    runCommand(id: string): void;

    /** Another note, or no note, became active. */
    on(event: "file-open", callback: (note: ActiveNote | null) => unknown): EventRef;
    /** The text of the active note changed. */
    on(event: "editor-change", callback: (note: ActiveNote) => unknown): EventRef;
  }

  export interface Metadata {
    getHeadings(path: string): { level: number; text: string; line: number }[];
    getLinks(path: string): {
      target: string;
      subpath: string | null;
      display: string | null;
      line: number;
      embed: boolean;
      /** The note the link points at, or `null` if it does not exist. */
      resolved: string | null;
    }[];
    getBacklinks(path: string): { source: string; line: number; context: string }[];
    /** Every tag, without `#`, with the number of notes using it, most used first. */
    getTags(): { tag: string; count: number }[];
    getNotesWithTag(tag: string): string[];
    /** The note a link target means, seen from the note `from`. */
    resolveLink(target: string, from?: string | null): string | null;
  }

  const api: { Plugin: typeof Plugin; Notice: typeof Notice; ui: typeof ui };
  export default api;
}
