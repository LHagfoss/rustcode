import { type CSSProperties, type ReactNode, useLayoutEffect, useRef } from "react";
import type { AppState } from "../state/store";
import type { LucideIcon } from "./icons";
import { cx, Tooltip } from "./primitives";

/** The views the panel shows. */
export type PanelKind = NonNullable<AppState["panel"]>;

export interface PanelTab {
  kind: PanelKind;
  label: string;
  icon: LucideIcon;
}

/** How long the selection takes to slide to another tab, and the tab's name to unfold. */
const SLIDE_MS = 220;

const easeOutCubic = (t: number) => 1 - (1 - t) ** 3;

interface Span {
  x: number;
  w: number;
}

/**
 * Slides the selection's fill from the tab it was on to the one picked, following that tab while
 * its name unfolds, so the fill and the tab land together.
 */
function useSlidingMark(
  list: React.RefObject<HTMLDivElement | null>,
  mark: React.RefObject<HTMLSpanElement | null>,
  key: string,
) {
  const last = useRef<Span | null>(null);
  useLayoutEffect(() => {
    const host = list.current;
    const fill = mark.current;
    if (!host || !fill || !key) return;
    const target = (): Span | null => {
      const tab = host.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]');
      return tab ? { x: tab.offsetLeft, w: tab.offsetWidth } : null;
    };
    const from = last.current;
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const start = performance.now();
    let frame = 0;
    const step = (now: number) => {
      const to = target();
      if (!to) {
        fill.style.opacity = "0";
        return;
      }
      const t = from && !still ? Math.min(1, (now - start) / SLIDE_MS) : 1;
      const e = easeOutCubic(t);
      const at = from ? { x: from.x + (to.x - from.x) * e, w: from.w + (to.w - from.w) * e } : to;
      fill.style.opacity = "1";
      fill.style.transform = `translateX(${at.x}px)`;
      fill.style.width = `${at.w}px`;
      last.current = at;
      if (t < 1) frame = requestAnimationFrame(step);
    };
    step(start);
    return () => cancelAnimationFrame(frame);
  }, [list, mark, key]);
}

/** One view's tab: its icon alone, and its name unfolding beside it while it is the one showing. */
function PanelTabButton({ tab, active, onSelect }: { tab: PanelTab; active: boolean; onSelect: () => void }) {
  const Icon = tab.icon;
  return (
    <Tooltip label={tab.label}>
      <button
        type="button"
        role="tab"
        aria-selected={active}
        aria-label={tab.label}
        onClick={onSelect}
        className={cx(
          "relative inline-flex h-7 shrink-0 items-center rounded-full px-[7px] text-callout",
          active ? "font-medium text-fg" : "text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong",
        )}
      >
        <Icon size={14} className="shrink-0" />
        <span
          className="grid transition-[grid-template-columns,opacity] ease-snappy"
          style={{
            gridTemplateColumns: active ? "1fr" : "0fr",
            opacity: active ? 1 : 0,
            transitionDuration: `${SLIDE_MS}ms`,
          }}
        >
          <span className="min-w-0 overflow-hidden whitespace-nowrap">
            <span className="block pr-1 pl-1.5">{tab.label}</span>
          </span>
        </span>
      </button>
    </Tooltip>
  );
}

/**
 * The panel's tab row: a fill slides between the tabs as the view changes, and the picked tab's
 * name unfolds while the one left folds away. `end` stands at the row's far end (the toggle).
 */
export function PanelTabs({
  tabs,
  active,
  onSelect,
  end,
  height,
}: {
  tabs: PanelTab[];
  active: PanelKind;
  onSelect: (kind: PanelKind) => void;
  end: ReactNode;
  height: number;
}) {
  const list = useRef<HTMLDivElement>(null);
  const mark = useRef<HTMLSpanElement>(null);
  const activeLabel = tabs.find((t) => t.kind === active)?.label ?? "";
  useSlidingMark(list, mark, `${active}:${activeLabel}:${tabs.length}`);
  return (
    <div
      data-tauri-drag-region
      role="tablist"
      aria-label="Side panel views"
      className="flex shrink-0 items-center border-separator border-b px-2"
      style={{ height } as CSSProperties}
    >
      <div ref={list} className="relative flex min-w-0 items-center gap-0.5">
        <span
          ref={mark}
          aria-hidden
          className="pointer-events-none absolute top-0 left-0 h-7 rounded-full bg-fill opacity-0"
        />
        {tabs.map((tab) => (
          <PanelTabButton
            key={tab.kind}
            tab={tab}
            active={tab.kind === active}
            onSelect={() => onSelect(tab.kind)}
          />
        ))}
      </div>
      <span className="flex-1 self-stretch" data-tauri-drag-region />
      {end}
    </div>
  );
}
