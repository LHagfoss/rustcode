// The banner's print effects and baked blur, off the main thread (lib/banner.ts): the pixels of a
// picture already scaled to at most 2048px arrive as a transferred buffer and go back the same way,
// once per effect, theme and blur. Only plain arrays here, no OffscreenCanvas or ImageBitmap, so
// every web view that runs a worker (WebKitGTK on Linux included) runs this one.

import { type PixelEffect, processPixels } from "./bannerEffects";

export interface EffectRequest {
  id: number;
  /** RGBA, `width * height * 4` bytes. */
  pixels: ArrayBuffer;
  width: number;
  height: number;
  /** Null for the picture as it came: only blurred. */
  effect: PixelEffect | null;
  dark: boolean;
  /** Blur radius in these pixels, 0 for none (Windows and Linux; macOS blurs live). */
  blur: number;
}

export interface EffectResponse {
  id: number;
  /** The processed RGBA, or null when it failed. */
  pixels: ArrayBuffer | null;
}

self.onmessage = (event: MessageEvent<EffectRequest>) => {
  const { id, pixels, width, height, effect, dark, blur } = event.data;
  let out: ArrayBuffer | null = null;
  try {
    out = processPixels(new Uint8ClampedArray(pixels), width, height, effect, dark, blur)
      .buffer as ArrayBuffer;
  } catch {
    out = null;
  }
  self.postMessage({ id, pixels: out } satisfies EffectResponse, out ? [out] : []);
};
