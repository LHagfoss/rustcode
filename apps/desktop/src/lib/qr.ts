// A QR Code encoder, for the pairing payload a phone scans (Settings ›
// Remote access). Byte mode, error-correction level M, versions 1-40, and the
// mask with the lowest penalty, as ISO/IEC 18004 and Nayuki's qrcodegen
// describe it. Local only: no dependency and no network, so a pairing
// credential never leaves the app to be drawn.

export type QrEcc = "L" | "M" | "Q" | "H";

export interface QrCode {
  /** Modules per side, without the quiet zone the caller draws. */
  size: number;
  /** `modules[y][x]`, true for a dark module. */
  modules: boolean[][];
}

// Error-correction codewords per block and the number of blocks, by version
// and level (`-1` at index 0, where no version exists).
const ECC_CODEWORDS_PER_BLOCK: Record<QrEcc, number[]> = {
  L: [
    -1, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28, 30, 30, 26, 28,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
  ],
  M: [
    -1, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28, 28, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
  ],
  Q: [
    -1, 13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30, 30, 30, 30,
    28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
  ],
  H: [
    -1, 17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
  ],
};

const NUM_ERROR_CORRECTION_BLOCKS: Record<QrEcc, number[]> = {
  L: [
    -1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13, 14, 15, 16,
    17, 18, 19, 19, 20, 21, 22, 24, 25,
  ],
  M: [
    -1, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25, 26, 28,
    29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
  ],
  Q: [
    -1, 1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29, 34, 34, 35,
    38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68,
  ],
  H: [
    -1, 1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32, 35, 37, 40, 42,
    45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81,
  ],
};

/** The format bits' error-correction field. */
const ECC_FORMAT_BITS: Record<QrEcc, number> = { L: 1, M: 0, Q: 3, H: 2 };

const MAX_VERSION = 40;

/** The version a payload fits in, or 0 when it is too long for any. */
export function qrVersion(text: string, ecc: QrEcc = "M"): number {
  const length = new TextEncoder().encode(text).length;
  for (let version = 1; version <= MAX_VERSION; version++) {
    const capacity = dataCodewords(version, ecc) * 8;
    if (4 + charCountBits(version) + 8 * length <= capacity) return version;
  }
  return 0;
}

/** The matrix for `text`, or null when it does not fit in a version 40 code. */
export function encodeQr(text: string, ecc: QrEcc = "M"): QrCode | null {
  const version = qrVersion(text, ecc);
  if (!version) return null;
  const bytes = new TextEncoder().encode(text);
  return draw(version, ecc, addErrorCorrection(payloadCodewords(bytes, version, ecc), version, ecc));
}

/** Unused, but the pairing flow asks for level M and the caller may pass another. */
export const QR_ECC_DEFAULT: QrEcc = "M";

function charCountBits(version: number): number {
  return version <= 9 ? 8 : 16;
}

function numRawDataModules(version: number): number {
  let result = (16 * version + 128) * version + 64;
  if (version >= 2) {
    const align = Math.floor(version / 7) + 2;
    result -= (25 * align - 10) * align - 55;
    if (version >= 7) result -= 36;
  }
  return result;
}

function dataCodewords(version: number, ecc: QrEcc): number {
  return (
    Math.floor(numRawDataModules(version) / 8) -
    ECC_CODEWORDS_PER_BLOCK[ecc][version]! * NUM_ERROR_CORRECTION_BLOCKS[ecc][version]!
  );
}

class Bits {
  readonly bits: number[] = [];
  append(value: number, length: number) {
    for (let i = length - 1; i >= 0; i--) this.bits.push((value >>> i) & 1);
  }
  toCodewords(): number[] {
    const out: number[] = [];
    for (let i = 0; i < this.bits.length; i += 8) {
      let value = 0;
      for (let j = 0; j < 8; j++) value = (value << 1) | (this.bits[i + j] ?? 0);
      out.push(value);
    }
    return out;
  }
}

