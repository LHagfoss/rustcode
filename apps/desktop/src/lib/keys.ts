// Every keyboard shortcut the app answers, one table: the command's id, its
// label, its default binding and whether Settings may change it. Pure (no
// React, no store, no DOM) so the matcher, the recorder's rules and the
// stored overrides are testable on their own; `state/shortcuts.ts` runs a
// matched command and `ui/ShortcutSettings.tsx` edits the bindings.
//
// A binding is canonical text: modifiers then a key, joined with "+", in the
// order mod, alt, shift — "mod+shift+o", "mod+=", "f6". "mod" is ⌘ on a Mac
// and Ctrl elsewhere. A key that only exists shifted ("+", "?", ":") is
// stored as its unshifted key, so ⌘+ and ⌘= are one binding.

export type ShortcutGroup = "Threads" | "App" | "View" | "Text" | "Composer";

export interface AppCommand {
  /** Stable id; stored overrides are keyed by it. */
  id: string;
  label: string;
  /** Default binding, canonical; "" when the command has none of its own. */
  binding: string;
  /** Shown when the binding is not a key of its own (the approval digits). */
  display?: string;
  group: ShortcutGroup;
  /** False for the keys the composer owns, which Settings lists but does not change. */
  remappable: boolean;
  /**
   * True for a key the thread list answers while one of its rows has the keyboard: the window
   * never fires it, so a bare letter cannot type over the composer or a field.
   */
  list?: boolean;
}

export const APP_COMMANDS: readonly AppCommand[] = [
  { id: "new-thread", label: "New thread", binding: "mod+n", group: "Threads", remappable: true },
  {
    id: "thread-folder",
    label: "Folder of this thread (a new one, or move this one)",
    binding: "mod+o",
    group: "Threads",
    remappable: true,
  },
  {
    id: "default-folder",
    label: "Default folder for new threads",
    binding: "mod+shift+o",
    group: "Threads",
    remappable: true,
  },
  { id: "stop-turn", label: "Stop the turn", binding: "mod+.", group: "Threads", remappable: true },
  {
    id: "pin-thread",
    label: "Pin or unpin the highlighted thread",
    binding: "p",
    group: "Threads",
    remappable: true,
    list: true,
  },
  {
    id: "settle-thread",
    label: "Settle the highlighted thread, or unsettle it",
    binding: "d",
    group: "Threads",
    remappable: true,
    list: true,
  },
  {
    id: "command-palette",
    label: "Command palette: threads, actions",
    binding: "mod+k",
    group: "App",
    remappable: true,
  },
  { id: "settings", label: "Settings", binding: "mod+,", group: "App", remappable: true },
  { id: "inbox", label: "Needs you inbox", binding: "mod+shift+i", group: "View", remappable: true },
  { id: "toggle-sidebar", label: "Toggle sidebar", binding: "mod+b", group: "View", remappable: true },
  {
    id: "toggle-panel",
    label: "Toggle the right panel",
    binding: "mod+alt+b",
    group: "View",
    remappable: true,
  },
  { id: "find-in-thread", label: "Find in thread", binding: "mod+f", group: "View", remappable: true },
  { id: "toggle-terminal", label: "Terminal", binding: "mod+j", group: "View", remappable: true },
  { id: "zoom-in", label: "Text larger", binding: "mod+=", group: "Text", remappable: true },
  { id: "zoom-out", label: "Text smaller", binding: "mod+-", group: "Text", remappable: true },
  { id: "zoom-actual", label: "Text at its actual size", binding: "mod+0", group: "Text", remappable: true },
  {
    id: "composer-send",
    label: "Send · queue while running",
    binding: "enter",
    group: "Composer",
    remappable: false,
  },
  {
    id: "composer-steer",
    label: "Steer the running turn",
    binding: "mod+enter",
    group: "Composer",
    remappable: false,
  },
  {
    id: "composer-newline",
    label: "New line",
    binding: "shift+enter",
    group: "Composer",
    remappable: false,
  },
  {
    id: "composer-escape",
    label: "Close find, a menu, or the composer's pickers",
    binding: "escape",
    group: "Composer",
    remappable: false,
  },
  {
    id: "answer-approval",
    label: "Answer an approval",
    binding: "",
    display: "1–9",
    group: "Composer",
    remappable: false,
  },
];

/** The commands Settings lets the user change, in table order. */
export const CHANGEABLE_COMMANDS = APP_COMMANDS.filter((command) => command.remappable);

/** Key tokens a binding may name besides a single character. */
const KEY_NAMES = new Set([
  "enter",
  "escape",
  "tab",
  "space",
  "backspace",
  "delete",
  "home",
  "end",
  "pageup",
  "pagedown",
  "up",
  "down",
  "left",
  "right",
  "insert",
  ...Array.from({ length: 24 }, (_, i) => `f${i + 1}`),
]);

/** Characters that only exist with Shift held: stored as the key under them. */
const UNSHIFTED: Record<string, string> = {
  "+": "=",
  _: "-",
  "!": "1",
  "@": "2",
  "#": "3",
  $: "4",
  "%": "5",
  "^": "6",
  "&": "7",
  "*": "8",
  "(": "9",
  ")": "0",
  ":": ";",
  '"': "'",
  "<": ",",
  ">": ".",
  "?": "/",
  "~": "`",
  "{": "[",
  "}": "]",
  "|": "\\",
};

