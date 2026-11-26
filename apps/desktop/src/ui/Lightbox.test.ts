import { describe, expect, test } from "bun:test";
import { collectPictures, picturesAttr, zoomAt } from "./Lightbox";
import { galleryLayout } from "./MessageGallery";

describe("lightbox zoom", () => {
  test("keeps the point under the pointer still", () => {
    const v = zoomAt({ zoom: 1, x: 0, y: 0 }, 2, 100, 50);
    expect(v.x + (100 / 1) * 0 + ((100 - v.x) / v.zoom) * v.zoom).toBe(100);
    expect(v).toEqual({ zoom: 2, x: -100, y: -50 });
  });
  test("clamps and recentres at 1×", () => {
    expect(zoomAt({ zoom: 2, x: -100, y: -50 }, 0.5, 0, 0)).toEqual({ zoom: 1, x: 0, y: 0 });
    expect(zoomAt({ zoom: 4, x: 0, y: 0 }, 100, 0, 0).zoom).toBe(8);
  });
});

describe("chat pictures", () => {
  const holder = (images: unknown) => {
    const attrs = picturesAttr(images as never);
    return { getAttribute: (name: string) => attrs[name] ?? null } as unknown as Element;
  };
  test("collects every marked picture in order and finds where the clicked message starts", () => {
    const first = holder([{ src: "a", alt: "A" }]);
    const own = holder([
      { src: "b", alt: "B" },
      { src: "c", alt: "C" },
    ]);
    const last = holder([{ src: "d", alt: "D" }]);
    const root = { querySelectorAll: () => [first, own, last] } as unknown as ParentNode;
    const { images, start } = collectPictures(root, own);
    expect(images.map((i) => i.src)).toEqual(["a", "b", "c", "d"]);
    expect(start).toBe(1);
  });
  test("skips malformed entries and reports a message outside the chat", () => {
    const bad = holder([{ src: 1 }, { src: "ok", alt: "" }]);
    const root = { querySelectorAll: () => [bad] } as unknown as ParentNode;
    expect(collectPictures(root, null)).toEqual({ images: [{ src: "ok", alt: "" }], start: -1 });
  });
});

describe("message gallery", () => {
  test("shows up to three tiles and counts the rest on the last", () => {
    expect(galleryLayout(1)).toEqual({ tiles: 1, more: 0 });
    expect(galleryLayout(3)).toEqual({ tiles: 3, more: 0 });
    expect(galleryLayout(7)).toEqual({ tiles: 3, more: 4 });
  });
});
