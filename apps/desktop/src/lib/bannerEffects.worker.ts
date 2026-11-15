// The banner's print effects, off the main thread (lib/banner.ts): a picture already scaled to
// at most 2048px arrives as an ImageBitmap and goes back as a PNG, once per effect and theme.

import { applyEffect, type PixelEffect } from "./bannerEffects";

export interface EffectRequest {
  id: number;
  bitmap: ImageBitmap;
  effect: PixelEffect;
  dark: boolean;
}

export interface EffectResponse {
  id: number;
  /** Null when this web view cannot draw off screen. */
  png: Blob | null;
}

self.onmessage = async (event: MessageEvent<EffectRequest>) => {
  const { id, bitmap, effect, dark } = event.data;
  let png: Blob | null = null;
  try {
    const { width, height } = bitmap;
    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (ctx) {
      ctx.drawImage(bitmap, 0, 0);
      const pixels = ctx.getImageData(0, 0, width, height);
      pixels.data.set(applyEffect(effect, pixels.data, width, height, dark));
      ctx.putImageData(pixels, 0, 0);
      png = await canvas.convertToBlob({ type: "image/png" });
    }
  } catch {
    png = null;
  } finally {
    bitmap.close();
  }
  self.postMessage({ id, png } satisfies EffectResponse);
};