/** Keys that are a modifier on their own: a keydown of them starts nothing. */
const MODIFIERS = new Set([
  "shift",
  "control",
  "alt",
  "meta",
  "capslock",
  "fn",
  "fnlock",
  "hyper",
  "super",
  "symbol",
  "symbollock",
  "os",
  "numlock",
  "scrolllock",
]);

/** A binding a handler compares: canonical text, "" when there is none. */
export interface Binding {
  mod: boolean;
  alt: boolean;
  shift: boolean;
  key: string;
}

/** The key a keydown names, and whether Shift was part of typing it. */
function eventKey(event: KeyEventLike): { key: string; shifted: boolean } | null {
  const raw = event.key;
  if (!raw || raw === "Unidentified" || raw === "Dead" || raw === "Process") return null;
  const lower = raw.toLowerCase();
  if (MODIFIERS.has(lower)) return null;
  if (raw.length === 1) {
    const base = UNSHIFTED[raw];
    if (base) return { key: base, shifted: false };
    return { key: lower, shifted: true };
  }
  return KEY_NAMES.has(lower) ? { key: lower, shifted: true } : null;
}

export interface KeyEventLike {
  key: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}

/**
 * The canonical binding a keydown is, or null for a modifier on its own or a
 * key the table does not name.
 */
export function bindingFromEvent(event: KeyEventLike): string | null {
  const named = eventKey(event);
  if (!named) return null;
  const parts: string[] = [];
  if (event.metaKey || event.ctrlKey) parts.push("mod");
  if (event.altKey) parts.push("alt");
  // Shift is part of a letter or digit ("mod+shift+o"), and of any named key
  // ("shift+enter"); on punctuation it is how the character is typed, not a
  // modifier of its own.
  if (event.shiftKey && named.shifted && (named.key.length > 1 || /^[a-z0-9]$/.test(named.key))) {
    parts.push("shift");
  }
  parts.push(named.key);
  return parts.join("+");
}

/** A binding's parts, or null when the text is not a binding this app can use. */
export function parseBinding(text: string): Binding | null {
  if (!text) return null;
  const normalized = text
    .trim()
    .toLowerCase()
    .replaceAll("⌘", "mod+")
    .replaceAll("⌥", "alt+")
    .replaceAll("⇧", "shift+")
    .replaceAll("⌃", "mod+");
  const tokens = normalized.split("+");
  const key = tokens.pop() ?? "";
  if (!key) return null;
  const binding: Binding = { mod: false, alt: false, shift: false, key };
  if (key.length > 1 && !KEY_NAMES.has(key)) return null;
  if (key === "+") return null;
  for (const token of tokens) {
    if (token === "mod" || token === "cmd" || token === "command" || token === "meta" || token === "ctrl") {
      binding.mod = true;
    } else if (token === "alt" || token === "opt" || token === "option") binding.alt = true;
    else if (token === "shift") binding.shift = true;
    else return null;
  }
  return binding;
}

const KEY_LABELS: Record<string, string> = {
  enter: "⏎",
  escape: "Esc",
  tab: "⇥",
  space: "Space",
  backspace: "⌫",
  delete: "⌦",
  up: "↑",
  down: "↓",
  left: "←",
  right: "→",
  pageup: "⇞",
  pagedown: "⇟",
  "=": "+",
};

/** How a binding reads in the app: "⌘⇧O", "⌘⏎". */
export function formatBinding(binding: string): string {
  const parsed = parseBinding(binding);
  if (!parsed) return binding;
  const parts: string[] = [];
  if (parsed.mod) parts.push("⌘");
  if (parsed.alt) parts.push("⌥");
  if (parsed.shift) parts.push("⇧");
  const key = KEY_LABELS[parsed.key] ?? parsed.key.toUpperCase();
  parts.push(key);
  return parts.join("");
}

/** The shortcut to show for a command, its own display text when it has none. */
export function commandHint(binding: string, command?: AppCommand): string {
  return command?.display ?? formatBinding(binding);
}

/** Combos the system or the app's own menu owns; a remap may not take them. */
export const RESERVED_BINDINGS: readonly string[] = ["mod+q", "mod+w", "mod+h", "mod+m", "mod+tab"];

/**
 * The editing keys every text field has. The window's menu or the webview
 * answers them before a page sees the keydown, so a remap to one would never
 * fire: refused rather than stored and dead.
 */
export const RESERVED_EDITING: readonly string[] = [
  "mod+c",
  "mod+v",
  "mod+x",
  "mod+a",
  "mod+z",
  "mod+shift+z",
];

/** Alt and ⌘ are modifiers; Shift alone (or nothing) is not enough. */
function hasModifier(binding: Binding): boolean {
  return binding.mod || binding.alt;
}

function isFunctionKey(binding: Binding): boolean {
  return /^f([1-9]|1[0-9]|2[0-4])$/.test(binding.key);
}

