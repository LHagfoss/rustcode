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
  ["ui/Composer.tsx", "aria-pressed={wt.on}", "a worktree toggle; its fill is its state"],
  ["ui/Composer.tsx", "Attach and more", "a round icon button, the send button's size"],
  ["ui/Composer.tsx", "Show all", "the +N thumbnail tile, sized and shaped as the pictures beside it"],
  ["ui/Review.tsx", "aria-current={selected === path", "a file row in the review pane"],
  ["ui/TurnChanges.tsx", "bg-fill-subtle", "the changes pill is a popover trigger"],
  ["ui/TurnTrace.tsx", "bg-fill-subtle", "the trace pill is a popover trigger"],
  [
    "ui/ModelPicker.tsx",
    "aria-pressed={selected}",
    "a harness in the model card's rail; its fill marks the one browsed",
  ],
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
    const reduced = block(
      css.indexOf(':root:is(.reduce-transparency, [data-platform="windows"], [data-platform="linux"]) {'),
    );
    expect(reduced).toContain("--rc-scrim-bg: var(--rc-bg);");
    expect(reduced).toContain("--rc-lightbox-scrim-bg: var(--rc-bg);");
    // The solid fallback takes the shared filter away with the translucency: a blur behind an
    // opaque fill is a compositing layer for nothing. The media query covers the first frame,
    // before lib/transparency.ts has run; the shared rule after it covers the class and the two
    // platforms that paint no blur at all.
    const media = [...css.matchAll(/:root\s+:is\([^)]*\)[^{]*\{([^}]*)\}/g)];
    expect(media).toHaveLength(1);
    for (const rule of media) expect(rule[1]).toContain("backdrop-filter: none;");
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
    const block = (start: number) => {
      expect(start).toBeGreaterThanOrEqual(0);
      return start < 0 ? "" : css.slice(start, css.indexOf("}", start));
    };
    // One filter for the lot, written in one place as a literal: a `var()` here is no blur on
    // WebKit, whose prefixed `backdrop-filter` ignores custom properties (WebKit bug 289800), and
    // the `-webkit-` twin is the one it paints with.
    const FILTER = "saturate(1.15) blur(9px)";
    const filters = [...css.matchAll(/^\s*(?:-webkit-)?backdrop-filter:\s*([^;]+);/gm)].map((match) =>
      match[1]?.trim(),
    );
    expect(new Set(filters)).toEqual(new Set([FILTER, "none"]));
    // The composer keeps the sidebar's hue and takes the shared alpha and filter in every theme.
    expect(css).not.toContain("--rc-composer-filter");
    for (const alpha of ["--rc-glass-bg", "--rc-glass-bg-strong", "--rc-glass-faint-bg"])
      expect(css).toContain(`${alpha}: color-mix(in srgb, var(--rc-surface-2) var(--rc-glass-alpha`);
    // Every surface that draws glass takes the one filter; none names a token of its own.
    for (const surface of [
      ".rc-glass {",
      ".rc-glass-faint {",
      ".rc-scrim {",
      ".rc-lightbox-scrim {",
      ".rc-glass-overlay {",
      ".rc-composer {",
      ".rc-composer-footer {",
      ".rc-header-glass::before {",
      "@utility elevated {",
    ])
      expect(block(css.indexOf(surface)), surface).toContain("@apply glass-filter;");
    expect(css).toContain(
      ".rc-composer,\n.rc-toast {\n  background: var(--rc-glass-bg);\n  @apply glass-filter;",
    );
    // A `backdrop-filter` under an element that fades or slides samples an empty group, so the blur
    // drops for the length of the motion and snaps back at its end (#3299). Nothing that draws the
    // glass carries an animation, and the overview's keyframes animate only what a card can afford.
    const animated: string[] = [];
    for (const match of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      const selector = match[1] ?? "";
      const body = match[2] ?? "";
      const glass = body.includes("backdrop-filter") || body.includes("@apply glass-filter");
      if (glass && body.includes("animation")) animated.push(selector.trim());
    }
    expect(animated).toEqual([]);
    // The motion may only be claimed by what is inside a card: the fade and rise by the body, the
    // fill fade by the card's own surface. An animation back on `.rc-overview-in`/`-out` themselves
    // would sit above the glass and bring the dropped blur straight back.
    const claims: string[] = [];
    for (const match of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      const name = /animation:\s*(rc-overview-[\w-]+)/.exec(match[2] ?? "");
      if (name) claims.push(`${(match[1] ?? "").trim()} → ${name[1]}`);
    }
    expect(claims.sort()).toEqual([
      ".rc-overview-in .rc-overview-body → rc-overview-in",
      ".rc-overview-in .rc-overview-card → rc-overview-fill-in",
      ".rc-overview-out .rc-overview-body → rc-overview-out",
      ".rc-overview-out .rc-overview-card → rc-overview-fill-out",
    ]);
    // ...and each of them animates only what a still surface can afford.
    const allowed: Record<string, string[]> = {
      "rc-overview-in": ["opacity", "transform"],
      "rc-overview-out": ["opacity", "transform"],
      "rc-overview-fill-in": ["background-color"],
      "rc-overview-fill-out": ["background-color"],
    };
    for (const [name, properties] of Object.entries(allowed)) {
      const at = css.indexOf(`@keyframes ${name} {`);
      expect(at).toBeGreaterThanOrEqual(0);
      const block = css.slice(at, css.indexOf("\n}", at));
      const found = [...block.matchAll(/^\s+(\S+):/gm)].map((match) => match[1] ?? "");
      expect(found.length).toBeGreaterThan(0);
      for (const property of found) expect(properties).toContain(property);
    }
  });

  test("Windows and Linux paint no blur, and no window has a material", () => {
    const css = readFileSync(join(SRC, "index.css"), "utf8");
    // Windows and Linux take the same solid fallback as Reduce transparency, on every frame.
    const solid = ':root:is(.reduce-transparency, [data-platform="windows"], [data-platform="linux"])';
    const block = (selector: string) => {
      const start = css.indexOf(selector);
      expect(start).toBeGreaterThanOrEqual(0);
      return css.slice(start, css.indexOf("}", start));
    };
    // Opaque fills: surface-2 for the glass and popovers, sidebar for the composer, bg for scrims.
    const tokens = block(`${solid} {`);
    for (const token of [
      "--rc-sidebar-bg: var(--rc-sidebar);",
      "--rc-glass-bg: var(--rc-surface-2);",
      "--rc-glass-bg-strong: var(--rc-surface-2);",
      "--rc-glass-faint-bg: var(--rc-surface-2);",
      "--rc-scrim-bg: var(--rc-bg);",
      "--rc-lightbox-scrim-bg: var(--rc-bg);",
      "--rc-composer-bg: var(--rc-sidebar);",
    ])
      expect(tokens, token).toContain(token);
    // Both filters are taken away, the header's pseudo-element included, on every surface that
    // would otherwise draw the glass.
    const surfaces = css.slice(css.indexOf(`${solid}\n  :is(`));
    const fallback = surfaces.slice(0, surfaces.indexOf("}"));
    expect(fallback).toContain("backdrop-filter: none;");
    expect(fallback).toContain("-webkit-backdrop-filter: none;");
    for (const surface of [
      ".rc-glass",
      ".rc-glass-faint",
      ".rc-glass-overlay",
      ".rc-composer",
      ".rc-toast",
      ".rc-composer-footer",
      ".rc-scrim",
      ".rc-lightbox-scrim",
      ".elevated",
      ".glass-filter",
    ])
      expect(fallback, surface).toContain(surface);
    expect(css).toContain(`${solid} .rc-header-glass::before {`);
    // The decorative blurs go too: the streaming reveal, and the banner's own (ui/NewThreadBanner.tsx).
    expect(block(':root:is([data-platform="windows"], [data-platform="linux"]) .rc-reveal')).toContain(
      "animation-name: rc-fade;",
    );
    // Nothing lets a window material show through the page, and no window asks for one.
    expect(css).not.toContain("data-material");
    const config = (name: string) =>
      JSON.parse(readFileSync(join(SRC, `../src-tauri/${name}`), "utf8")).app.windows[0];
    expect(config("tauri.windows.conf.json").transparent).toBe(false);
    expect(config("tauri.linux.conf.json").transparent).toBe(false);
    // Windows draws its own title bar (ui/WindowControls.tsx), so it has no native decorations
    // and no native title; Linux keeps both.
    expect(config("tauri.windows.conf.json").decorations).toBe(false);
    expect(config("tauri.windows.conf.json").titleBarStyle).toBeUndefined();
    expect(config("tauri.windows.conf.json").hiddenTitle).toBeUndefined();
    expect(config("tauri.linux.conf.json").decorations).toBe(true);
    // The page can drive the window it draws: drag its title row, and minimize, maximize and close.
    const capabilities = JSON.parse(readFileSync(join(SRC, "../src-tauri/capabilities/default.json"), "utf8"))
      .permissions as string[];
    for (const permission of [
      "core:window:allow-start-dragging",
      "core:window:allow-minimize",
      "core:window:allow-toggle-maximize",
      "core:window:allow-is-maximized",
      "core:window:allow-close",
    ])
      expect(capabilities).toContain(permission);
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
