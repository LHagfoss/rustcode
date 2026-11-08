// A picture opened from the composer or the transcript: over a dimmed, blurred
// backdrop, fit to the window. Esc or a click outside closes it, ←→ step through
// the message's pictures. Pinch or ⌘-scroll zooms at the pointer, a click on the
// picture toggles 2× there, + − 0 zoom from the keyboard, and a zoomed picture
// pans by dragging or scrolling.
//
// Gestures must not wait for a React render: the live view lives in a ref and
// the frame is written to the picture's transform straight from a
// requestAnimationFrame. Only the toolbar's percentage, and the classes that
// depend on zoom, go through state, throttled.

import { ChevronLeft, ChevronRight, Minus, Plus, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
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

/** The easing a discrete action (a button, a key, a double-click) gets; gestures get none. */
const EASE = "transform 120ms var(--ease-snappy, ease-out)";
/** How often a running gesture may push the toolbar's percentage into state. */
const ZOOM_SYNC_MS = 100;
/** Once the wheel has been quiet this long, the toolbar takes the exact zoom. */
const ZOOM_IDLE_MS = 150;

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

/** More travel than this and a press was a pan, not a click. */
const CLICK_SLOP_PX = 4;

export function Lightbox() {
  const current = useSyncExternalStore(subscribe, () => state);
  const open = current !== null;
  // The view the picture is actually at: gestures write it and repaint in a
  // frame, so a wheel or a drag never waits for a render.
  const view = useRef<View>(FITTED);
  // Only what the toolbar and the cursor need, so React renders stay rare.
  const [zoomLevel, setZoomLevel] = useState(MIN_ZOOM);
  const stage = useRef<HTMLDivElement>(null);
  const picture = useRef<HTMLImageElement>(null);
  const frame = useRef(0);
  const eased = useRef(false);
  const dragging = useRef<{ x: number; y: number } | null>(null);
  // How far this press travelled: a pan that ends on the picture is not a click on it.
  const pressTravel = useRef(0);
  const zoomSyncedAt = useRef(0);
  const zoomIdle = useRef<ReturnType<typeof setTimeout> | null>(null);
  const index = current?.index;

  // Writes the live view onto the picture. Runs once per frame, at most.
  const paint = useCallback(() => {
    if (frame.current) cancelAnimationFrame(frame.current);
    frame.current = 0;
    const el = picture.current;
    if (!el) return;
    const { zoom, x, y } = view.current;
    const transition = eased.current ? EASE : "none";
    if (el.style.transition !== transition) el.style.transition = transition;
    el.style.transform = `translate(${x}px, ${y}px) scale(${zoom})`;
  }, []);

  const schedule = useCallback(() => {
    if (!frame.current) frame.current = requestAnimationFrame(paint);
  }, [paint]);

  const commit = useCallback(
    (next: View, options?: { eased?: boolean; live?: boolean }) => {
      view.current = next;
      eased.current = options?.eased ?? false;
      schedule();
      if (!options?.live) {
        setZoomLevel(next.zoom);
        return;
      }
      // Mid-gesture: catch the toolbar up on a throttle, then exactly once idle.
      const now = performance.now();
      if (now - zoomSyncedAt.current >= ZOOM_SYNC_MS) {
        zoomSyncedAt.current = now;
        setZoomLevel(next.zoom);
      }
      if (zoomIdle.current) clearTimeout(zoomIdle.current);
      zoomIdle.current = setTimeout(() => setZoomLevel(view.current.zoom), ZOOM_IDLE_MS);
    },
    [schedule],
  );

  const centre = useCallback(() => {
    const r = stage.current?.getBoundingClientRect();
    return r ? { cx: r.left + r.width / 2, cy: r.top + r.height / 2 } : { cx: 0, cy: 0 };
  }, []);

  const zoomBy = useCallback(
    (factor: number) => commit(zoomAt(view.current, view.current.zoom * factor, 0, 0), { eased: true }),
    [commit],
  );
  const fit = useCallback(() => commit(FITTED, { eased: true }), [commit]);

  // A new picture starts fitted.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `index` is the trigger
  useEffect(() => {
    view.current = FITTED;
    eased.current = false;
    setZoomLevel(MIN_ZOOM);
    paint();
  }, [index, paint]);

  useEffect(
    () => () => {
      if (frame.current) cancelAnimationFrame(frame.current);
      if (zoomIdle.current) clearTimeout(zoomIdle.current);
    },
    [],
  );

  // Fitted, or wherever the view is, as soon as the element exists.
  const attachPicture = useCallback((el: HTMLImageElement | null) => {
    picture.current = el;
    if (!el) return;
    const { zoom, x, y } = view.current;
    el.style.transform = `translate(${x}px, ${y}px) scale(${zoom})`;
  }, []);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
      else if (e.key === "ArrowLeft") step(-1);
      else if (e.key === "ArrowRight") step(1);
      else if (e.key === "+" || e.key === "=") zoomBy(1.5);
      else if (e.key === "-") zoomBy(1 / 1.5);
      else if (e.key === "0") fit();
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, zoomBy, fit]);

  // Not a React listener: wheel must be non-passive to keep the page from zooming.
  // Registered once per opening, not on every render.
  useEffect(() => {
    if (!open) return;
    const el = stage.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const v = view.current;
      if (e.ctrlKey || e.metaKey) {
        // A trackpad pinch arrives as ctrl+wheel.
        const { cx, cy } = centre();
        const factor = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.002));
        commit(zoomAt(v, v.zoom * factor, e.clientX - cx, e.clientY - cy), { live: true });
      } else if (v.zoom > MIN_ZOOM) {
        commit({ zoom: v.zoom, x: v.x - e.deltaX, y: v.y - e.deltaY }, { live: true });
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [open, centre, commit]);

  const endDrag = useCallback(() => {
    if (!dragging.current) return;
    dragging.current = null;
    setZoomLevel(view.current.zoom);
  }, []);

  if (!current) return null;
  const image = current.images[current.index]!;
  const many = current.images.length > 1;
  const zoomed = zoomLevel > MIN_ZOOM;
  // Solid, not glass: these sit over a picture that moves, and a backdrop blur
  // there would be recomputed on every frame of a pan. The scrim's own blur sits
  // under the picture, over a transcript that holds still.
  const round =
    "absolute flex size-9 items-center justify-center rounded-full bg-surface-2 text-fg shadow-1 hover:bg-surface-3";
  const nav = cx(round, "top-1/2 -translate-y-1/2");
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={image.alt || "Picture"}
      className="rc-overlay rc-lightbox-scrim fixed inset-0 z-[60] [-webkit-backdrop-filter:var(--rc-glass-filter)] [backdrop-filter:var(--rc-glass-filter)]"
      data-state="open"
    >
      <div
        ref={stage}
        className="absolute inset-0 flex items-center justify-center overflow-hidden p-12"
        onMouseDown={(e) => e.target === e.currentTarget && close()}
      >
        <img
          ref={attachPicture}
          src={image.src}
          alt={image.alt}
          draggable={false}
          decoding="async"
          onPointerDown={(e) => {
            pressTravel.current = 0;
            if (view.current.zoom <= MIN_ZOOM || e.button !== 0) return;
            e.currentTarget.setPointerCapture(e.pointerId);
            dragging.current = { x: e.clientX, y: e.clientY };
          }}
          onPointerMove={(e) => {
            const d = dragging.current;
            if (!d) return;
            const v = view.current;
            pressTravel.current += Math.abs(e.clientX - d.x) + Math.abs(e.clientY - d.y);
            commit({ zoom: v.zoom, x: v.x + e.clientX - d.x, y: v.y + e.clientY - d.y }, { live: true });
            d.x = e.clientX;
            d.y = e.clientY;
          }}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          onLostPointerCapture={endDrag}
          onClick={(e) => {
            if (pressTravel.current > CLICK_SLOP_PX) return;
            const { cx: ccx, cy: ccy } = centre();
            const v = view.current;
            commit(v.zoom > MIN_ZOOM ? FITTED : zoomAt(v, 2, e.clientX - ccx, e.clientY - ccy), {
              eased: true,
            });
          }}
          className={cx(
            "max-h-full max-w-full rounded-lg object-contain will-change-transform",
            zoomed ? "cursor-grab active:cursor-grabbing" : "cursor-zoom-in",
          )}
        />
      </div>
      <button type="button" aria-label="Close" onClick={close} className={cx(round, "top-6 right-6")}>
        <X size={16} />
      </button>
      <div className="absolute bottom-5 left-1/2 flex -translate-x-1/2 items-center gap-0.5 rounded-full bg-surface-2 p-1 text-callout text-fg-muted shadow-1">
        <button
          type="button"
          aria-label="Zoom out"
          disabled={!zoomed}
          onClick={() => zoomBy(1 / 1.5)}
          className="flex size-7 items-center justify-center rounded-full hover:bg-fill disabled:opacity-40"
        >
          <Minus size={14} />
        </button>
        <Pill quiet aria-label="Fit to window" className="min-w-[52px] tabular" onClick={fit}>
          {Math.round(zoomLevel * 100)}%
        </Pill>
        <button
          type="button"
          aria-label="Zoom in"
          disabled={zoomLevel >= MAX_ZOOM}
          onClick={() => zoomBy(1.5)}
          className="flex size-7 items-center justify-center rounded-full hover:bg-fill disabled:opacity-40"
        >
          <Plus size={14} />
        </button>
        {many && (
          <span className="border-border border-l px-2 tabular">
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
