// The design system's guard rails (docs/design.md): no raw colours or
// palette classes outside the tokens, no retired class names, no arbitrary
// radii, no native selects, hand-timed hover, hand-made blur or menu items,
// no hand-styled labelled buttons, no spacing off the scale, and index.css's
// built-in themes equal to BUILTIN_THEMES.

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
  ["ui/Board.tsx", "Unsettle", "a 24px icon button"],
  ["ui/ChatView.tsx", "px-2 font-medium", "the plan pill is a popover trigger"],
  ["ui/ChatView.tsx", "Thread status:", "the status pill is a menu trigger"],
  ["ui/Composer.tsx", 'role="option"', "a slash-command menu row"],
  ["ui/Composer.tsx", "aria-pressed={wt.on}", "a worktree toggle; its fill is its state"],
  ["ui/Composer.tsx", "Attach and more", "a round icon button, the send button's size"],
  ["ui/Composer.tsx", "Show all", "the +N thumbnail tile, sized and shaped as the pictures beside it"],
  ["ui/Review.tsx", "aria-current={selected === path", "a file row in the review pane"],
  ["ui/TurnChanges.tsx", "bg-fill-subtle", "the changes pill is a popover trigger"],
  ["ui/TurnTrace.tsx", "bg-fill-subtle", "the trace pill is a popover trigger"],
  ["ui/ModelPicker.tsx", 'role="switch"', "the fast-mode switch"],
  ["ui/ModelPicker.tsx", "Model and effort", "the model pill is a trigger, and not in this pass"],
  ["ui/ModelPicker.tsx", "aria-pressed={chip.selected}", "an account chip, whose fill is its state"],
  ["ui/Settings.tsx", 'role="option"', "a settings result row"],
  ["ui/ShortcutSettings.tsx", "recording ?", "the keycap button; its fill marks recording"],
  [
    "ui/settingsKit.tsx",
    "enabled:hover:bg-fill enabled:active:bg-fill-strong",
    "a settings row's Action: one control shape in a card",
  ],
  ["ui/Settings.tsx", "aria-current={s.id === section", "a settings section row"],
];

/**
 * Spacing off the scale (docs/design.md › Spacing): 10px (`2.5`) and 14px (`3.5`) sit between two
 * steps, so a row, a card and a popover that use them drift a few pixels apart. A line that truly
 * needs one names itself here with why; a stale entry fails the test instead of quietly allowing
 * the next one. The scale also names its insets as constants (`cardInset`, `rowInset`,
 * `menuInset` in ui/primitives.tsx), which a screen should reuse before picking a number.
 */
