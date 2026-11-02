import { expect, test } from "bun:test";
import { crop, RAMP, renderAscii } from "./banner";

function solid(width: number, height: number, rgb: [number, number, number], alpha = 255) {
  const px = new Uint8Array(width * height * 4);
  for (let i = 0; i < px.length; i += 4) px.set([...rgb, alpha], i);
  return px;
}

test("crops a tall picture to a band at least 3:1, centred", () => {
  expect(crop(300, 300)).toEqual({ x: 0, y: 100, width: 300, height: 100 });
  expect(crop(900, 100)).toEqual({ x: 0, y: 0, width: 900, height: 100 });
});

test("fill crops to exactly the room's shape", () => {
  expect(crop(1000, 100, 5, true)).toEqual({ x: 250, y: 0, width: 500, height: 100 });
  expect(crop(400, 400, 4, true)).toEqual({ x: 0, y: 150, width: 400, height: 100 });
});

test("white inks densest on dark, blank when inverted", () => {
  const white = solid(60, 20, [255, 255, 255]);
  const dark = renderAscii(white, 60, 20, 30);
  expect(dark.length).toBe(5);
  expect(dark[0]?.length).toBe(30);
  expect(dark[0]?.[0]?.ch).toBe(RAMP[RAMP.length - 1]);
  expect(renderAscii(white, 60, 20, 30, { inverted: true })[0]?.[0]?.ch).toBe(" ");
});

test("a see-through picture reads as background", () => {
  const clear = solid(30, 10, [255, 0, 0], 0);
  expect(renderAscii(clear, 30, 10, 10)[0]?.[0]?.ch).toBe(" ");
  expect(renderAscii(clear, 30, 10, 0)).toEqual([]);
});
