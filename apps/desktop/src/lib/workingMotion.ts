/** The side of the working indicator's dot grid. */
export const GRID = 5;

/** One lit dot: its cell index (row * GRID + column), brightness 0–1, and whether it leads the motion. */
export type Lit = { cell: number; level: number; head?: boolean };

export type Pattern = (step: number) => Lit[];

const center = (GRID - 1) / 2;
const cellOf = (x: number, y: number) => y * GRID + x;

function hash(n: number): number {
  const s = Math.sin(n * 12.9898) * 43758.5453;
  return s - Math.floor(s);
}

/** Rings that swell out from the centre and fade, like a slow breath. */
const ring: Pattern = (step) => {
  const radius = (step * 0.35) % (center + 1.6);
  const out: Lit[] = [];
  for (let y = 0; y < GRID; y++)
    for (let x = 0; x < GRID; x++) {
      const level = 1 - Math.abs(Math.hypot(x - center, y - center) - radius) * 1.1;
      if (level > 0) out.push({ cell: cellOf(x, y), level });
    }
  return out;
};

/** A diamond that ripples out from the centre dot. */
const ripple: Pattern = (step) => {
  const front = (step * 0.4) % (center * 2 + 1);
  const out: Lit[] = [];
  for (let y = 0; y < GRID; y++)
    for (let x = 0; x < GRID; x++) {
      const d = Math.abs(x - center) + Math.abs(y - center);
      const level = 1 - Math.abs(d - front) * 0.9;
      if (level > 0) out.push({ cell: cellOf(x, y), level, head: d === 0 && level > 0.8 });
    }
  return out;
};

const perimeter: number[] = (() => {
  const path: number[] = [];
  for (let x = 0; x < GRID; x++) path.push(cellOf(x, 0));
  for (let y = 1; y < GRID; y++) path.push(cellOf(GRID - 1, y));
  for (let x = GRID - 2; x >= 0; x--) path.push(cellOf(x, GRID - 1));
  for (let y = GRID - 2; y > 0; y--) path.push(cellOf(0, y));
  return path;
})();

/** A short trail that runs round the edge of the grid. */
const orbit: Pattern = (step) => {
  const out: Lit[] = [];
  for (let k = 0; k < 6; k++) {
    const at = perimeter[(((step - k) % perimeter.length) + perimeter.length) % perimeter.length]!;
    out.push({ cell: at, level: 1 - k / 6, head: k === 0 });
  }
  return out;
};

function wander(t: number): number {
  const x = Math.round(center + center * Math.sin(t * 0.37));
  const y = Math.round(center + center * Math.sin(t * 0.53 + 1.3));
  return cellOf(x, y);
}

/** A snake that wanders the grid, its tail fading behind it. */
const snake: Pattern = (step) => {
  const out: Lit[] = [];
  const seen = new Set<number>();
  for (let k = 0; k < 7; k++) {
    const cell = wander(step - k);
    if (seen.has(cell)) continue;
    seen.add(cell);
    out.push({ cell, level: 1 - k / 7, head: k === 0 });
  }
  return out;
};

/** Dots that light and fade at random, like sparks. */
const sparkle: Pattern = (step) => {
  const out: Lit[] = [];
  for (let cell = 0; cell < GRID * GRID; cell++) {
    const phase = (step + Math.floor(hash(cell) * 40)) % 40;
    if (phase < 5) out.push({ cell, level: 1 - phase / 5, head: phase === 0 });
  }
  return out;
};

const spiralPath: number[] = (() => {
  const path: number[] = [];
  let [top, left, bottom, right] = [0, 0, GRID - 1, GRID - 1];
  while (top <= bottom && left <= right) {
    for (let x = left; x <= right; x++) path.push(cellOf(x, top));
    for (let y = top + 1; y <= bottom; y++) path.push(cellOf(right, y));
    if (top < bottom) for (let x = right - 1; x >= left; x--) path.push(cellOf(x, bottom));
    if (left < right) for (let y = bottom - 1; y > top; y--) path.push(cellOf(left, y));
    top++;
    left++;
    bottom--;
    right--;
  }
  return [...path, ...path.slice(0, -1).reverse()];
})();

