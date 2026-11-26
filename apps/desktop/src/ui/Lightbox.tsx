import { zoomGestureModifier } from "../lib/platform";

// A picture opened from the composer or the transcript: over a dimmed, blurred
// backdrop, fit to the window. Esc or a click outside closes it, ←→ and the
// filmstrip step through every picture the chat has on screen. Pinch or primary-modifier scroll zooms at the pointer, a click on the
// picture toggles 2× there, + − 0 zoom from the keyboard, and a zoomed picture
// pans by dragging or scrolling.
//
// Gestures must not wait for a React render: the live view lives in a ref and
// the frame is written to the picture's transform straight from a
// requestAnimationFrame. Only the toolbar's percentage, and the classes that
// depend on zoom, go through state, throttled.

import { useCallback, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { ChevronLeft, ChevronRight, Minus, Plus, X } from "./icons";
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

/** Marks an element as holding these pictures, so a chat-wide lightbox can find them in order. */
export const PICTURES_ATTR = "data-rc-pictures";

export function picturesAttr(images: readonly LightboxImage[]): Record<string, string> {
  return { [PICTURES_ATTR]: JSON.stringify(images) };
}

function readPictures(el: Element): LightboxImage[] {
  try {
    const parsed: unknown = JSON.parse(el.getAttribute(PICTURES_ATTR) ?? "[]");
    return Array.isArray(parsed)
      ? parsed.filter((p): p is LightboxImage => typeof p?.src === "string" && typeof p?.alt === "string")
      : [];
  } catch {
    return [];
  }
}

/**
 * Every picture marked with `PICTURES_ATTR` under `root`, in reading order, and where the pictures of
 * `own` start among them (-1 when `own` is not under `root`).
 */
export function collectPictures(
  root: ParentNode,
  own: Element | null,
): { images: LightboxImage[]; start: number } {
  const images: LightboxImage[] = [];
  let start = -1;
  for (const el of root.querySelectorAll(`[${PICTURES_ATTR}]`)) {
    if (el === own) start = images.length;
    images.push(...readPictures(el));
  }
  return { images, start };
}

/**
 * Opens `images[index]` among every picture of the chat `from` sits in, so ←→ go on past its own
 * message. Outside a chat it opens just `images`.
 */
export function openChatLightbox(from: Element, images: LightboxImage[], index = 0) {
  const own = from.closest(`[${PICTURES_ATTR}]`);
  const chat = from.closest('[role="log"]');
  if (!own || !chat) return openLightbox(images, index);
  const all = collectPictures(chat, own);
  if (all.start < 0 || all.images.length <= images.length) return openLightbox(images, index);
  openLightbox(all.images, all.start + index);
}

function goTo(index: number) {
  if (!state || index === state.index) return;
  state = { ...state, index };
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
  const strip = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    if (index === undefined) return;
    strip.current?.children[index]?.scrollIntoView({ block: "nearest", inline: "center" });
  }, [index]);

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
      if (zoomGestureModifier(e)) {
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
      className="rc-overlay rc-lightbox-scrim fixed inset-0 z-[60]"
      data-state="open"
    >
      <div
        ref={stage}
        className={cx(
          "absolute inset-0 flex items-center justify-center overflow-hidden p-12",
          many && "pb-32",
        )}
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
          <div
            ref={strip}
            role="tablist"
            aria-label="Pictures"
            className="absolute bottom-[68px] left-1/2 flex max-w-[min(720px,80%)] -translate-x-1/2 gap-1.5 overflow-x-auto rounded-xl bg-surface-2 p-1.5 shadow-1 [scrollbar-width:none]"
          >
            {current.images.map((picture, i) => (
              <button
                key={`${i}:${picture.src.slice(0, 64)}`}
                type="button"
                role="tab"
                aria-selected={i === current.index}
                aria-label={picture.alt || `Picture ${i + 1}`}
                onClick={() => goTo(i)}
                className={cx(
                  "size-11 shrink-0 overflow-hidden rounded-md outline-offset-1 transition-opacity",
                  i === current.index
                    ? "opacity-100 outline-2 outline-accent"
                    : "opacity-55 hover:opacity-100",
                )}
              >
                <img src={picture.src} alt="" draggable={false} className="size-full object-cover" />
              </button>
            ))}
          </div>
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
