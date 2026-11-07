// The banner over a new thread, after NewThreadBannerView in
// NewThreadBanner.swift: the chosen picture in coloured text (drawn once on
// an offscreen canvas, lib/banner.ts) or as is, covering the whole pane, cropped
// to fit, with a soft glow of itself under it. A picture seen before shows at
// once; a new one fades in.

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { asciiDrawing, charWidth, type Drawing, LINE_HEIGHT, loadBanner } from "../lib/banner";
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

export function NewThreadBanner({
  name,
  style,
  height,
  className,
}: {
  name: string;
  style: "ascii" | "picture";
  height: number;
  className?: string;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [art, setArt] = useState<Drawing | null>(null);
  const dark = useDarkScheme();
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

  const ready = style === "picture" ? !!image : !!art;
  const scale = art ? Math.max(width / art.width, height / art.height) : 1;

  const layer = (glow: boolean) =>
    style === "picture" && image ? (
      <img
        src={image.src}
        alt=""
        draggable={false}
        className={cx("absolute inset-0 size-full object-cover", glow && "blur-[14px]")}
        style={glow ? { opacity: dark ? 0.5 : 0.3 } : undefined}
      />
    ) : art ? (
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
        style={{ opacity: ready ? 1 : 0 }}
      >
        {layer(true)}
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
