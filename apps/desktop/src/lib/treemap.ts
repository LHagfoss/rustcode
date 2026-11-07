// The context card's tile map: pure geometry, no DOM. A squarified treemap packs the categories that
// fill the window into one rectangle, largest first, so every tile stays readable instead of the
// long thin strips a plain slice-and-dice leaves. The component only turns these boxes into styles,
// which is also what lets a tile glide when its category changes size.

export interface TreemapItem {
  key: string;
  value: number;
}

export interface TreemapRect {
  key: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Scaled {
  key: string;
  area: number;
}

const sum = (items: { area: number }[]) => items.reduce((total, item) => total + item.area, 0);

/** The worst aspect ratio a row would have if another item joined it: the squarify rule. */
function worst(row: Scaled[], side: number): number {
  if (!row.length || side <= 0) return Number.POSITIVE_INFINITY;
  const total = sum(row);
  if (total <= 0) return Number.POSITIVE_INFINITY;
  const largest = Math.max(...row.map((item) => item.area));
  const smallest = Math.min(...row.map((item) => item.area));
  if (smallest <= 0) return Number.POSITIVE_INFINITY;
  return Math.max((side * side * largest) / (total * total), (total * total) / (side * side * smallest));
}

function squarify(values: Scaled[], box: Box): TreemapRect[] {
  const rects: TreemapRect[] = [];
  let remaining = values;
  let { x, y, width, height } = box;
  while (remaining.length > 0) {
    const side = Math.min(width, height);
    if (side <= 0) break;
    // Grow the row while another item makes it no less square.
    let row: Scaled[] = [remaining[0]!];
    let next = 1;
    while (next < remaining.length) {
      const candidate = [...row, remaining[next]!];
      if (worst(candidate, side) > worst(row, side)) break;
      row = candidate;
      next += 1;
    }
    const rowArea = sum(row);
    if (rowArea <= 0) break;
    if (width >= height) {
      // A column down the left edge; each item's slice of it is its area.
      const columnWidth = rowArea / height;
      let top = y;
      for (const item of row) {
        const itemHeight = (item.area / rowArea) * height;
        rects.push({ key: item.key, x, y: top, width: columnWidth, height: itemHeight });
        top += itemHeight;
      }
      x += columnWidth;
      width -= columnWidth;
    } else {
      // A row across the top edge.
      const rowHeight = rowArea / width;
      let left = x;
      for (const item of row) {
        const itemWidth = (item.area / rowArea) * width;
        rects.push({ key: item.key, x: left, y, width: itemWidth, height: rowHeight });
        left += itemWidth;
      }
      y += rowHeight;
      height -= rowHeight;
    }
    remaining = remaining.slice(row.length);
  }
  return rects;
}

/** Inset each box by half the gap on every side, so neighbours sit `gap` apart. */
function inset(rect: TreemapRect, gap: number): TreemapRect {
  if (gap <= 0) return rect;
  const half = gap / 2;
  return {
    ...rect,
    x: rect.x + half,
    y: rect.y + half,
    width: Math.max(0, rect.width - gap),
    height: Math.max(0, rect.height - gap),
  };
}

/**
 * Lay `items` out in a `width` × `height` box. Zero, negative and non-finite values take no room;
 * items of equal value keep their given order. Returns boxes in the order they were laid out (largest
 * first), never outside the box.
 */
export function layoutTiles(items: TreemapItem[], width: number, height: number, gap = 0): TreemapRect[] {
  if (!(width > 0) || !(height > 0)) return [];
  const positive = items
    .filter((item) => Number.isFinite(item.value) && item.value > 0)
    .map((item, index) => ({ ...item, index }))
    .sort((a, b) => b.value - a.value || a.index - b.index);
  if (!positive.length) return [];
  const total = positive.reduce((count, item) => count + item.value, 0);
  const area = width * height;
  const scaled = positive.map((item) => ({
    key: item.key,
    area: (item.value / total) * area,
  }));
  return squarify(scaled, { x: 0, y: 0, width, height }).map((rect) => inset(rect, gap));
}
