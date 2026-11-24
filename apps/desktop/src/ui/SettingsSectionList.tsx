import { type ComponentType, useLayoutEffect, useRef, useState } from "react";
import type { SettingsSection } from "../state/appState";
import { cx } from "./primitives";

export interface SettingsNavPage {
  id: SettingsSection;
  label: string;
  icon: ComponentType<{ size?: number; strokeWidth?: number; className?: string }>;
}

export interface SettingsNavGroup {
  heading?: string;
  pages: SettingsNavPage[];
}

interface Highlight {
  top: number;
  height: number;
  ready: boolean;
}

/**
 * The Settings pages, grouped under their headings. The open page's fill is one shared pill that
 * glides to the page picked next (200ms `--ease-snappy`, transform only), so the eye follows the
 * move instead of seeing one row go dark and another light up.
 */
export function SettingsSectionList({
  groups,
  section,
  attention,
  onOpen,
}: {
  groups: SettingsNavGroup[];
  section: SettingsSection;
  attention: Partial<Record<SettingsSection, string>>;
  onOpen: (id: SettingsSection) => void;
}) {
  const list = useRef<HTMLDivElement>(null);
  const [highlight, setHighlight] = useState<Highlight | null>(null);
  useLayoutEffect(() => {
    const row = list.current?.querySelector<HTMLElement>(`[data-section="${section}"]`);
    if (!row) return setHighlight(null);
    setHighlight((prev) => ({ top: row.offsetTop, height: row.offsetHeight, ready: prev !== null }));
  }, [section]);
  return (
    <div ref={list} className="relative flex flex-col gap-4">
      {highlight && (
        <span
          aria-hidden
          data-testid="settings-nav-highlight"
          className={cx(
            "pointer-events-none absolute inset-x-0 top-0 rounded-md bg-fill",
            highlight.ready && "transition-transform duration-200 ease-snappy",
          )}
          style={{ height: highlight.height, transform: `translateY(${highlight.top}px)` }}
        />
      )}
      {groups.map((group) => (
        <div key={group.heading ?? group.pages[0]?.id} className="flex flex-col gap-1">
          {group.heading && (
            <div className="px-2 pb-0.5 font-medium text-callout text-fg-muted">{group.heading}</div>
          )}
          {group.pages.map((s) => {
            const current = s.id === section;
            return (
              <button
                key={s.id}
                type="button"
                data-section={s.id}
                aria-current={current ? "page" : undefined}
                aria-description={attention[s.id]}
                onClick={() => onOpen(s.id)}
                className={cx(
                  "relative flex h-8 items-center gap-2 rounded-md px-2 text-left font-medium text-body",
                  current ? "text-fg" : "text-fg-muted hover:bg-fill hover:text-fg",
                  current && !highlight && "bg-fill",
                )}
              >
                <s.icon size={16} strokeWidth={1.85} className="shrink-0" />
                <span className="min-w-0 flex-1 truncate">{s.label}</span>
                {attention[s.id] && (
                  <span
                    data-testid={`settings-attention-${s.id}`}
                    title={attention[s.id]}
                    className="size-1.5 shrink-0 rounded-full bg-warning"
                  />
                )}
              </button>
            );
          })}
        </div>
      ))}
    </div>
  );
}
