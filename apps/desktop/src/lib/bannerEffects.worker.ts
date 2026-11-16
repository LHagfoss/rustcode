// The banner's print effects, off the main thread (lib/banner.ts): a picture already scaled to
// at most 2048px arrives as an ImageBitmap and goes back as a PNG, once per effect, theme and
// blur. Windows and Linux also bake the Blur setting in here, since they paint no live blur.

import { applyEffect, boxBlur, type PixelEffect } from "./bannerEffects";

export interface EffectRequest {
  id: number;
  bitmap: ImageBitmap;
  /** Null for the picture as it came: only blurred. */
  effect: PixelEffect | null;
  dark: boolean;
  /** Blur radius in the bitmap's pixels, 0 for none (Windows and Linux; macOS blurs live). */
  blur: number;
}

export interface EffectResponse {
  id: number;
  /** Null when this web view cannot draw off screen. */
  png: Blob | null;
}

self.onmessage = async (event: MessageEvent<EffectRequest>) => {
  const { id, bitmap, effect, dark, blur } = event.data;
  let png: Blob | null = null;
  try {
    const { width, height } = bitmap;
    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (ctx) {
      ctx.drawImage(bitmap, 0, 0);
      const pixels = ctx.getImageData(0, 0, width, height);
      let data = effect ? applyEffect(effect, pixels.data, width, height, dark) : pixels.data;
      if (blur > 0) data = boxBlur(data, width, height, blur);
      if (data !== pixels.data) pixels.data.set(data);
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
