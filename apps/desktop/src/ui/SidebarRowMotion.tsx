import { Component, type RefObject } from "react";

type Props = {
  list: RefObject<HTMLElement | null>;
  threads: ReadonlyMap<string, boolean>;
  ready: boolean;
};

type Departure = { copy: HTMLElement; bounds: DOMRect };

function rowFor(list: HTMLElement, id: string) {
  return list.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(id)}"]`);
}

function play(row: HTMLElement, entering: boolean) {
  const faded = {
    opacity: 0,
    transform: matchMedia("(prefers-reduced-motion: reduce)").matches ? "none" : "translateY(6px)",
  };
  const visible = { opacity: 1, transform: "none" };
  return row.animate(entering ? [faded, visible] : [visible, faded], {
    duration: entering ? 180 : 125,
    easing: getComputedStyle(row).getPropertyValue("--ease-snappy").trim() || "ease-out",
  });
}

export class SidebarRowMotion extends Component<Props, Record<string, never>, Departure[]> {
  getSnapshotBeforeUpdate(previous: Props): Departure[] {
    const list = this.props.list.current;
    if (!list || !previous.ready || !this.props.ready || previous.threads === this.props.threads) return [];

    const departures: Departure[] = [];
    for (const [id, settled] of this.props.threads) {
      if (!settled || previous.threads.get(id) !== false) continue;
      const row = rowFor(list, id);
      if (!row) continue;
      const bounds = row.getBoundingClientRect();
      if (!bounds.width || !bounds.height) continue;
      const copy = row.cloneNode(true) as HTMLElement;
      copy.removeAttribute("data-row-id");
      copy.inert = true;
      copy.setAttribute("aria-hidden", "true");
      departures.push({ copy, bounds });
    }
    return departures;
  }

  componentDidUpdate(previous: Props, _state: Record<string, never>, departures: Departure[]) {
    const { list: listRef, threads, ready } = this.props;
    const list = listRef.current;
    if (!list || !ready || !previous.ready || previous.threads === threads) return;

    for (const [id, settled] of threads) {
      if (settled || previous.threads.has(id)) continue;
      const row = rowFor(list, id);
      if (row) play(row, true);
    }
    if (!departures.length) return;

    const viewport = list.getBoundingClientRect();
    for (const { copy, bounds } of departures) {
      if (bounds.bottom <= viewport.top || bounds.top >= viewport.bottom) continue;
      Object.assign(copy.style, {
        position: "absolute",
        left: `${bounds.left - viewport.left + list.scrollLeft}px`,
        top: `${bounds.top - viewport.top + list.scrollTop}px`,
        width: `${bounds.width}px`,
        height: `${bounds.height}px`,
        pointerEvents: "none",
      });
      list.append(copy);
      const animation = play(copy, false);
      animation.onfinish = animation.oncancel = () => copy.remove();
    }
  }

  render() {
    return null;
  }
}
