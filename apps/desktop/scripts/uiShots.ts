/** Pure parts of `ui-shots.ts`: which views changed and the comment that shows them. */

export const MARKER = "<!-- rustcode-ui-screenshots -->";

/** A channel may move this much (of 255) without counting: blur and dithering stay under it. */
export const TOLERANCE = 16;
/** A view counts as changed past this many moved pixels, so a stray edge pixel does not. */
export const MIN_PIXELS = 24;

/** Pixels whose red, green or blue moved more than `tolerance` between two RGBA buffers. */
export function countDiffering(a: ArrayLike<number>, b: ArrayLike<number>, tolerance: number): number {
  if (a.length !== b.length) return Math.max(a.length, b.length) / 4;
  let count = 0;
  for (let i = 0; i < a.length; i += 4) {
    for (let c = 0; c < 3; c++) {
      if (Math.abs((a[i + c] as number) - (b[i + c] as number)) > tolerance) {
        count++;
        break;
      }
    }
  }
  return count;
}

/** Views (by file name) whose pictures differ, appeared or disappeared; `moved` counts a pair's differing pixels. */
export async function changedShots(
  before: Map<string, Buffer>,
  after: Map<string, Buffer>,
  moved: (a: Buffer, b: Buffer) => Promise<number>,
): Promise<string[]> {
  const names = [...new Set([...before.keys(), ...after.keys()])].sort();
  const changed: string[] = [];
  for (const name of names) {
    const a = before.get(name);
    const b = after.get(name);
    if (!a || !b || (!a.equals(b) && (await moved(a, b)) > MIN_PIXELS)) changed.push(name);
  }
  return changed;
}

/** The comment body: a before/after table per changed view, or a one-line all-clear. */
export function commentFor(changed: string[], total: number, imageBase: string): string {
  if (changed.length === 0) {
    return `${MARKER}\n**UI screenshots**: none of the ${total} captured views changed.\n`;
  }
  const image = (side: string, file: string) => `<img src="${imageBase}/${side}/${file}" width="420">`;
  const rows = changed.map((file) => {
    const label = file.replace(/\.png$/, "");
    return `| **${label}** | ${image("before", file)} | ${image("after", file)} |`;
  });
  return [
    MARKER,
    `**UI screenshots**: ${changed.length} of ${total} views changed (mock build, 1512×945).`,
    "",
    "| View | Before | After |",
    "| --- | --- | --- |",
    ...rows,
    "",
  ].join("\n");
}
