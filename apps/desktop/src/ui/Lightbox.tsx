// A picture opened from the composer or the transcript: over a dimmed,
// frosted backdrop, fit to the window. Esc or a click outside closes it,
// ←→ step through the message's pictures. Pinch or ⌘-scroll zooms at the
// pointer, a double-click toggles 2× there, + − 0 zoom from the keyboard,
// and a zoomed picture pans by dragging or scrolling.

import { ChevronLeft, ChevronRight, Minus, Plus, X } from "lucide-react";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { cx, Pill } from "./primitives";

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

const MIN_ZOOM = 1;
const MAX_ZOOM = 8;
const clampZoom = (z: number) => Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z));

interface View {
  zoom: number;
  x: number;
  y: number;
}
const FITTED: View = { zoom: 1, x: 0, y: 0 };

/**
 * Zooms to `zoom` keeping the point at (`px`, `py`), relative to the stage's
 * centre, where it is on screen. Back at 1× the picture recentres.
 */
export function zoomAt(view: View, zoom: number, px: number, py: number): View {
  const next = clampZoom(zoom);
  if (next === MIN_ZOOM) return FITTED;
  const k = next / view.zoom;
  return { zoom: next, x: px - (px - view.x) * k, y: py - (py - view.y) * k };
}

export function Lightbox() {
  const current = useSyncExternalStore(subscribe, () => state);
  const [view, setView] = useState<View>(FITTED);
  const stage = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x: number; y: number; moved: boolean } | null>(null);
  const index = current?.index;
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new picture starts fitted
  useEffect(() => setView(FITTED), [index]);
  const centre = () => {
    const r = stage.current?.getBoundingClientRect();
    return r ? { cx: r.left + r.width / 2, cy: r.top + r.height / 2 } : { cx: 0, cy: 0 };
  };
  useEffect(() => {
    if (!current) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
      else if (e.key === "ArrowLeft") step(-1);
      else if (e.key === "ArrowRight") step(1);
      else if (e.key === "+" || e.key === "=") setView((v) => zoomAt(v, v.zoom * 1.5, 0, 0));
      else if (e.key === "-") setView((v) => zoomAt(v, v.zoom / 1.5, 0, 0));
      else if (e.key === "0") setView(FITTED);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [current]);
  // Not a React listener: wheel must be non-passive to keep the page from zooming.
  useEffect(() => {
    const el = stage.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const { cx, cy } = centre();
      if (e.ctrlKey || e.metaKey) {
        // A trackpad pinch arrives as ctrl+wheel.
        const factor = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.002));
        setView((v) => zoomAt(v, v.zoom * factor, e.clientX - cx, e.clientY - cy));
      } else {
        setView((v) => (v.zoom > 1 ? { ...v, x: v.x - e.deltaX, y: v.y - e.deltaY } : v));
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  });
  if (!current) return null;
  const image = current.images[current.index]!;
  const many = current.images.length > 1;
  const zoomed = view.zoom > 1;
  const round =
    "rc-glass absolute flex size-9 items-center justify-center rounded-full text-fg shadow-1 hover:bg-surface-3";
  const nav = cx(round, "top-1/2 -translate-y-1/2");
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={image.alt || "Picture"}
      className="rc-overlay rc-glass fixed inset-0 z-[60] bg-bg/60"
      data-state="open"
    >
      <div
        ref={stage}
        className="absolute inset-0 flex items-center justify-center overflow-hidden p-12"
        onMouseDown={(e) => e.target === e.currentTarget && close()}
      >
        <img
          src={image.src}
          alt={image.alt}
          draggable={false}
          onPointerDown={(e) => {
            if (!zoomed || e.button !== 0) return;
            e.currentTarget.setPointerCapture(e.pointerId);
            drag.current = { x: e.clientX, y: e.clientY, moved: false };
          }}
          onPointerMove={(e) => {
            const d = drag.current;
            if (!d) return;
            const dx = e.clientX - d.x;
            const dy = e.clientY - d.y;
            d.x = e.clientX;
            d.y = e.clientY;
            d.moved = true;
            setView((v) => ({ ...v, x: v.x + dx, y: v.y + dy }));
          }}
          onPointerUp={() => {
            drag.current = null;
          }}
          onDoubleClick={(e) => {
            const { cx, cy } = centre();
            setView((v) => (v.zoom > 1 ? FITTED : zoomAt(v, 2, e.clientX - cx, e.clientY - cy)));
          }}
          style={{
            transform: `translate(${view.x}px, ${view.y}px) scale(${view.zoom})`,
            transition: drag.current ? "none" : "transform 120ms var(--ease-snappy, ease-out)",
          }}
          className={cx(
            "max-h-full max-w-full rounded-lg object-contain shadow-2 will-change-transform",
            zoomed ? "cursor-grab active:cursor-grabbing" : "cursor-zoom-in",
          )}
        />
      </div>
      <button type="button" aria-label="Close" onClick={close} className={cx(round, "top-6 right-6")}>
        <X size={16} />
      </button>
      <div className="rc-glass absolute bottom-5 left-1/2 flex -translate-x-1/2 items-center gap-0.5 rounded-full p-1 text-callout text-fg-muted shadow-1">
        <button
          type="button"
          aria-label="Zoom out"
          disabled={!zoomed}
          onClick={() => setView((v) => zoomAt(v, v.zoom / 1.5, 0, 0))}
          className="flex size-7 items-center justify-center rounded-full hover:bg-fill disabled:opacity-40"
        >
          <Minus size={14} />
        </button>
        <Pill
          quiet
          aria-label="Fit to window"
          className="min-w-[52px] tabular"
          onClick={() => setView(FITTED)}
        >
          {Math.round(view.zoom * 100)}%
        </Pill>
        <button
          type="button"
          aria-label="Zoom in"
          disabled={view.zoom >= MAX_ZOOM}
          onClick={() => setView((v) => zoomAt(v, v.zoom * 1.5, 0, 0))}
          className="flex size-7 items-center justify-center rounded-full hover:bg-fill disabled:opacity-40"
        >
          <Plus size={14} />
        </button>
        {many && (
          <span className="border-border border-l px-2.5 tabular">
            {current.index + 1} / {current.images.length}
          </span>
        )}
      </div>
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
        </>
      )}
    </div>
  );
}