const SPACING_ALLOWED: [file: string, part: string, why: string][] = [];
const SPACING =
  /(?<![\w-])(?:p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|ml|mr|gap|space-x|space-y)-(?:2|3)\.5(?![\w])/;

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

  test("spacing comes from the scale, not half steps", () => {
    const found: string[] = [];
    const used = new Set(SPACING_ALLOWED);
    for (const path of files(SRC)) {
      const file = relative(SRC, path);
      readFileSync(path, "utf8")
        .split("\n")
        .forEach((line, i) => {
          if (!SPACING.test(line)) return;
          const allowed = SPACING_ALLOWED.find(([at, part]) => at === file && line.includes(part));
          if (allowed) used.delete(allowed);
          else found.push(`${file}:${i + 1} off-scale spacing: ${line.trim()}`);
        });
    }
    expect(found).toEqual([]);
    expect([...used].map(([, , why]) => `allowlist entry no longer matches a line: ${why}`)).toEqual([]);
  });

  test("the engine's built-in themes carry the same surfaces", () => {
    // The app applies a theme's tokens inline, so an engine built-in that still
    // holds an old fill shadows index.css: the real app shows the theme file's
    // surface, not this one. Only the surface stack is compared: the engine
    // resolves every token to #rrggbb, so it flattens the app's translucent
    // borders and scrollbar, which cannot match byte for byte.
    const SURFACES = ["bg", "sidebar", "surface", "surface_2", "surface_3"] as const;
    for (const theme of BUILTIN_THEMES) {
      const toml = readFileSync(
        join(SRC, `../../../rustcode/engine/src/themes/builtin/${theme.name}.toml`),
        "utf8",
      );
      const desktop = toml.slice(toml.indexOf("[desktop]"));
      for (const token of SURFACES) {
        const value = new RegExp(`^${token} = "([^"]+)"`, "m").exec(desktop)?.[1];
        expect(value, `${theme.name}.toml desktop.${token}`).toBe(theme.tokens[token]);
      }
    }
  });

  test("blurred scrims have opaque platform and reduced-transparency fallbacks", () => {
    const css = readFileSync(join(SRC, "index.css"), "utf8");
    const block = (start: number) => {
      expect(start).toBeGreaterThanOrEqual(0);
      return start < 0 ? "" : css.slice(start, css.indexOf("}", start));
    };
    const reduced = block(css.indexOf(":root.reduce-transparency {"));
    expect(reduced).toContain("--rc-scrim-bg: var(--rc-bg);");
    expect(reduced).toContain("--rc-lightbox-scrim-bg: var(--rc-bg);");
    expect(reduced).toContain("--rc-glass-filter: none;");
    expect(block(css.indexOf(".rc-scrim {"))).toContain("background: var(--rc-scrim-bg);");
    expect(block(css.indexOf(".rc-lightbox-scrim {"))).toContain(
      "background-color: var(--rc-lightbox-scrim-bg);",
    );
    const reducedAt = css.indexOf("@media (prefers-reduced-transparency: reduce)");
    const reducedRoot = block(css.indexOf("  :root {", reducedAt));
    expect(reducedRoot).toContain("--rc-scrim-bg: var(--rc-bg);");
    expect(reducedRoot).toContain("--rc-lightbox-scrim-bg: var(--rc-bg);");
    // The media query is the fallback before lib/transparency.ts has run: the sidebar is solid too.
    expect(reducedRoot).toContain("--rc-sidebar-bg: var(--rc-sidebar);");
  });

  test("every translucent surface draws the shared filter, and nothing animates a blur", () => {
    const css = readFileSync(join(SRC, "index.css"), "utf8");
    // One filter for the lot: the composer, popovers, menus, dialogs, toasts, the header pane and
    // the overview's cards all read `--rc-glass-filter`, so one edit tunes them together.
    const filters = [...css.matchAll(/^\s*(?:-webkit-)?backdrop-filter:\s*([^;]+);/gm)];
    expect(filters.length).toBeGreaterThan(6);
    for (const match of filters) expect(match[1]?.trim()).toBe("var(--rc-glass-filter)");
    // The composer keeps the sidebar's hue and takes the shared alpha and filter in every theme.
    expect(css).not.toContain("--rc-composer-filter");
    for (const alpha of ["--rc-glass-bg", "--rc-glass-bg-strong", "--rc-glass-faint-bg"])
      expect(css).toContain(`${alpha}: color-mix(in srgb, var(--rc-surface-2) var(--rc-glass-alpha`);
    // A `backdrop-filter` under an element that fades or slides samples an empty group, so the blur
    // drops for the length of the motion and snaps back at its end (#3299). Nothing that draws the
    // glass carries an animation, and the overview's keyframes animate only what a card can afford.
    const animated: string[] = [];
    for (const match of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      const selector = match[1] ?? "";
      const body = match[2] ?? "";
      if (body.includes("backdrop-filter") && body.includes("animation")) animated.push(selector.trim());
    }
    expect(animated).toEqual([]);
    for (const name of ["rc-overview-in", "rc-overview-out", "rc-overview-fill-in", "rc-overview-fill-out"]) {
      const at = css.indexOf(`@keyframes ${name} {`);
      expect(at).toBeGreaterThanOrEqual(0);
      const block = css.slice(at, css.indexOf("\n}", at));
      const properties = [...block.matchAll(/^\s+(\S+):/gm)].map((match) => match[1] ?? "");
      expect(properties.length).toBeGreaterThan(0);
      for (const property of properties)
        expect(["opacity", "transform", "background-color"]).toContain(property);
    }
  });

  test("Linux keeps the glass; Reduce transparency is the solid fallback on every platform", () => {
    const css = readFileSync(join(SRC, "index.css"), "utf8");
    // No rule hands Linux (or Windows) the solid glass tokens: only the class and the media query do.
    for (const platform of ["linux", "windows"]) {
      const rules = [
        ...css.matchAll(new RegExp(`:root\\[data-platform="${platform}"\\][^{]*\\{([^}]*)\\}`, "g")),
      ];
      expect(rules.length).toBeGreaterThan(0);
      for (const [, body] of rules) {
        expect(body).not.toContain("--rc-glass-filter");
        expect(body).not.toContain("--rc-glass-bg");
        expect(body).not.toContain("--rc-scrim-bg");
      }
    }
    // The solid fallback reaches the sidebar too, so nothing shows a material under Reduce transparency.
    const reduced = css.slice(css.indexOf(":root.reduce-transparency {"));
    expect(reduced.slice(0, reduced.indexOf("}"))).toContain("--rc-sidebar-bg: var(--rc-sidebar);");
  });

  test("Windows shows Mica through the sidebar only, and only when the shell has it", () => {
    const css = readFileSync(join(SRC, "index.css"), "utf8");
    const mica = ':root[data-platform="windows"][data-material="mica"]:not(.reduce-transparency)';
    const rule = css.slice(css.indexOf(`${mica} {`));
    const body = rule.slice(0, rule.indexOf("}"));
    expect(body).toContain("--rc-sidebar-bg: color-mix(in srgb, var(--rc-sidebar) 72%, transparent);");
    // The body is cleared for the material and nothing else is: the chat pane keeps its opaque colour.
    expect(css).toContain(`${mica} body {\n  background: transparent;`);
    expect(css).toContain("--color-sidebar-glass: var(--rc-sidebar-bg);");
    // The base sidebar fill is the solid colour, so a platform without the material is unchanged.
    expect(css).toContain("--rc-sidebar-bg: var(--rc-sidebar);");
    // The window is transparent where the shell can give it a material, and nowhere else.
    const config = (name: string) =>
      JSON.parse(readFileSync(join(SRC, `../src-tauri/${name}`), "utf8")).app.windows[0];
    expect(config("tauri.windows.conf.json").transparent).toBe(true);
    expect(config("tauri.windows.conf.json").decorations).toBe(true);
    expect(config("tauri.linux.conf.json").transparent).toBe(false);
    expect(config("tauri.linux.conf.json").decorations).toBe(true);
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
