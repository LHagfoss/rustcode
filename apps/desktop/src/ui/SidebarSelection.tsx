import { useLayoutEffect, useRef } from "react";
import { SIDEBAR_ROW_CURRENT, SIDEBAR_ROW_RADIUS } from "./sidebarRowStyle";

export function SidebarSelection() {
  const highlight = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const pill = highlight.current;
    const list = pill?.parentElement;
    if (!pill || !list) return;
    let previous: HTMLElement | null = null;
    const measure = () => {
      const row = list.querySelector<HTMLElement>("[data-sidebar-current]");
      const target = row?.closest('[data-fold="closed"]') ? null : row;
      pill.hidden = !target;
      if (!target) {
        previous = null;
        return;
      }
      if (target !== previous) pill.dataset.sliding = String(previous !== null);
      const bounds = target.getBoundingClientRect();
      const origin = list.getBoundingClientRect();
      pill.style.width = `${bounds.width}px`;
      pill.style.height = `${bounds.height}px`;
      pill.style.transform = `translate(${bounds.left - origin.left}px, ${bounds.top - origin.top}px)`;
      previous = target;
    };
    const resize = new ResizeObserver(measure);
    const observe = () => {
      resize.disconnect();
      resize.observe(list);
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
      attributeFilter: ["data-sidebar-current", "data-fold"],
    });
    observe();
    return () => {
      resize.disconnect();
      mutations.disconnect();
    };
  }, []);

  return (
    <span
      ref={highlight}
      hidden
      aria-hidden
      className={`pointer-events-none absolute top-0 left-0 ${SIDEBAR_ROW_RADIUS} ${SIDEBAR_ROW_CURRENT} motion-safe:data-[sliding=true]:transition-transform duration-200 ease-snappy`}
    />
  );
}