/** The bindings in force: every command's default with the overrides on top. */
export function bindingsFor(overrides: Readonly<Record<string, string>>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const command of APP_COMMANDS) out[command.id] = overrides[command.id] ?? command.binding;
  return out;
}

/** One command's binding, override or default. */
export function bindingOf(overrides: Readonly<Record<string, string>>, id: string): string {
  const command = APP_COMMANDS.find((c) => c.id === id);
  return overrides[id] ?? command?.binding ?? "";
}

/** True for a command only the thread list answers while a row has its keyboard. */
function isListKey(id: string): boolean {
  return APP_COMMANDS.some((command) => command.id === id && command.list === true);
}

/** The commands whose binding would be the same key, the one being changed aside. */
export function conflictingCommands(
  overrides: Readonly<Record<string, string>>,
  id: string,
  binding: string,
): AppCommand[] {
  const wanted = parseBinding(binding);
  if (!wanted) return [];
  const bindings = bindingsFor(overrides);
  return APP_COMMANDS.filter((command) => {
    if (command.id === id) return false;
    const other = parseBinding(bindings[command.id] ?? "");
    return (
      other !== null &&
      other.key === wanted.key &&
      other.mod === wanted.mod &&
      other.alt === wanted.alt &&
      other.shift === wanted.shift
    );
  });
}

/**
 * Why a binding cannot be used for `id`, in words, or null when it can: the
 * system owns it, it has no modifier and is no function key, or another
 * command already answers it.
 */
export function bindingProblem(
  overrides: Readonly<Record<string, string>>,
  id: string,
  binding: string,
): string | null {
  const parsed = parseBinding(binding);
  if (!parsed) return "That is not a key this app can use.";
  const text = formatBinding(binding);
  if (RESERVED_BINDINGS.includes(binding)) return `${text} belongs to macOS.`;
  if (RESERVED_EDITING.includes(binding)) return `${text} belongs to copy, paste and undo.`;
  // A list key is typed where nothing else takes it (a row has the keyboard), so a bare letter
  // is usable for one; every other command needs ⌘/⌥ or a function key.
  if (!hasModifier(parsed) && !isFunctionKey(parsed) && !isListKey(id)) {
    return `Add ⌘ to it, or use a function key like F6.`;
  }
  const [taken] = conflictingCommands(overrides, id, binding);
  if (taken) return `${taken.label} already uses ${text}. Reset that one first.`;
  return null;
}

/**
 * The command a keydown runs app-wide, or null: only commands Settings can change fire here, and
 * the list's own marking keys are left to the thread list (see `listCommandFor`).
 */
export function commandFor(overrides: Readonly<Record<string, string>>, event: KeyEventLike): string | null {
  const pressed = bindingFromEvent(event);
  if (!pressed) return null;
  const bindings = bindingsFor(overrides);
  const command = CHANGEABLE_COMMANDS.find((c) => !c.list && bindings[c.id] === pressed);
  return command?.id ?? null;
}

/**
 * The thread list's marking key a keydown is, or null: P (pin) and D (settle) are answered by
 * the row the list has focus on, never by the window, and follow a remap like every other key.
 */
export function listCommandFor(
  overrides: Readonly<Record<string, string>>,
  event: KeyEventLike,
): string | null {
  const pressed = bindingFromEvent(event);
  if (!pressed) return null;
  const bindings = bindingsFor(overrides);
  const command = CHANGEABLE_COMMANDS.find((c) => c.list && bindings[c.id] === pressed);
  return command?.id ?? null;
}

/** Where the overrides live: one JSON object of id → binding, nothing else. */
export const SHORTCUTS_KEY = "rustcode.shortcuts";

/**
 * Overrides from stored JSON. Anything unexpected is dropped rather than
 * thrown: a corrupt or hand-edited value leaves the defaults in place, an
 * unknown id is ignored, and a binding that is unusable or already taken by an
 * earlier command keeps that command's binding.
 */
export function parseOverrides(raw: string | null | undefined): Record<string, string> {
  if (!raw) return {};
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return {};
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) return {};
  const out: Record<string, string> = {};
  for (const command of CHANGEABLE_COMMANDS) {
    const stored = (value as Record<string, unknown>)[command.id];
    if (typeof stored !== "string") continue;
    const parsed = parseBinding(stored);
    if (!parsed) continue;
    const binding = bindingFromParts(parsed);
    if (RESERVED_BINDINGS.includes(binding) || RESERVED_EDITING.includes(binding)) continue;
    if (!hasModifier(parsed) && !isFunctionKey(parsed) && !command.list) continue;
    if (conflictingCommands(out, command.id, binding).length > 0) continue;
    // A bare default is not an override: the table stays the source of it.
    if (binding === command.binding) continue;
    out[command.id] = binding;
  }
  return out;
}

/** A parsed binding back in canonical text. */
function bindingFromParts(binding: Binding): string {
  const parts: string[] = [];
  if (binding.mod) parts.push("mod");
  if (binding.alt) parts.push("alt");
  if (binding.shift) parts.push("shift");
  parts.push(binding.key);
  return parts.join("+");
}
