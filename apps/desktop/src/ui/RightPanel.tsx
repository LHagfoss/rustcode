// The right panel (docs/design.md, Right panel): one floating frame for
// every side view (Changes, Agents, Pull requests), inset from the window's
// edges, with tabs between them, a close button and a draggable left edge.
// Its slot takes its width at once (one reflow of the chat column, not one
// per frame), and the frame slides and fades in and out on the compositor;
// its content stays mounted while it closes (docs/desktop.md › Native frame rate).

import { X } from "lucide-react";
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { type AppState, setPanel, showPanel, useApp } from "../state/store";
import { cx, Segments, Tooltip } from "./primitives";

const ChangesPanel = lazy(() => import("./ChangesPanel").then((m) => ({ default: m.ChangesPanel })));
const PullsPanel = lazy(() => import("./PullRequests").then((m) => ({ default: m.PullsPanel })));

type Kind = NonNullable<AppState["panel"]>;

const MIN = 320;
const MAX = 760;
const KEY = "rustcode.rightPanelWidth";
/** The gap between the panel and the window's edges. */
const INSET = 8;

function storedWidth(): number {
  try {
    const v = Number(localStorage.getItem(KEY));
    return v >= MIN && v <= MAX ? v : 460;
  } catch {
    return 460;
  }
}

export function RightPanel() {
  const panel = useApp((s) => s.panel);
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  // What the frame shows: the open panel, or while closing the one that was.
  const [shown, setShown] = useState<Kind | null>(panel);
  const [width, setWidth] = useState(storedWidth);
  const widthRef = useRef(width);
  widthRef.current = width;

  useEffect(() => {
    if (panel) setShown(panel);
  }, [panel]);

  const open = !!panel;
  const kind = panel ?? shown;

  const onResize = (e: React.PointerEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const start = widthRef.current;
    const move = (ev: PointerEvent) => setWidth(Math.min(MAX, Math.max(MIN, start - (ev.clientX - startX))));
    const up = () => {
      try {
        localStorage.setItem(KEY, String(widthRef.current));
      } catch {
        // Not kept in a private window.
      }
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const tabs: { value: Kind; label: string }[] = [
    { value: "changes", label: "Changes" },
    ...(pullsOn ? [{ value: "pulls" as const, label: "Pull requests" }] : []),
  ];

  return (
    <div
      className="relative shrink-0 overflow-hidden"
      style={{ width: kind ? width + INSET : 0 }}
      aria-hidden={!open}
    >
      {kind && (
        <aside
          aria-label="Side panel"
          className={cx(
            "rc-glass rc-drill absolute flex flex-col overflow-hidden rounded-xl shadow-2 transition-[opacity,translate] duration-200 ease-snappy",
            open ? "translate-x-0 opacity-100" : "translate-x-3 opacity-0",
          )}
          style={{ top: INSET, bottom: INSET, left: 0, width }}
          onTransitionEnd={(e) => {
            if (e.target === e.currentTarget && e.propertyName === "opacity" && !open) setShown(null);
          }}
        >
          <div data-tauri-drag-region className="flex h-11 shrink-0 items-center gap-2 pr-2 pl-2.5">
            {tabs.length > 1 ? (
              <Segments label="Panel" value={kind} options={tabs} onChange={(v) => showPanel(v)} />
            ) : (
              <span className="px-1 font-semibold text-body">{tabs[0]?.label}</span>
            )}
            <span className="flex-1" data-tauri-drag-region />
            <Tooltip label="Close (⌘⌥B)">
              <button
                type="button"
                aria-label="Close panel"
                onClick={() => setPanel(null)}
                className="flex size-7 items-center justify-center rounded-full text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
              >
                <X size={14} />
              </button>
            </Tooltip>
          </div>
          <Suspense fallback={<div className="flex-1" />}>
            {kind === "changes" && <ChangesPanel />}
            {kind === "pulls" && <PullsPanel />}
          </Suspense>
          {open && (
            <div
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize panel"
              onPointerDown={onResize}
              className="group absolute top-3 bottom-3 left-0 z-10 w-[7px] cursor-col-resize"
            >
              <div className="mx-auto h-full w-0.5 rounded-full bg-transparent group-hover:bg-fill-strong" />
            </div>
          )}
        </aside>
      )}
    </div>
  );
}
