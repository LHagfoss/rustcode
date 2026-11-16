// The banner over a new thread, after NewThreadBannerView in
// NewThreadBanner.swift: the chosen picture in coloured text (drawn once on
// an offscreen canvas, lib/banner.ts) or as is, covering the whole pane, cropped
// to fit, with a soft glow of itself under it. A picture seen before shows at
// once; a new one fades in.

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import {
  asciiDrawing,
  type BannerStyle,
  blurScale,
  cachedEffect,
  charWidth,
  type Drawing,
  LINE_HEIGHT,
  loadBanner,
  renderEffect,
  renderSize,
} from "../lib/banner";
import { isPixelEffect } from "../lib/bannerEffects";
import { paintsBlur } from "../lib/platform";
import { observeSize } from "../lib/resize";
import { cx } from "./primitives";

function subscribeScheme(listener: () => void) {
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  media.addEventListener("change", listener);
  const observer = new MutationObserver(listener);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return () => {
    media.removeEventListener("change", listener);
    observer.disconnect();
  };
}

function isDark(): boolean {
  const theme = document.documentElement.dataset.theme;
  if (theme) return theme === "dark";
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

export function useDarkScheme(): boolean {
  return useSyncExternalStore(subscribeScheme, isDark);
}

/** The haze style: blur in px and how much of the picture shows through the pane. */
const HAZE_BLUR = 24;
const HAZE_OPACITY = 0.55;

export function NewThreadBanner({
  name,
  style,
  height,
  blur = 0,
  className,
}: {
  name: string;
  style: BannerStyle;
  height: number;
  /** Blur radius in px: live on macOS, where the picture grows a little so the soft edge falls
   * outside the pane; baked into the picture once on Windows and Linux. */
  blur?: number;
  className?: string;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [art, setArt] = useState<Drawing | null>(null);
  const dark = useDarkScheme();
  const effect = isPixelEffect(style) ? style : null;
  // The print effect's PNG (rendered once in a worker, lib/banner.ts); "" when it cannot be, and
  // then the picture shows as is.
  const [printed, setPrinted] = useState<string | null>(() =>
    effect ? (cachedEffect(name, effect, dark) ?? null) : null,
  );
  // Whether a drawing shows: then a resize waits before drawing again.
  const drawn = useRef(false);
  drawn.current = !!art;

  useEffect(() => {
    const el = root.current;
    if (!el) return;
    setWidth(el.offsetWidth);
    return observeSize(el, () => setWidth(el.offsetWidth));
  }, []);

  useEffect(() => {
    let live = true;
    setImage(null);
    setArt(null);
    void loadBanner(name).then((img) => live && setImage(img));
    return () => {
      live = false;
    };
  }, [name]);

  const columns = image && width ? Math.max(20, Math.min(Math.floor(width / charWidth()), 480)) : 0;
  const rows = Math.max(4, Math.floor(height / LINE_HEIGHT));

  useEffect(() => {
    if (!image || !columns || style !== "ascii") return;
    // Settle a resize before drawing again; the last drawing scales meanwhile.
    const draw = () => setArt(asciiDrawing(name, image, columns, rows, dark, window.devicePixelRatio || 2));
    if (!drawn.current) {
      draw();
      return;
    }
    const t = setTimeout(draw, 120);
    return () => clearTimeout(t);
  }, [image, columns, rows, dark, style, name]);

  // macOS blurs live; Windows and Linux paint no blur (docs/design.md › Platforms), so there the
  // blur is baked into the picture once by the worker instead, and nothing blurs on screen.
  const blurred = paintsBlur();
  const haze = style === "haze";
  // Haze is the picture blurred well past the slider and faded into the pane.
  const wanted = haze ? Math.max(blur, HAZE_BLUR) : blur;
  const painted = blurred ? wanted : 0;
  const baked = blurred ? 0 : wanted;
  const scale = art ? Math.max(width / art.width, height / art.height) : 1;

  // What the worker draws from, and what it is called in its cache.
  const source = style === "ascii" ? (art?.canvas ?? null) : image;
  const look =
    style === "ascii" ? (art ? `ascii ${art.canvas.width}x${art.canvas.height}` : "") : (effect ?? "picture");
  const needsWorker = !!effect || baked > 0;
  // The blur in the drawn copy's pixels: the CSS radius over how much that copy is scaled on screen.
  let radius = 0;
  if (baked > 0 && source && width) {
    const size = renderSize(source);
    const shown =
      style === "ascii" && art
        ? art.width * scale
        : size.width * Math.max(width / size.width, height / size.height);
    radius = shown > 0 ? Math.max(1, Math.round((baked * size.width) / shown)) : 0;
  }
  const printedShows = useRef(false);
  printedShows.current = !!printed;

  useEffect(() => {
    if (!needsWorker) {
      setPrinted(null);
      return;
    }
    if (!source || (baked > 0 && !radius)) return;
    let live = true;
    const cached = cachedEffect(name, look, dark, radius);
    if (cached) {
      setPrinted(cached);
      return;
    }
    const run = () =>
      void renderEffect(name, source, look, dark, radius).then((url) => live && setPrinted(url ?? ""));
    // A resize or a slider drag settles first while the last render shows.
    const t = printedShows.current ? setTimeout(run, 150) : undefined;
    if (!t) run();
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [source, look, needsWorker, baked, radius, dark, name]);

  // Still rendering: nothing shows yet, and it fades in. "" is a render that cannot be made here,
  // and then the picture or the text shows as is.
  const pending = needsWorker && printed === null;
  const rendered = needsWorker && printed ? printed : null;
  // Drawn as an image: as is, hazed, printed or blurred.
  const photo = style === "ascii" || pending ? null : (rendered ?? image?.src ?? null);
  // The text drawing, blurred by the worker, in its place.
  const artPhoto = style === "ascii" ? rendered : null;
  const showsArt = style === "ascii" && !!art && !pending;
  const ready = style === "ascii" ? showsArt : !!photo;

  const layer = (glow: boolean) =>
    photo ? (
      <img
        src={photo}
        alt=""
        draggable={false}
        className={cx("absolute inset-0 size-full object-cover", glow && "blur-[14px]")}
        style={glow ? { opacity: dark ? 0.5 : 0.3 } : undefined}
      />
    ) : showsArt && art && artPhoto ? (
      <img
        src={artPhoto}
        alt=""
        draggable={false}
        className={cx("absolute top-0 left-1/2 max-w-none", glow && "blur-[14px]")}
        style={{
          width: art.width,
          height: art.height,
          transform: `translateX(-50%) scale(${scale})`,
          transformOrigin: "top center",
          opacity: glow ? (dark ? 0.5 : 0.3) : 1,
        }}
      />
    ) : showsArt && art ? (
      <ArtCanvas
        art={art}
        className={cx("absolute top-0 left-1/2", glow && "blur-[14px]")}
        style={{
          width: art.width,
          height: art.height,
          transform: `translateX(-50%) scale(${scale})`,
          transformOrigin: "top center",
          opacity: glow ? (dark ? 0.5 : 0.3) : 1,
        }}
      />
    ) : null;

  return (
    <div
      ref={root}
      aria-hidden="true"
      className={cx("pointer-events-none relative w-full overflow-hidden", className)}
      style={{ height }}
    >
      <div
        className="absolute inset-0 transition-opacity duration-[250ms] ease-out"
        style={{
          opacity: ready ? (haze ? HAZE_OPACITY : 1) : 0,
          ...(painted > 0
            ? { filter: `blur(${painted}px)`, transform: `scale(${blurScale(painted, width, height)})` }
            : undefined),
        }}
      >
        {blurred && !haze && layer(true)}
        {layer(false)}
      </div>
    </div>
  );
}

/** The offscreen drawing copied into a canvas in the page. */
function ArtCanvas({
  art,
  className,
  style,
}: {
  art: Drawing;
  className?: string;
  style?: React.CSSProperties;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.width = art.canvas.width;
    el.height = art.canvas.height;
    el.getContext("2d")?.drawImage(art.canvas, 0, 0);
  }, [art]);
  return <canvas ref={ref} className={className} style={style} />;
}
