import { formatBinding } from "../lib/keys";
// Settings › Chat › Thread header buttons: which of the thread header's buttons
// show, and in what order. Two lists — Shown and Hidden — and a row moves
// between them by drag or by its own Show/Hide button, so it works without a
// pointer too; ⌥↑/↓ moves a focused Shown row. The header reads the same store
// field, so it changes as a row moves.
//
// Drag is pointer events, captured on the grip and ended on pointerup,
// pointercancel or a lost capture: the Tauri webview takes HTML5 drops itself
// (lib/native.ts), and this list sits inside Settings. Where a row lands is
// `insertIndex` in state/headerButtons.ts, which the tests cover; the Shown
// list keeps its order in the store, the Hidden one is the rest in registry
// order, so dragging across is what matters there.

import { Eye, EyeOff, GripVertical, type LucideIcon } from "lucide-react";
import { type KeyboardEvent, type PointerEvent, useEffect, useRef, useState } from "react";
import {
  DEFAULT_HEADER_BUTTONS,
  HEADER_BUTTON_IDS,
  type HeaderButtonId,
  insertIndex,
  sameHeaderButtons,
} from "../state/headerButtons";
import { setHeaderButtons, useApp } from "../state/store";
import { HEADER_BUTTONS } from "./ChatView";
import { cx, Pill } from "./primitives";
import { Row } from "./settingsKit";

type List = "shown" | "hidden";

/** The ids a list shows, in order: the store's for Shown, the registry's for Hidden. */
function idsOf(list: List, shown: readonly HeaderButtonId[]): HeaderButtonId[] {
  return list === "shown" ? [...shown] : HEADER_BUTTON_IDS.filter((id) => !shown.includes(id));
}

interface Drag {
  id: HeaderButtonId;
  from: List;
  x: number;
  y: number;
  left: number;
  top: number;
  width: number;
}

/** Where the pointer would drop the dragged row, and the line to draw for it. */
interface Drop {
  list: List;
  index: number;
}

/** Return the saved order after a drop without mutating the current order. */
export function reorderHeaderButtons(shown: readonly HeaderButtonId[], id: HeaderButtonId, target: Drop) {
  const next = shown.filter((item) => item !== id);
  if (target.list === "shown") next.splice(Math.min(Math.max(target.index, 0), next.length), 0, id);
  return next;
}

