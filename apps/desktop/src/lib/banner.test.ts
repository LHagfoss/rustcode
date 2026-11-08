import { expect, test } from "bun:test";
import {
  BANNER_BLUR_DEFAULT,
  BANNER_BLUR_MAX,
  BANNER_DARKEN_DEFAULT,
  BANNER_DARKEN_MAX,
  BANNER_IN_THREADS_DEFAULT,
  bannerSetting,
  blurScale,
  boundedNumber,
  crop,
  darkenOverlay,
  RAMP,
  renderAscii,
  setBannerBlur,
  setBannerDarken,
  setBannerInThreads,
} from "./banner";

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

/** App storage in a test (there is no window): a map, taken away again by `done()`. */
function testStorage(seed: Record<string, string> = {}) {
  const data = new Map<string, string>(Object.entries(seed));
  const scope = globalThis as unknown as Record<string, unknown>;
  scope.localStorage = {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
    removeItem: (key: string) => void data.delete(key),
  } as unknown as Storage;
  return { data, done: () => delete scope.localStorage };
}

test("the background starts undarkened, unblurred and only on new threads", () => {
  expect(BANNER_DARKEN_DEFAULT).toBe(0);
  expect(BANNER_BLUR_DEFAULT).toBe(0);
  expect(BANNER_IN_THREADS_DEFAULT).toBe(false);
  const storage = testStorage();
  try {
    setBannerInThreads(false);
    expect(bannerSetting()).toMatchObject({ darken: 0, blur: 0, inThreads: false });
  } finally {
    storage.done();
  }
});

test("darken, blur and show-in-threads persist and read back", () => {
  const storage = testStorage();
  try {
    setBannerDarken(45);
    setBannerBlur(12);
    setBannerInThreads(true);
    expect(storage.data.get("rustcode.newThreadBannerDarken")).toBe("45");
    expect(storage.data.get("rustcode.newThreadBannerBlur")).toBe("12");
    expect(storage.data.get("rustcode.newThreadBannerInThreads")).toBe("1");
    expect(bannerSetting()).toMatchObject({ darken: 45, blur: 12, inThreads: true });
    // Back to the defaults stores nothing, so a reset leaves no entry behind.
    setBannerDarken(BANNER_DARKEN_DEFAULT);
    setBannerBlur(BANNER_BLUR_DEFAULT);
    setBannerInThreads(false);
    expect([...storage.data.keys()].filter((k) => /Darken|Blur|InThreads/.test(k))).toEqual([]);
    expect(bannerSetting()).toMatchObject({ darken: 0, blur: 0, inThreads: false });
  } finally {
    storage.done();
  }
});

test("a stored amount outside its range is held to it, and junk is the default", () => {
  expect(boundedNumber("120", BANNER_DARKEN_MAX, 0)).toBe(BANNER_DARKEN_MAX);
  expect(boundedNumber("-5", BANNER_BLUR_MAX, 0)).toBe(0);
  expect(boundedNumber("12.6", BANNER_BLUR_MAX, 0)).toBe(13);
  expect(boundedNumber("lots", BANNER_BLUR_MAX, 7)).toBe(7);
  expect(boundedNumber("", BANNER_BLUR_MAX, 7)).toBe(7);
  const storage = testStorage({
    "rustcode.newThreadBannerDarken": "999",
    "rustcode.newThreadBannerBlur": "NaN",
  });
  try {
    setBannerInThreads(false);
    expect(bannerSetting()).toMatchObject({ darken: BANNER_DARKEN_MAX, blur: BANNER_BLUR_DEFAULT });
    setBannerDarken(500);
    expect(bannerSetting().darken).toBe(BANNER_DARKEN_MAX);
  } finally {
    storage.done();
  }
});

test("the darkening is a black veil of that strength, and nothing at 0", () => {
  expect(darkenOverlay(0)).toBeNull();
  expect(darkenOverlay(35)).toBe("rgb(0 0 0 / 35%)");
  expect(darkenOverlay(500)).toBe(`rgb(0 0 0 / ${BANNER_DARKEN_MAX}%)`);
});

test("a blurred picture grows past the pane so its soft edge is out of sight", () => {
  expect(blurScale(0, 800, 600)).toBe(1);
  expect(blurScale(10, 0, 600)).toBe(1);
  expect(blurScale(15, 1200, 600)).toBeCloseTo(1.1);
  expect(blurScale(40, 1200, 600)).toBeGreaterThan(blurScale(10, 1200, 600));
});
