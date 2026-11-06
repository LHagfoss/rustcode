// The design system's guard rails (docs/design.md): no raw colours or
// palette classes outside the tokens, no retired class names, no arbitrary
// radii, no native selects, hand-timed hover, hand-made blur or menu items,
// no hand-styled labelled buttons, and index.css's built-in themes equal to
// BUILTIN_THEMES.

import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { BUILTIN_THEMES, cssName, TOKENS } from "./lib/themes";

const SRC = import.meta.dir;

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return files(path);
    return /\.(ts|tsx)$/.test(name) && !name.endsWith(".test.ts") ? [path] : [];
  });
}

const RAW_COLOR = /#[0-9a-fA-F]{3,8}\b(?![-\w])|\b(?:rgba?|hsla?|oklch)\(/;
const PALETTE =
  /(?<![\w-])(?:bg|text|border|ring|from|to|via|fill|stroke|outline|divide|decoration|caret|accent|placeholder|shadow)-(?:white|black|zinc|neutral|stone|gray|slate|red|blue|green|amber|orange|yellow|emerald|sky|indigo|violet|purple|pink|rose|teal|cyan|lime)(?![\w-])|-\[(?:#|rgb|hsl)/;
const RETIRED =
  /(?<![\w-])(?:bg|text|border|ring|from|to|via|fill|stroke|outline|divide|accent|placeholder)-(?:fg-[234]|fill-[1-4]|hairline|brand|brand-fill|chat|field|attention|info)(?![\w-])|(?<![\w-])shadow-(?:float|panel)(?![\w-])|--rc-(?:fg|chat|hairline|brand|attention|red|green|orange|blue|field|fill-[1-4])(?![\w-])/;
const ARBITRARY_RADIUS = /(?<![\w-])rounded(?:-[trblse]{1,2})?-(?:\[|4xl)/;
const NATIVE_SELECT = /<select\b/;
const HAND_TIMED_HOVER = /(?<![\w-])transition-colors(?![\w-])/;
const HAND_MADE_BLUR = /(?<![\w-])backdrop-blur/;
const HAND_MADE_MENU_ITEM = /data-\[highlighted\]:/;
// A labelled button is a `Pill` (docs/design.md › Component recipes): its own resting
// fill plus a `rounded-md`/`lg`/`full` shape is a hand-made one. Whether a tag also has
// text cannot be told here, so the two class families alone are the signal.
const FILLED = /(?<![\w:-])bg-fill(?:-subtle|-strong)?(?![\w-])/;
const SHAPED = /(?<![\w-])rounded-(?:md|lg|full)(?![\w-])/;

/**
 * Tags with a fill of their own that are not labelled actions: list rows, menu items,
 * choice rows, icon buttons, switches, toggles and popover triggers whose fill marks
 * state. Keyed by the file and a part of the tag that names it, with why; a stale entry
 * fails the test instead of quietly allowing the next hand-styled button.
 */
const PILL_ALLOWED: [file: string, part: string, why: string][] = [
  ["ui/AttentionCards.tsx", "answerQuestionOptions(request, [o])", "the answers are choice rows"],
  ["ui/Board.tsx", "Clear the status", "a 24px icon button"],
  ["ui/ChatView.tsx", "px-2.5 font-medium", "the plan pill is a popover trigger"],
  ["ui/ChatView.tsx", "Thread status:", "the status pill is a menu trigger"],
  ["ui/Composer.tsx", 'role="option"', "a slash-command menu row"],
  ["ui/Composer.tsx", "aria-pressed={wt.on}", "a worktree toggle; its fill is its state"],
  ["ui/Composer.tsx", "Attach and more", "a 28px icon button"],
  ["ui/Composer.tsx", "Show all", "the +N thumbnail tile, sized and shaped as the pictures beside it"],
  ["ui/Review.tsx", "aria-current={selected === path", "a file row in the review pane"],
  ["ui/TurnChanges.tsx", "bg-fill-subtle", "the changes pill is a popover trigger"],
  ["ui/ModelPicker.tsx", 'role="switch"', "the fast-mode switch"],
  ["ui/ModelPicker.tsx", "Model and effort", "the model pill is a trigger, and not in this pass"],
  ["ui/ModelPicker.tsx", "aria-pressed={current}", "an account pill, whose fill is its state"],
  ["ui/Settings.tsx", 'role="option"', "a settings result row"],
  ["ui/Settings.tsx", "aria-current={s.id === section", "a settings section row"],
];

/** Every `<button` opening tag in a file: tags span lines and hold `>` in expressions. */
function buttonTags(text: string): string[] {
  const tags: string[] = [];
  for (let at = text.indexOf("<button"); at >= 0; at = text.indexOf("<button", at + 7)) {
    let depth = 0;
    let quote = "";
    let end = at + 7;
    for (; end < text.length; end++) {
      const c = text[end];
      if (quote) {
        if (c === quote) quote = "";
      } else if (c === '"' || c === "'" || c === "`") quote = c;
      else if (c === "{") depth++;
      else if (c === "}") depth--;
      else if (c === ">" && depth === 0) break;
    }
    tags.push(text.slice(at, end + 1));
  }
  return tags;
}

/** A tag's class names: the string, or whatever a `{…}` expression names. */
function classesOf(tag: string): string {
  const at = tag.indexOf("className=");
  if (at < 0) return "";
  const start = at + "className=".length;
  if (tag[start] === '"') return tag.slice(start + 1, tag.indexOf('"', start + 1));
  if (tag[start] !== "{") return "";
  let depth = 0;
  for (let i = start; i < tag.length; i++) {
    if (tag[i] === "{") depth++;
    else if (tag[i] === "}" && --depth === 0) return tag.slice(start + 1, i);
  }
  return "";
}

describe("design system", () => {
  const violations: string[] = [];
  for (const path of files(SRC)) {
    const text = readFileSync(path, "utf8");
    if (text.startsWith("// design-lint: allow-file")) continue;
    const lines = text.split("\n");
    lines.forEach((line, i) => {
      // `design-lint: allow (why)` on the line, or on the line before it.
      if (line.includes("design-lint: allow") || lines[i - 1]?.includes("design-lint: allow")) return;
      const where = `${relative(SRC, path)}:${i + 1}`;
      if (RAW_COLOR.test(line)) violations.push(`${where} raw colour: ${line.trim()}`);
      if (PALETTE.test(line)) violations.push(`${where} palette class: ${line.trim()}`);
      if (RETIRED.test(line)) violations.push(`${where} retired token: ${line.trim()}`);
      if (ARBITRARY_RADIUS.test(line)) violations.push(`${where} arbitrary radius: ${line.trim()}`);
      if (NATIVE_SELECT.test(line)) violations.push(`${where} native select, use Select: ${line.trim()}`);
      if (HAND_TIMED_HOVER.test(line))
        violations.push(`${where} hover is timed in index.css: ${line.trim()}`);
      if (HAND_MADE_BLUR.test(line))
        violations.push(`${where} hand-made blur, use the glass: ${line.trim()}`);
      if (HAND_MADE_MENU_ITEM.test(line) && !path.endsWith("primitives.tsx"))
        violations.push(`${where} hand-made menu item, use menuItem: ${line.trim()}`);
    });
  }

  test("components use tokens only", () => {
    expect(violations).toEqual([]);
  });

  test("labelled buttons are Pills, not hand-styled fills", () => {
    const found: string[] = [];
    const used = new Set(PILL_ALLOWED);
    for (const path of files(SRC)) {
      if (path.endsWith("primitives.tsx")) continue;
      const file = relative(SRC, path);
      for (const tag of buttonTags(readFileSync(path, "utf8"))) {
        const classes = classesOf(tag);
        if (!FILLED.test(classes) || !SHAPED.test(classes)) continue;
        const allowed = PILL_ALLOWED.find(([at, part]) => at === file && tag.includes(part));
        if (allowed) used.delete(allowed);
        else found.push(`${file}: a filled, rounded button: ${classes.replace(/\s+/g, " ").trim()}`);
      }
    }
    expect(found).toEqual([]);
    expect([...used].map(([, , why]) => `allowlist entry no longer matches a tag: ${why}`)).toEqual([]);
  });

  test("index.css's built-in themes match lib/themes.ts", () => {
    const css = readFileSync(join(SRC, "index.css"), "utf8");
    const block = (selector: string) => {
      const start = css.indexOf(selector);
      expect(start).toBeGreaterThanOrEqual(0);
      return css.slice(start, css.indexOf("}", start));
    };
    const light = block(":root {\n  color-scheme: light;");
    const dark = block(':root[data-theme="dark"] {');
    const media = block(':root:not([data-theme="light"]) {');
    for (const [theme, blocks] of [
      [BUILTIN_THEMES.find((t) => t.mode === "light")!, [light]],
      [BUILTIN_THEMES.find((t) => t.mode === "dark")!, [dark, media]],
    ] as const) {
      for (const b of blocks)
        for (const t of TOKENS) expect(b).toContain(`${cssName(t)}: ${theme.tokens[t]};`);
    }
  });
});
