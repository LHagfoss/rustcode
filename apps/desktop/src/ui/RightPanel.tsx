// The right panel (docs/design.md, Right panel): one floating frame for
// every side view (Changes, Agents, Pull requests), inset from the window's
// edges, with tabs between them, a close button and a draggable left edge.
// It slides open by animating its slot's width, so the chat column reflows
// with it; its content stays mounted while it closes.

import { X } from "lucide-react";
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { type AppState, hasAgents, setPanel, showPanel, useApp } from "../state/store";
import { AgentsPanel } from "./AgentViews";
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
  const agentsOn = useApp(
    (s) => hasAgents(s.selectedId ? s.threads[s.selectedId] : undefined) && !!s.selectedId,
  );
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  // What the frame shows: the open panel, or while closing the one that was.
  const [shown, setShown] = useState<Kind | null>(panel);
  const [width, setWidth] = useState(storedWidth);
  const [dragging, setDragging] = useState(false);
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
    setDragging(true);
    const move = (ev: PointerEvent) => setWidth(Math.min(MAX, Math.max(MIN, start - (ev.clientX - startX))));
    const up = () => {
      setDragging(false);
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
    ...(agentsOn ? [{ value: "agents" as const, label: "Agents" }] : []),
    ...(pullsOn ? [{ value: "pulls" as const, label: "Pull requests" }] : []),
  ];

  return (
    <div
      className={cx(
        "relative shrink-0 overflow-hidden",
        !dragging && "transition-[width] duration-200 ease-snappy",
      )}
      style={{ width: open ? width + INSET : 0 }}
      onTransitionEnd={(e) => {
        if (e.target === e.currentTarget && !open) setShown(null);
      }}
      aria-hidden={!open}
    >
      {kind && (
        <aside
          aria-label="Side panel"
          className={cx(
            "rc-glass absolute flex flex-col overflow-hidden rounded-xl shadow-2 transition-[opacity,transform] duration-200 ease-snappy",
            open ? "translate-x-0 opacity-100" : "translate-x-3 opacity-0",
          )}
          style={{ top: INSET, bottom: INSET, left: 0, width }}
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
            {kind === "agents" && <AgentsPanel />}
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
