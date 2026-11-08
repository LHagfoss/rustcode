// The pairing QR code (docs/remote.md › Pairing): a byte-mode, level-M code the
// phone camera reads. The encoder has no dependency, so these tests check it
// at the level a scanner does: version choice, the fixed patterns, the format
// bits, and the payload read back out of the data region.

import { describe, expect, test } from "bun:test";
import { encodeQr, type QrCode, qrVersion } from "./qr";

const PAYLOAD = JSON.stringify({
  protocol_version: 1,
  address: "192.168.1.24:17879",
  gateway_id: "9f2c1d0b4a6e8c7d5b3a1908f7e6d5c4",
  credential: "jH8sK2mQ9xT4vLpR6nB1yC3fW7gZ0aE5uI2oP8sD4qX",
  host_name: "Lars's MacBook Pro",
});

describe("qr code", () => {
  test("picks the smallest version that fits the bytes", () => {
    // Version 1-M holds 16 data codewords: 4 mode + 8 count + 8n bits.
    expect(qrVersion("x".repeat(14))).toBe(1);
    expect(qrVersion("x".repeat(15))).toBe(2);
    // A pairing payload is a couple of hundred bytes, well inside version 40.
    expect(qrVersion(PAYLOAD)).toBeGreaterThan(1);
    expect(qrVersion("x".repeat(10_000))).toBe(0);
    expect(encodeQr("x".repeat(10_000))).toBeNull();
  });

  test("draws the three finders, the timing patterns and the dark module", () => {
    const code = encodeQr(PAYLOAD)!;
    const { size } = code;
    const at = (x: number, y: number) => code.modules[y]![x]!;
    for (const [cx, cy] of [
      [3, 3],
      [size - 4, 3],
      [3, size - 4],
    ]) {
      for (let dy = -3; dy <= 3; dy++) {
        for (let dx = -3; dx <= 3; dx++) {
          // The ring is dark, the gap light, the centre 3×3 dark.
          const ring = Math.max(Math.abs(dx), Math.abs(dy));
          expect(at(cx! + dx, cy! + dy)).toBe(ring === 3 || ring <= 1);
        }
      }
    }
    // Timing patterns alternate on the row and column through the finders.
    for (let i = 8; i < size - 8; i++) {
      expect(at(i, 6)).toBe(i % 2 === 0);
      expect(at(6, i)).toBe(i % 2 === 0);
    }
    expect(at(8, size - 8)).toBe(true);
  });

  test("the format area names level M and a real mask", () => {
    for (const text of [PAYLOAD, "x".repeat(15), "hello"]) {
      const code = encodeQr(text)!;
      const mask = formatMask(code);
      expect(mask).toBeGreaterThanOrEqual(0);
      expect(mask).toBeLessThan(8);
    }
  });

  test("the data region reads back as the payload", () => {
    for (const text of [PAYLOAD, "x".repeat(15), "hello, qr"]) {
      const code = encodeQr(text)!;
      expect(readPayload(code)).toBe(text);
    }
  });
});

/** The mask the format bits record, by matching the 15-bit BCH string. */
function formatMask(code: QrCode): number {
  const bit = (x: number, y: number) => code.modules[y]![x]!;
  const bits: number[] = [];
  for (let i = 0; i <= 5; i++) bits.push(bit(8, i) ? 1 : 0);
  bits.push(bit(8, 7) ? 1 : 0, bit(8, 8) ? 1 : 0, bit(7, 8) ? 1 : 0);
  for (let i = 9; i < 15; i++) bits.push(bit(14 - i, 8) ? 1 : 0);
  for (let mask = 0; mask < 8; mask++) {
    let remainder = mask; // level M is 0b00, so the data is just the mask
    for (let i = 0; i < 10; i++) remainder = (remainder << 1) ^ ((remainder >>> 9) * 0x537);
    const expected = ((mask << 10) | remainder) ^ 0x5412;
    if (bits.every((value, i) => value === ((expected >>> i) & 1))) return mask;
  }
  return -1;
}

/**
 * A small reader for the test only: the function modules, the data walk and
 * the byte-mode header, without the error correction (a scanner checks that).
 */