export function HeaderButtonsSetting() {
  const shown = useApp((s) => s.headerButtons);
  const shownList = useRef<HTMLDivElement>(null);
  const hiddenList = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [drop, setDrop] = useState<Drop | null>(null);
  const preview = useRef<HTMLDivElement>(null);
  const landing = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const key = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        if (timer.current) clearTimeout(timer.current);
        landing.current = false;
        setDrag(null);
        setDrop(null);
      }
    };
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("keydown", key);
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  /** Rows of the Shown list as boxes in the window, without the dragged one. */
  const shownRows = (except: HeaderButtonId) => {
    const list = shownList.current;
    if (!list) return [];
    return [...list.querySelectorAll<HTMLElement>("[data-header-button]")]
      .filter((el) => el.dataset.headerButton !== except)
      .map((el) => {
        const r = el.getBoundingClientRect();
        return { top: r.top, height: r.height };
      });
  };

  /** The pointer is in a list: between its sides, and within a short reach above and below. */
  const inside = (el: HTMLElement | null, x: number, y: number) => {
    if (!el) return false;
    const r = el.getBoundingClientRect();
    return x >= r.left && x <= r.right && y >= r.top - 20 && y <= r.bottom + 20;
  };

  /** Moves `id` to `index` of the Shown list (hiding it when the target is Hidden). */
  const place = (id: HeaderButtonId, target: Drop) => {
    setHeaderButtons(reorderHeaderButtons(shown, id, target));
  };

  const onPointerMove = (e: PointerEvent) => {
    if (!drag || landing.current) return;
    setDrag({ ...drag, x: e.clientX, y: e.clientY });
    // A drag that started in Hidden has nowhere to go but the Shown list: it is
    // already hidden, and the Hidden list has no order of its own.
    if (drag.from === "hidden" && inside(hiddenList.current, e.clientX, e.clientY)) {
      setDrop(null);
      return;
    }
    if (drag.from === "shown" && inside(hiddenList.current, e.clientX, e.clientY)) {
      setDrop({ list: "hidden", index: 0 });
      return;
    }
    if (!inside(shownList.current, e.clientX, e.clientY)) {
      setDrop(null);
      return;
    }
    setDrop({ list: "shown", index: insertIndex(shownRows(drag.id), e.clientY) });
  };

  const endDrag = () => {
    setDrag(null);
    setDrop(null);
  };

  const onGripDown = (e: PointerEvent<HTMLElement>, id: HeaderButtonId, from: List) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    const row = e.currentTarget.closest<HTMLElement>("[data-header-button]");
    if (!row) return;
    const box = row.getBoundingClientRect();
    setDrag({
      id,
      from,
      x: e.clientX,
      y: e.clientY,
      left: box.left - e.clientX,
      top: box.top - e.clientY,
      width: box.width,
    });
    setDrop(null);
  };

  const onGripUp = () => {
    if (!drag) return;
    if (!drop) {
      endDrag();
      return;
    }
    landing.current = true;
    const target = document.querySelector<HTMLElement>("[data-header-drop-slot='active']");
    const ghost = preview.current;
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const finish = () => {
      place(drag.id, drop);
      landing.current = false;
      endDrag();
    };
    if (target && ghost && !reduced) {
      const from = ghost.getBoundingClientRect();
      const to = target.getBoundingClientRect();
      ghost.animate(
        [
          { transform: "scale(1.03)" },
          { transform: `translate(${to.left - from.left}px, ${to.top - from.top}px) scale(1)` },
        ],
        { duration: 160, easing: "ease-out", fill: "forwards" },
      );
      timer.current = setTimeout(finish, 160);
    } else finish();
  };

  const onKeyDown = (e: KeyboardEvent, id: HeaderButtonId, list: List) => {
    if (!e.altKey) return;
    const delta = e.key === "ArrowUp" ? -1 : e.key === "ArrowDown" ? 1 : 0;
    if (!delta || list !== "shown") return;
    e.preventDefault();
    const from = shown.indexOf(id);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= shown.length) return;
    const next = [...shown];
    next.splice(from, 1);
    next.splice(to, 0, id);
    setHeaderButtons(next);
  };

  const column = (which: List) => {
    const ids = idsOf(which, shown);
    const others = ids.filter((id) => id !== drag?.id);
    const hiddenIndex = drag
      ? others.filter((id) => HEADER_BUTTON_IDS.indexOf(id) < HEADER_BUTTON_IDS.indexOf(drag.id)).length
      : 0;
    const slotIndex = drop?.list === which ? (which === "shown" ? drop.index : hiddenIndex) : -1;
    const gap = (index: number) => (
      <div
        key={`slot-${index}`}
        data-header-drop-slot={slotIndex === index ? "active" : undefined}
        aria-hidden
        className={cx(
          "overflow-hidden motion-safe:transition-[height] motion-safe:duration-150",
          slotIndex === index ? "h-9" : "h-0",
        )}
      >
        <div className="h-full rounded-md bg-accent/10 ring-1 ring-inset ring-accent/50" />
      </div>
    );
    const body = (
      <div
        ref={which === "shown" ? shownList : hiddenList}
        role="list"
        aria-label={which === "shown" ? "Buttons shown in the header" : "Buttons hidden from the header"}
        className={cx(
          "relative flex flex-col rounded-lg p-1",
          which === "hidden" && drop?.list === "hidden" ? "bg-accent/10" : "bg-fill-subtle/50",
        )}
      >
        {ids.map((id, index) => (
          <div key={id}>
            {id !== drag?.id && gap(ids.slice(0, index).filter((item) => item !== drag?.id).length)}
            <HeaderButtonRow
              id={id}
              list={which}
              dragging={drag?.id === id}
              onGripDown={onGripDown}
              onGripUp={onGripUp}
              onGripMove={onPointerMove}
              onGripCancel={() => {
                if (!landing.current) endDrag();
              }}
              onKeyDown={onKeyDown}
              onToggle={() =>
                which === "shown"
                  ? setHeaderButtons(shown.filter((x) => x !== id))
                  : place(id, { list: "shown", index: shown.length })
              }
            />
          </div>
        ))}
        {gap(others.length)}
        {ids.length === 0 && (
          <p className="px-2 py-2 text-callout text-fg-faint">
            {which === "shown"
              ? "Every button is hidden: the header shows only its title."
              : "Every button is in the header."}
          </p>
        )}
      </div>
    );
    return (
      <div className="flex min-w-0 flex-col gap-1.5">
        <h3 className="px-1 font-medium text-caption text-fg-faint">
          {which === "shown" ? "Shown" : "Hidden"}
          <span className="px-1 text-fg-faint/70">·</span>
          <span className="tabular">{ids.length}</span>
        </h3>
        {body}
      </div>
    );
  };

  return (
    <>
      <Row
        title="Thread header buttons"
        reset={
          sameHeaderButtons(shown, DEFAULT_HEADER_BUTTONS)
            ? undefined
            : () => setHeaderButtons([...DEFAULT_HEADER_BUTTONS])
        }
        subtitle="Which buttons the thread header shows, and in what order. A button whose thread has nothing to show (no plan, no pull requests) still stays out of the way."
        status={`Drag a row to move it, or press ${formatBinding("alt+up")} or ${formatBinding("alt+down")} on a focused row. Show and Hide work too.`}
      />
      <div className="@container px-4 pt-3 pb-4">
        <div className="grid grid-cols-1 gap-4 @min-[34rem]:grid-cols-2">
          {column("shown")}
          {column("hidden")}
        </div>
      </div>
      {drag &&
        (() => {
          const entry = HEADER_BUTTONS[drag.id];
          const Icon = entry.icon;
          return (
            <div
              ref={preview}
              aria-hidden
              className="pointer-events-none fixed z-50 flex h-9 items-center gap-2 rounded-md bg-surface-2 px-1.5 text-body shadow-lg motion-safe:scale-[1.03]"
              style={{ left: drag.x + drag.left, top: drag.y + drag.top, width: drag.width }}
            >
              <GripVertical size={13} className="mx-1 shrink-0 text-fg-faint" />
              <Icon size={14} className="shrink-0 text-fg-muted" />
              <span className="min-w-0 flex-1 truncate">{entry.label}</span>
              <span className="px-2 text-caption text-fg-muted">
                {drag.from === "shown" ? "Hide" : "Show"}
              </span>
            </div>
          );
        })()}
    </>
  );
}

