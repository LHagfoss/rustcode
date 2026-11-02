// A report command's answer (`/status`, `/stats`, `/context`…) in a compact
// panel over the composer, not in the transcript: its title, then label/value
// rows when the text has them (state/report.ts), else Markdown. Long output
// scrolls inside. Escape and a click outside close it, after any popover or
// menu over it, and never stop the running turn.

import { X } from "lucide-react";
import { useEffect, useMemo, useRef } from "react";
import { readReport } from "../state/report";
import { closeReport } from "../state/store";
import { CopyButton, Markdown } from "./Markdown";

/** A Radix popover, menu or dialog is above the panel and takes Escape or the click first. */
function somethingAbove(): boolean {
  return !!document.querySelector(
    '[data-radix-popper-content-wrapper]:not(:has([role="tooltip"])), [role="dialog"], [role="alertdialog"]',
  );
}

export function ReportPanel({ text }: { text: string }) {
  const view = useMemo(() => readReport(text), [text]);
  const panel = useRef<HTMLElement>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || somethingAbove()) return;
      e.preventDefault();
      e.stopPropagation();
      closeReport();
    };
    const onDown = (e: PointerEvent) => {
      const target = e.target as Node | null;
      if (!target || panel.current?.contains(target) || somethingAbove()) return;
      closeReport();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onDown, true);
    };
  }, []);
  return (
    <section
      ref={panel}
      aria-label={view.title ?? "Command result"}
      className="elevated flex max-h-[min(360px,45vh)] flex-col rounded-xl rc-in"
    >
      <header className="flex shrink-0 items-center gap-2 px-3.5 pt-2.5 pb-1.5">
        <span className="min-w-0 flex-1 truncate font-semibold text-body">{view.title ?? "Result"}</span>
        <CopyButton text={text} iconOnly label="Copy the result" />
        <button
          type="button"
          aria-label="Close"
          onClick={closeReport}
          className="flex size-[22px] items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg"
        >
          <X size={13} />
        </button>
      </header>
      <div className="selectable min-h-0 overflow-y-auto px-3.5 pb-3">
        {view.sections ? (
          <div className="flex flex-col gap-3">
            {view.sections.map((section, i) => (
              <div key={i} className="flex flex-col">
                {section.heading && (
                  <span className="mb-1 font-semibold text-caption text-fg-muted">{section.heading}</span>
                )}
                <dl className="grid grid-cols-[minmax(90px,max-content)_1fr] gap-x-4 gap-y-1 text-callout">
                  {section.rows.map((row, j) => (
                    <div key={j} className="contents">
                      <dt className="text-fg-muted">{row.label}</dt>
                      <dd className="m-0 min-w-0 break-words tabular">{row.value}</dd>
                    </div>
                  ))}
                </dl>
              </div>
            ))}
          </div>
        ) : (
          <Markdown text={view.body || text} className="gap-1.5 text-callout" />
        )}
      </div>
    </section>
  );
}
