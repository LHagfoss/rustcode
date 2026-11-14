// A command's coloured output as styled text: SGR sequences (`ESC[…m`) become a style on the text
// after them, mapped to theme tokens rather than the terminal's own colours, and every other escape
// sequence (cursor moves, erases, OSC titles and links) is dropped. A `\r` that is not part of a
// line break overwrites the line, as a progress bar does, so only what was written last stays.
// Pure, so the tool row and the tests share it.

export type AnsiColor = "danger" | "success" | "warning" | "accent" | "fg" | "muted";

export interface AnsiStyle {
  color?: AnsiColor;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
}

export interface AnsiSpan {
  text: string;
  style: AnsiStyle;
}

/** One line of output: its spans, in order. An empty line has none. */
export type AnsiLine = AnsiSpan[];

const ESC = String.fromCharCode(27);
const BEL = String.fromCharCode(7);

/**
 * One escape sequence: a CSI (`ESC[` params, intermediates, final byte), an OSC up to BEL or ST,
 * or a two-byte escape. An unterminated CSI or OSC at the end, and a lone ESC, match too, so a
 * truncated sequence is dropped rather than shown.
 */
const SEQUENCE = new RegExp(
  `${ESC}\\[([0-?]*)[ -/]*([@-~]|$)|${ESC}\\][^${BEL}${ESC}]*(?:${BEL}|${ESC}\\\\|$)|${ESC}[ -~]?`,
  "g",
);

/** The 8 base colours (30–37, and 90–97 bright): a token each, or none to stay plain. */
const BASE: readonly (AnsiColor | undefined)[] = [
  "muted", // black: invisible on a dark theme, so the quiet text colour
  "danger", // red
  "success", // green
  "warning", // yellow
  "accent", // blue
  undefined, // magenta: no token is close, so plain
  "accent", // cyan
  "fg", // white
];

/** The 256-colour palette's RGB for an index from 16 up (the 6×6×6 cube, then the grey ramp). */
function rgb256(n: number): [number, number, number] {
  if (n >= 232) {
    const v = 8 + (n - 232) * 10;
    return [v, v, v];
  }
  const i = n - 16;
  const level = (c: number) => (c === 0 ? 0 : 55 + c * 40);
  return [level(Math.floor(i / 36)), level(Math.floor(i / 6) % 6), level(i % 6)];
}

/** The token nearest an RGB colour: by hue when it has one, else quiet or plain by lightness. */
export function nearestColor(r: number, g: number, b: number): AnsiColor | undefined {
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  if (max - min < 40) return max < 128 ? "muted" : undefined;
  const d = max - min;
  let hue: number;
  if (max === r) hue = ((g - b) / d) * 60;
  else if (max === g) hue = ((b - r) / d) * 60 + 120;
  else hue = ((r - g) / d) * 60 + 240;
  if (hue < 0) hue += 360;
  if (hue < 20 || hue >= 330) return "danger";
  if (hue < 70) return "warning";
  if (hue < 170) return "success";
  if (hue < 260) return "accent";
  return undefined;
}

function color256(n: number): AnsiColor | undefined {
  if (!Number.isInteger(n) || n < 0 || n > 255) return undefined;
  if (n < 16) return BASE[n % 8];
  const [r, g, b] = rgb256(n);
  return nearestColor(r, g, b);
}