/** Mode, length, data, terminator and the pad bytes, as the standard orders them. */
function payloadCodewords(bytes: Uint8Array, version: number, ecc: QrEcc): number[] {
  const capacity = dataCodewords(version, ecc) * 8;
  const bits = new Bits();
  bits.append(0b0100, 4); // byte mode
  bits.append(bytes.length, charCountBits(version));
  for (const byte of bytes) bits.append(byte, 8);
  bits.append(0, Math.min(4, capacity - bits.bits.length));
  bits.append(0, (8 - (bits.bits.length % 8)) % 8);
  let pad = 0xec;
  while (bits.bits.length < capacity) {
    bits.append(pad, 8);
    pad ^= 0xec ^ 0x11;
  }
  return bits.toCodewords();
}

function mul(x: number, y: number): number {
  let z = 0;
  for (let i = 7; i >= 0; i--) {
    z = (z << 1) ^ ((z >>> 7) * 0x11d);
    z ^= ((y >>> i) & 1) * x;
  }
  return z;
}

function rsDivisor(degree: number): number[] {
  const result = new Array<number>(degree).fill(0);
  result[degree - 1] = 1;
  let root = 1;
  for (let i = 0; i < degree; i++) {
    for (let j = 0; j < degree; j++) {
      result[j] = mul(result[j]!, root);
      if (j + 1 < degree) result[j] = result[j]! ^ result[j + 1]!;
    }
    root = mul(root, 0x02);
  }
  return result;
}

function rsRemainder(data: number[], divisor: number[]): number[] {
  const result = new Array<number>(divisor.length).fill(0);
  for (const byte of data) {
    const factor = byte ^ result.shift()!;
    result.push(0);
    for (let i = 0; i < divisor.length; i++) result[i] = result[i]! ^ mul(divisor[i]!, factor);
  }
  return result;
}

/** Each block's data and error-correction codewords, interleaved as the standard lays them out. */
function addErrorCorrection(data: number[], version: number, ecc: QrEcc): number[] {
  const blocks = NUM_ERROR_CORRECTION_BLOCKS[ecc][version]!;
  const eccLength = ECC_CODEWORDS_PER_BLOCK[ecc][version]!;
  const rawCodewords = Math.floor(numRawDataModules(version) / 8);
  const shortBlocks = blocks - (rawCodewords % blocks);
  const shortBlockLength = Math.floor(rawCodewords / blocks);
  const divisor = rsDivisor(eccLength);
  const padded: number[][] = [];
  for (let i = 0, offset = 0; i < blocks; i++) {
    const length = shortBlockLength - eccLength + (i < shortBlocks ? 0 : 1);
    const block = data.slice(offset, offset + length);
    offset += length;
    const correction = rsRemainder(block, divisor);
    // Pad a short block once so every block has the same length: its slot is
    // skipped when the codewords are interleaved.
    if (i < shortBlocks) block.push(0);
    padded.push(block.concat(correction));
  }
  const out: number[] = [];
  for (let i = 0; i < padded[0]!.length; i++) {
    for (let j = 0; j < padded.length; j++) {
      if (i !== shortBlockLength - eccLength || j >= shortBlocks) out.push(padded[j]![i]!);
    }
  }
  return out;
}

function alignmentPositions(version: number): number[] {
  if (version === 1) return [];
  const count = Math.floor(version / 7) + 2;
  const step = version === 32 ? 26 : Math.ceil((version * 4 + 4) / (count * 2 - 2)) * 2;
  const result = [6];
  for (let pos = version * 4 + 10; result.length < count; pos -= step) result.splice(1, 0, pos);
  return result;
}