/** A trail that winds into the centre along a spiral and back out again. */
const spiral: Pattern = (step) => {
  const out: Lit[] = [];
  const seen = new Set<number>();
  for (let k = 0; k < 5; k++) {
    const cell = spiralPath[(((step - k) % spiralPath.length) + spiralPath.length) % spiralPath.length]!;
    if (seen.has(cell)) continue;
    seen.add(cell);
    out.push({ cell, level: 1 - k / 5, head: k === 0 });
  }
  return out;
};

/** Drops that fall down each column at their own pace. */
const rain: Pattern = (step) => {
  const out: Lit[] = [];
  const fall = GRID + 3;
  for (let x = 0; x < GRID; x++) {
    const drop = (step + Math.floor(hash(x + 7) * fall)) % fall;
    for (let k = 0; k < 3; k++) {
      const y = drop - k;
      if (y >= 0 && y < GRID) out.push({ cell: cellOf(x, y), level: 1 - k / 3, head: k === 0 });
    }
  }
  return out;
};

/** A bright column that sweeps left and right like a scanner. */
const scan: Pattern = (step) => {
  const span = (GRID - 1) * 2;
  const phase = step % span;
  const at = phase < GRID ? phase : span - phase;
  const out: Lit[] = [];
  for (let y = 0; y < GRID; y++)
    for (let x = 0; x < GRID; x++) {
      const level = 1 - Math.abs(x - at) * 0.55;
      if (level > 0) out.push({ cell: cellOf(x, y), level, head: x === at && y === center });
    }
  return out;
};

export const PATTERNS = { ring, ripple, orbit, snake, sparkle, spiral, rain, scan } as const;
export type PatternName = keyof typeof PATTERNS;
const PATTERN_NAMES = Object.keys(PATTERNS) as PatternName[];

/** How many steps each pattern holds before the cycle moves on, and how many of them blend into the next. */
export const CYCLE_STEPS = 44;
export const CYCLE_BLEND = 8;

/**
 * Every pattern in turn, starting from `first`: each holds for `CYCLE_STEPS` and fades into the
 * next over its last `CYCLE_BLEND` steps, so a long turn shows the whole set.
 */
export function cyclePatterns(first: PatternName): Pattern {
  const start = PATTERN_NAMES.indexOf(first);
  const at = (i: number) => PATTERNS[PATTERN_NAMES[(start + i) % PATTERN_NAMES.length]!];
  return (step) => {
    const segment = Math.floor(step / CYCLE_STEPS);
    const into = step % CYCLE_STEPS;
    const current = at(segment)(step);
    const blendFrom = CYCLE_STEPS - CYCLE_BLEND;
    if (into < blendFrom) return current;
    const mix = (into - blendFrom + 1) / (CYCLE_BLEND + 1);
    const out: Lit[] = [];
    for (const lit of current) out.push({ ...lit, level: lit.level * (1 - mix) });
    for (const lit of at(segment + 1)(step)) out.push({ ...lit, level: lit.level * mix });
    return out.filter((lit) => lit.level > 0);
  };
}

/** The words a working turn picks from: one word each, said while the model works. */
export const WORKING_WORDS = [
  "Churning",
  "Pondering",
  "Brewing",
  "Mulling",
  "Tinkering",
  "Conjuring",
  "Percolating",
  "Simmering",
  "Musing",
  "Cogitating",
  "Whirring",
  "Crafting",
  "Puzzling",
  "Weaving",
  "Untangling",
  "Distilling",
  "Forging",
  "Noodling",
  "Wrangling",
  "Scheming",
] as const;

/** The turn's own word and grid pattern, fixed by its start time so a re-render never reshuffles them. */
export function turnFlavor(seed: number | undefined): { word: string; pattern: PatternName } {
  const s = seed ?? 0;
  return {
    word: WORKING_WORDS[Math.floor(hash(s) * WORKING_WORDS.length)]!,
    pattern: PATTERN_NAMES[Math.floor(hash(s + 1) * PATTERN_NAMES.length)]!,
  };
}

/**
 * The one word the indicator says for a step label from `workingLabel`: Thinking while the model
 * thinks, Writing while it replies, the turn's own word otherwise; the rest is the step's detail.
 */
export function splitWorkingLabel(label: string, word: string): { word: string; detail: string } {
  if (label === "Thinking") return { word: "Thinking", detail: "" };
  if (label.startsWith("Thinking · ")) return { word: "Thinking", detail: label.slice("Thinking · ".length) };
  if (label === "Writing the reply") return { word: "Writing", detail: "" };
  if (!label || label === "Working") return { word, detail: "" };
  return { word, detail: label };
}