function readPayload(code: QrCode): string {
  const { size } = code;
  const version = (size - 17) / 4;
  const mask = formatMask(code);
  const fixed = functionMap(version, size);
  const masked = (x: number, y: number): boolean => {
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
  const bits: number[] = [];
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5;
    for (let vert = 0; vert < size; vert++) {
      for (let j = 0; j < 2; j++) {
        const x = right - j;
        const upward = ((right + 1) & 2) === 0;
        const y = upward ? size - 1 - vert : vert;
        if (fixed[y]![x]) continue;
        let dark = code.modules[y]![x]!;
        if (masked(x, y)) dark = !dark;
        bits.push(dark ? 1 : 0);
      }
    }
  }
  // The matrix carries the codewords interleaved across the error-correction
  // blocks: undo that before the data can be read as one stream.
  const codewords: number[] = [];
  for (let i = 0; i + 8 <= bits.length; i += 8) {
    let value = 0;
    for (let j = 0; j < 8; j++) value = (value << 1) | (bits[i + j] ?? 0);
    codewords.push(value);
  }
  const [eccLength, blockCount] = M_BLOCKS[version]!;
  const shortLength = Math.floor(codewords.length / blockCount);
  const shortBlocks = blockCount - (codewords.length % blockCount);
  const blocks: number[][] = Array.from({ length: blockCount }, () => []);
  let at = 0;
  for (let i = 0; i <= shortLength; i++) {
    for (let j = 0; j < blockCount; j++) {
      if (i === shortLength - eccLength && j < shortBlocks) continue;
      blocks[j]![i] = codewords[at++]!;
    }
  }
  const data: number[] = [];
  for (let j = 0; j < blockCount; j++) {
    const length = shortLength - eccLength + (j < shortBlocks ? 0 : 1);
    for (let i = 0; i < length; i++) data.push(blocks[j]![i]!);
  }
  const stream = data.flatMap((word) => Array.from({ length: 8 }, (_, i) => (word >>> (7 - i)) & 1));
  const take = (count: number) => {
    let value = 0;
    for (let i = 0; i < count; i++) value = (value << 1) | (stream.shift() ?? 0);
    return value;
  };
  expect(take(4)).toBe(0b0100);
  const length = take(version <= 9 ? 8 : 16);
  const bytes = Array.from({ length }, () => take(8));
  return new TextDecoder().decode(new Uint8Array(bytes));
}

/** Level M's error-correction codewords per block and block count, versions 1-10. */
const M_BLOCKS: Record<number, [ecc: number, blocks: number]> = {
  1: [10, 1],
  2: [16, 1],
  3: [26, 1],
  4: [18, 2],
  5: [24, 2],
  6: [16, 4],
  7: [18, 4],
  8: [22, 4],
  9: [22, 5],
  10: [26, 5],
};

function functionMap(version: number, size: number): boolean[][] {
  const map = Array.from({ length: size }, () => new Array<boolean>(size).fill(false));
  const mark = (x: number, y: number) => {
    if (x >= 0 && x < size && y >= 0 && y < size) map[y]![x] = true;
  };
  for (let i = 0; i < size; i++) {
    mark(6, i);
    mark(i, 6);
  }
  for (const [cx, cy] of [
    [3, 3],
    [size - 4, 3],
    [3, size - 4],
  ]) {
    for (let dy = -4; dy <= 4; dy++) for (let dx = -4; dx <= 4; dx++) mark(cx! + dx, cy! + dy);
  }
  if (version > 1) {
    const count = Math.floor(version / 7) + 2;
    const step = version === 32 ? 26 : Math.ceil((version * 4 + 4) / (count * 2 - 2)) * 2;
    const positions = [6];
    for (let pos = version * 4 + 10; positions.length < count; pos -= step) positions.splice(1, 0, pos);
    for (let i = 0; i < positions.length; i++) {
      for (let j = 0; j < positions.length; j++) {
        if (
          (i === 0 && j === 0) ||
          (i === 0 && j === positions.length - 1) ||
          (i === positions.length - 1 && j === 0)
        )
          continue;
        for (let dy = -2; dy <= 2; dy++)
          for (let dx = -2; dx <= 2; dx++) mark(positions[i]! + dx, positions[j]! + dy);
      }
    }
  }
  for (let i = 0; i <= 5; i++) mark(8, i);
  mark(8, 7);
  mark(8, 8);
  mark(7, 8);
  for (let i = 9; i < 15; i++) mark(14 - i, 8);
  for (let i = 0; i < 8; i++) mark(size - 1 - i, 8);
  for (let i = 8; i < 15; i++) mark(8, size - 15 + i);
  mark(8, size - 8);
  if (version >= 7) {
    for (let i = 0; i < 18; i++) {
      const a = size - 11 + (i % 3);
      const b = Math.floor(i / 3);
      mark(a, b);
      mark(b, a);
    }
  }
  return map;
}