function draw(version: number, ecc: QrEcc, codewords: number[]): QrCode {
  const size = version * 4 + 17;
  const modules = Array.from({ length: size }, () => new Array<boolean>(size).fill(false));
  const isFunction = Array.from({ length: size }, () => new Array<boolean>(size).fill(false));

  const setFunction = (x: number, y: number, dark: boolean) => {
    modules[y]![x] = dark;
    isFunction[y]![x] = true;
  };

  // Timing patterns first: the three finder patterns overwrite their ends.
  for (let i = 0; i < size; i++) {
    setFunction(6, i, i % 2 === 0);
    setFunction(i, 6, i % 2 === 0);
  }

  // Finder patterns with their separators, at three corners.
  for (const [cx, cy] of [
    [3, 3],
    [size - 4, 3],
    [3, size - 4],
  ] as const) {
    for (let dy = -4; dy <= 4; dy++) {
      for (let dx = -4; dx <= 4; dx++) {
        const x = cx + dx;
        const y = cy + dy;
        if (x >= 0 && x < size && y >= 0 && y < size) {
          setFunction(
            x,
            y,
            Math.max(Math.abs(dx), Math.abs(dy)) !== 2 && Math.max(Math.abs(dx), Math.abs(dy)) !== 4,
          );
        }
      }
    }
  }

  // Alignment patterns, except the three that would sit on a finder.
  const align = alignmentPositions(version);
  for (let i = 0; i < align.length; i++) {
    for (let j = 0; j < align.length; j++) {
      if ((i === 0 && j === 0) || (i === 0 && j === align.length - 1) || (i === align.length - 1 && j === 0))
        continue;
      for (let dy = -2; dy <= 2; dy++) {
        for (let dx = -2; dx <= 2; dx++) {
          setFunction(align[i]! + dx, align[j]! + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
        }
      }
    }
  }

  // The format area is reserved before the data, then filled for the chosen mask.
  const drawFormat = (mask: number) => {
    const data = (ECC_FORMAT_BITS[ecc] << 3) | mask;
    let remainder = data;
    for (let i = 0; i < 10; i++) remainder = (remainder << 1) ^ ((remainder >>> 9) * 0x537);
    const bits = ((data << 10) | remainder) ^ 0x5412;
    const bit = (i: number) => ((bits >>> i) & 1) !== 0;
    for (let i = 0; i <= 5; i++) setFunction(8, i, bit(i));
    setFunction(8, 7, bit(6));
    setFunction(8, 8, bit(7));
    setFunction(7, 8, bit(8));
    for (let i = 9; i < 15; i++) setFunction(14 - i, 8, bit(i));
    for (let i = 0; i < 8; i++) setFunction(size - 1 - i, 8, bit(i));
    for (let i = 8; i < 15; i++) setFunction(8, size - 15 + i, bit(i));
    setFunction(8, size - 8, true);
  };
  drawFormat(0);

  if (version >= 7) {
    let remainder = version;
    for (let i = 0; i < 12; i++) remainder = (remainder << 1) ^ ((remainder >>> 11) * 0x1f25);
    const bits = (version << 12) | remainder;
    for (let i = 0; i < 18; i++) {
      const dark = ((bits >>> i) & 1) !== 0;
      const a = size - 11 + (i % 3);
      const b = Math.floor(i / 3);
      setFunction(a, b, dark);
      setFunction(b, a, dark);
    }
  }

  // Data in the two-module zigzag from the bottom right, skipping the timing column.
  let index = 0;
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5;
    for (let vert = 0; vert < size; vert++) {
      for (let j = 0; j < 2; j++) {
        const x = right - j;
        const upward = ((right + 1) & 2) === 0;
        const y = upward ? size - 1 - vert : vert;
        if (!isFunction[y]![x] && index < codewords.length * 8) {
          modules[y]![x] = ((codewords[index >>> 3]! >>> (7 - (index & 7))) & 1) !== 0;
          index++;
        }
      }
    }
  }

  const maskBit = (mask: number, x: number, y: number): boolean => {
    switch (mask) {
      case 0:
        return (x + y) % 2 === 0;
      case 1:
        return y % 2 === 0;
      case 2:
        return x % 3 === 0;
      case 3:
        return (x + y) % 3 === 0;
      case 4:
        return (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0;
      case 5:
        return ((x * y) % 2) + ((x * y) % 3) === 0;
      case 6:
        return (((x * y) % 2) + ((x * y) % 3)) % 2 === 0;
      default:
        return (((x + y) % 2) + ((x * y) % 3)) % 2 === 0;
    }
  };
  const applyMask = (mask: number) => {
    for (let y = 0; y < size; y++) {
      for (let x = 0; x < size; x++) {
        if (!isFunction[y]![x] && maskBit(mask, x, y)) modules[y]![x] = !modules[y]![x];
      }
    }
  };

  let bestMask = 0;
  let bestPenalty = Number.POSITIVE_INFINITY;
  for (let mask = 0; mask < 8; mask++) {
    applyMask(mask);
    drawFormat(mask);
    const score = penalty(modules);
    if (score < bestPenalty) {
      bestPenalty = score;
      bestMask = mask;
    }
    applyMask(mask); // undo: XOR applies both ways
  }
  applyMask(bestMask);
  drawFormat(bestMask);

  return { size, modules };
}

/** The four penalty rules of the standard; the lowest-scoring mask wins. */
function penalty(modules: boolean[][]): number {
  const size = modules.length;
  const at = (x: number, y: number) => modules[y]![x]!;
  let result = 0;

  for (let y = 0; y < size; y++) {
    let color = false;
    let run = 0;
    const history = [0, 0, 0, 0, 0, 0, 0];
    for (let x = 0; x < size; x++) {
      if (at(x, y) === color) {
        run++;
        if (run === 5) result += 3;
        else if (run > 5) result++;
      } else {
        addHistory(run, history, size);
        if (!color) result += countFinderPatterns(history) * 40;
        color = at(x, y);
        run = 1;
      }
    }
    result += terminateAndCount(color, run, history, size) * 40;
  }

  for (let x = 0; x < size; x++) {
    let color = false;
    let run = 0;
    const history = [0, 0, 0, 0, 0, 0, 0];
    for (let y = 0; y < size; y++) {
      if (at(x, y) === color) {
        run++;
        if (run === 5) result += 3;
        else if (run > 5) result++;
      } else {
        addHistory(run, history, size);
        if (!color) result += countFinderPatterns(history) * 40;
        color = at(x, y);
        run = 1;
      }
    }
    result += terminateAndCount(color, run, history, size) * 40;
  }

  for (let y = 0; y < size - 1; y++) {
    for (let x = 0; x < size - 1; x++) {
      const c = at(x, y);
      if (c === at(x + 1, y) && c === at(x, y + 1) && c === at(x + 1, y + 1)) result += 3;
    }
  }

  let dark = 0;
  for (const row of modules) for (const value of row) if (value) dark++;
  const total = size * size;
  const balance = Math.ceil(Math.abs(dark * 20 - total * 10) / total) - 1;
  return result + balance * 10;
}

function addHistory(run: number, history: number[], size: number) {
  // The run that starts at the border includes the quiet zone's light modules.
  const length = history[0] === 0 ? run + size : run;
  history.pop();
  history.unshift(length);
}

function countFinderPatterns(history: number[]): number {
  const n = history[1]!;
  const core = n > 0 && history[2] === n && history[3] === n * 3 && history[4] === n && history[5] === n;
  if (!core) return 0;
  return (
    (history[0]! >= n * 4 && history[6]! >= n ? 1 : 0) + (history[6]! >= n * 4 && history[0]! >= n ? 1 : 0)
  );
}

function terminateAndCount(color: boolean, run: number, history: number[], size: number): number {
  let length = run;
  if (color) {
    addHistory(length, history, size);
    length = 0;
  }
  length += size; // the light border after the last module
  addHistory(length, history, size);
  return countFinderPatterns(history);
}