function HeaderButtonRow({
  id,
  list,
  dragging,
  onGripDown,
  onGripMove,
  onGripUp,
  onGripCancel,
  onKeyDown,
  onToggle,
}: {
  id: HeaderButtonId;
  list: List;
  dragging: boolean;
  onGripDown: (e: PointerEvent<HTMLElement>, id: HeaderButtonId, from: List) => void;
  onGripMove: (e: PointerEvent) => void;
  onGripUp: (e: PointerEvent<HTMLElement>) => void;
  onGripCancel: () => void;
  onKeyDown: (e: KeyboardEvent, id: HeaderButtonId, list: List) => void;
  onToggle: () => void;
}) {
  const entry = HEADER_BUTTONS[id];
  const Icon: LucideIcon = entry.icon;
  return (
    <div
      role="listitem"
      data-header-button={id}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: the row takes ⌥↑/⌥↓ to reorder, so it has to hold focus
      tabIndex={0}
      onKeyDown={(e) => onKeyDown(e, id, list)}
      className={cx(
        "flex h-9 items-center gap-2 rounded-md px-1.5 text-body hover:bg-fill",
        dragging && "bg-fill-strong opacity-30",
      )}
    >
      <span
        aria-hidden
        onPointerDown={(e) => onGripDown(e, id, list)}
        onPointerMove={onGripMove}
        onPointerUp={onGripUp}
        onPointerCancel={onGripCancel}
        onLostPointerCapture={onGripCancel}
        className="flex size-5 shrink-0 cursor-grab touch-none items-center justify-center text-fg-faint hover:text-fg-muted active:cursor-grabbing"
      >
        <GripVertical size={13} />
      </span>
      <Icon size={14} className="shrink-0 text-fg-muted" />
      <span className="min-w-0 flex-1 truncate">{entry.label}</span>
      <Pill
        size="sm"
        quiet
        aria-label={`${list === "shown" ? "Hide" : "Show"} ${entry.label}`}
        onClick={onToggle}
      >
        {list === "shown" ? <EyeOff size={12} /> : <Eye size={12} />}
        {list === "shown" ? "Hide" : "Show"}
      </Pill>
    </div>
  );
}
