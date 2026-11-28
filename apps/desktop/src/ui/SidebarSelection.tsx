import { useLayoutEffect, useRef } from "react";
import { SIDEBAR_ROW_CURRENT, SIDEBAR_ROW_RADIUS } from "./sidebarRowStyle";

/**
 * The fill of the open row: one layer over the nav rows and the thread list that slides between
 * them, follows the list as it scrolls and is clipped to it.
 */
export function SidebarSelection({ hover = false }: { hover?: boolean }) {
  const highlight = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const pill = highlight.current;
    const list = pill?.parentElement;
    if (!pill || !list) return;
    let hovered: HTMLElement | null = null;
    let previous: HTMLElement | null = null;
    const measure = (scrolling = false) => {
      const row = hover
        ? hovered?.closest<HTMLElement>("[data-sidebar-hover]")
        : list.querySelector<HTMLElement>("[data-sidebar-current]");
      const target = row && list.contains(row) && !row.closest('[data-fold="closed"]') ? row : null;
      pill.hidden = !target;
      if (hover) {
        const current = "[data-sidebar-current]";
        pill.style.opacity = target?.matches(current) || target?.querySelector(current) ? "0" : "";
      }
      if (!target) {
        previous = null;
        return;
      }
      if (target !== previous) pill.dataset.sliding = String(previous !== null);
      else if (scrolling) pill.dataset.sliding = "false";
      const bounds = target.getBoundingClientRect();
      const origin = list.getBoundingClientRect();
      const viewport = target.closest("[data-sidebar-scroll]")?.getBoundingClientRect();
      const clipTop = viewport ? Math.max(0, viewport.top - bounds.top) : 0;
      const clipBottom = viewport ? Math.max(0, bounds.bottom - viewport.bottom) : 0;
      pill.style.clipPath = clipTop || clipBottom ? `inset(${clipTop}px 0 ${clipBottom}px 0)` : "";
      pill.style.width = `${bounds.width}px`;
      pill.style.height = `${bounds.height}px`;
      pill.style.transform = `translate(${bounds.left - origin.left}px, ${bounds.top - origin.top}px)`;
      previous = target;
    };
    const onPointer = (event: PointerEvent) => {
      if (event.pointerType === "touch") return;
      const next = event.type === "pointerout" ? event.relatedTarget : event.target;
      const row = next instanceof Element ? next.closest<HTMLElement>("[data-sidebar-hover]") : null;
      if (row === hovered) return;
      if (
        !row &&
        next instanceof Element &&
        next.closest("[data-sidebar-scroll], [data-sidebar-hover-group]") &&
        next.contains(hovered)
      )
        return;
      hovered = row && list.contains(row) ? row : null;
      measure();
    };
    const resize = new ResizeObserver(() => measure());
    const onScroll = () => measure(true);
    const observe = () => {
      resize.disconnect();
      resize.observe(list);
      for (const scroller of list.querySelectorAll("[data-sidebar-scroll], [data-sidebar-scroll] > *"))
        resize.observe(scroller);
      for (const fold of list.querySelectorAll("[data-fold]")) resize.observe(fold);
      measure();
    };
    const mutations = new MutationObserver((records) => {
      if (
        records.some((record) =>
          [...record.addedNodes, ...record.removedNodes].some(
            (node) =>
              node instanceof HTMLElement &&
              (node.matches("[data-fold]") || node.querySelector("[data-fold]")),
          ),
        )
      )
        observe();
      else measure();
    });
    mutations.observe(list, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["data-sidebar-current", "data-sidebar-hover", "data-fold"],
    });
    list.addEventListener("scroll", onScroll, { capture: true, passive: true });
    if (hover) {
      list.addEventListener("pointerover", onPointer);
      list.addEventListener("pointerout", onPointer);
    }
    observe();
    return () => {
      list.removeEventListener("scroll", onScroll, { capture: true });
      if (hover) {
        list.removeEventListener("pointerover", onPointer);
        list.removeEventListener("pointerout", onPointer);
      }
      resize.disconnect();
      mutations.disconnect();
    };
  }, [hover]);

  return (
    <span
      ref={highlight}
      hidden
      aria-hidden
      className={`pointer-events-none absolute top-0 left-0 ${SIDEBAR_ROW_RADIUS} ${hover ? "bg-fill glass-filter" : SIDEBAR_ROW_CURRENT} motion-safe:data-[sliding=true]:transition-transform duration-200 ease-snappy`}
    />
  );
}
