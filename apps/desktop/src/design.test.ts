// The design system's guard rails (docs/design.md): no raw colours or
// palette classes outside the tokens, no retired class names, no arbitrary
// radii, no native selects, hand-timed hover, hand-made blur or menu items,
// and index.css's built-in themes equal to BUILTIN_THEMES.

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
const ARBITRARY_RADIUS = /(?<![\w-])rounded(?:-[trblse]{1,2})?-(?:\[|3xl|4xl)/;
const NATIVE_SELECT = /<select\b/;
const HAND_TIMED_HOVER = /(?<![\w-])transition-colors(?![\w-])/;
const HAND_MADE_BLUR = /(?<![\w-])backdrop-blur/;
const HAND_MADE_MENU_ITEM = /data-\[highlighted\]:/;

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