/** The style after one SGR sequence's parameters (`1;31`, `38;5;208`, ``). */
export function applySgr(style: AnsiStyle, params: string): AnsiStyle {
  const next: AnsiStyle = { ...style };
  const codes = params === "" ? [0] : params.split(/[;:]/).map((p) => (p === "" ? 0 : Number(p)));
  for (let i = 0; i < codes.length; i += 1) {
    const code = codes[i] as number;
    if (code === 0) {
      for (const key of Object.keys(next)) delete next[key as keyof AnsiStyle];
    } else if (code === 1) next.bold = true;
    else if (code === 2) next.dim = true;
    else if (code === 3) next.italic = true;
    else if (code === 4) next.underline = true;
    else if (code === 22) {
      delete next.bold;
      delete next.dim;
    } else if (code === 23) delete next.italic;
    else if (code === 24) delete next.underline;
    else if (code === 39) delete next.color;
    else if ((code >= 30 && code <= 37) || (code >= 90 && code <= 97)) {
      const c = BASE[code % 10];
      if (c) next.color = c;
      else delete next.color;
    } else if (code === 38 || code === 48) {
      // An extended colour: `5;n` from the 256 palette or `2;r;g;b`. A background is skipped over
      // (a tool row draws no background colours), and so is a foreground no token is near.
      const mode = codes[i + 1];
      let c: AnsiColor | undefined;
      if (mode === 5) {
        c = color256(codes[i + 2] ?? -1);
        i += 2;
      } else if (mode === 2) {
        const [r, g, b] = [codes[i + 2] ?? 0, codes[i + 3] ?? 0, codes[i + 4] ?? 0];
        c = nearestColor(r, g, b);
        i += 4;
      } else {
        i += 1;
      }
      if (code === 38) {
        if (c) next.color = c;
        else delete next.color;
      }
    }
    // Backgrounds (40–47, 100–107, 49), inverse, blink and the rest are left out.
  }
  return next;
}

function sameStyle(a: AnsiStyle, b: AnsiStyle): boolean {
  return (
    a.color === b.color &&
    !!a.bold === !!b.bold &&
    !!a.dim === !!b.dim &&
    !!a.italic === !!b.italic &&
    !!a.underline === !!b.underline
  );
}

/** True when text needs `parseAnsi`: it holds an escape or a `\r` that is not a line break. */
export function hasTerminalCodes(text: string): boolean {
  return text.includes(ESC) || /\r(?!\n)/.test(text);
}

/**
 * Output as styled lines. A style carries over line breaks until a reset, as it does in a
 * terminal; a span never crosses a line, so a folded box can show the first lines alone.
 */
export function parseAnsi(text: string): AnsiLine[] {
  const lines: AnsiLine[] = [];
  let line: AnsiLine = [];
  let style: AnsiStyle = {};

  const push = (chunk: string) => {
    if (!chunk) return;
    const last = line[line.length - 1];
    if (last && sameStyle(last.style, style)) last.text += chunk;
    else line.push({ text: chunk, style });
  };
  // Plain text between sequences: line breaks end a line, a lone `\r` starts it over.
  const write = (chunk: string) => {
    let start = 0;
    for (let i = 0; i < chunk.length; i += 1) {
      const ch = chunk.charCodeAt(i);
      if (ch === 10) {
        push(chunk.slice(start, i));
        lines.push(line);
        line = [];
        start = i + 1;
      } else if (ch === 13) {
        push(chunk.slice(start, i));
        if (chunk.charCodeAt(i + 1) !== 10) line = [];
        start = i + 1;
      }
    }
    push(chunk.slice(start));
  };

  SEQUENCE.lastIndex = 0;
  let at = 0;
  for (let match = SEQUENCE.exec(text); match; match = SEQUENCE.exec(text)) {
    write(text.slice(at, match.index));
    at = match.index + match[0].length;
    if (match[2] === "m") style = applySgr(style, match[1] ?? "");
  }
  write(text.slice(at));
  if (line.length || !text.endsWith("\n")) lines.push(line);
  return lines;
}

/** Styled lines back as plain text: what Copy copies. */
export function ansiPlainText(lines: readonly AnsiLine[]): string {
  return lines.map((line) => line.map((span) => span.text).join("")).join("\n");
}

const COLOR_CLASS: Record<AnsiColor, string> = {
  danger: "text-danger",
  success: "text-success",
  warning: "text-warning",
  accent: "text-accent",
  fg: "text-fg",
  muted: "text-fg-muted",
};

/** A span's classes: theme tokens only. Dim is the quiet text colour, or a colour faded. */
export function ansiClass(style: AnsiStyle): string {
  const classes: string[] = [];
  if (style.color) classes.push(COLOR_CLASS[style.color]);
  if (style.dim) classes.push(style.color ? "opacity-70" : "text-fg-muted");
  if (style.bold) classes.push("font-semibold");
  if (style.italic) classes.push("italic");
  if (style.underline) classes.push("underline");
  return classes.join(" ");
}
