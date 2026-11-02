// A picture opened from the composer or the transcript: over a dimmed,
// frosted backdrop, fit to the window. Esc or a click outside closes it,
// ←→ step through the message's pictures, a click on the picture zooms it
// to its own size (and back).

import { ChevronLeft, ChevronRight, X } from "lucide-react";
import { useEffect, useState, useSyncExternalStore } from "react";
import { cx } from "./primitives";

export interface LightboxImage {
  src: string;
  alt: string;
}

let state: { images: LightboxImage[]; index: number } | null = null;
const listeners = new Set<() => void>();
const emit = () => {
  for (const l of listeners) l();
};

export function openLightbox(images: LightboxImage[], index = 0) {
  if (!images.length) return;
  state = { images, index: Math.max(0, Math.min(index, images.length - 1)) };
  emit();
}

function close() {
  state = null;
  emit();
}

function step(by: number) {
  if (!state) return;
  const n = state.images.length;
  state = { ...state, index: (state.index + by + n) % n };
  emit();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export function Lightbox() {
  const current = useSyncExternalStore(subscribe, () => state);
  const [zoomed, setZoomed] = useState(false);
  const index = current?.index;
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new picture starts fitted
  useEffect(() => setZoomed(false), [index]);
  useEffect(() => {
    if (!current) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
      else if (e.key === "ArrowLeft") step(-1);
      else if (e.key === "ArrowRight") step(1);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [current]);
  if (!current) return null;
  const image = current.images[current.index]!;
  const many = current.images.length > 1;
  const round =
    "rc-glass absolute flex size-9 items-center justify-center rounded-full text-fg shadow-1 hover:bg-surface-3";
  const nav = cx(round, "top-1/2 -translate-y-1/2");
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={image.alt || "Picture"}
      className="rc-overlay rc-glass fixed inset-0 z-[60] flex items-center justify-center bg-bg/60"
      data-state="open"
      onMouseDown={(e) => e.target === e.currentTarget && close()}
    >
      <div
        className={cx("max-h-full max-w-full", zoomed ? "overflow-auto" : "p-12")}
        onMouseDown={(e) => e.target === e.currentTarget && close()}
      >
        <img
          src={image.src}
          alt={image.alt}
          draggable={false}
          onClick={() => setZoomed(!zoomed)}
          className={cx(
            "rounded-lg shadow-2",
            zoomed
              ? "max-w-none cursor-zoom-out"
              : "max-h-[calc(100vh-96px)] max-w-[calc(100vw-96px)] cursor-zoom-in object-contain",
          )}
        />
      </div>
      <button type="button" aria-label="Close" onClick={close} className={cx(round, "top-6 right-6")}>
        <X size={16} />
      </button>
      {many && (
        <>
          <button
            type="button"
            aria-label="Previous picture"
            onClick={() => step(-1)}
            className={cx(nav, "left-5")}
          >
            <ChevronLeft size={18} />
          </button>
          <button
            type="button"
            aria-label="Next picture"
            onClick={() => step(1)}
            className={cx(nav, "right-5")}
          >
            <ChevronRight size={18} />
          </button>
          <div className="rc-glass absolute bottom-5 left-1/2 -translate-x-1/2 rounded-full px-3 py-1 text-callout text-fg-muted tabular">
            {current.index + 1} / {current.images.length}
          </div>
        </>
      )}
    </div>
  );
}
