
import { memo } from "react";
import { type UpdateRun, updateRunLabel } from "../state/updateRuns";
import { ChevronRight, Layers } from "./icons";
import { cx } from "./primitives";

export const UpdatesRow = memo(function UpdatesRow({
  run,
  onToggle,
}: {
  run: UpdateRun;
  onToggle: (key: string) => void;
}) {
  const { label, detail } = updateRunLabel(run);
  return (
    <div className="flex items-center gap-3 py-1 text-caption text-fg-muted">
      <span className="h-px flex-1 bg-border" />
      <button
        type="button"
        aria-expanded={run.open}
        onClick={() => onToggle(run.key)}
        className="group flex min-w-0 items-center gap-1.5 rounded-sm px-1 hover:text-fg"
      >
        <Layers size={12} className="shrink-0 text-fg-faint" aria-hidden />
        <span className="shrink-0">{label}</span>
        {detail && (
          <>
            <span aria-hidden className="text-fg-faint">
              ·
            </span>
            <span className="min-w-0 truncate tabular">{detail}</span>
          </>
        )}
        <ChevronRight
          size={12}
          aria-hidden
          className={cx("shrink-0 text-fg-faint transition-transform duration-200", run.open && "rotate-90")}
        />
      </button>
      <span className="h-px flex-1 bg-border" />
    </div>
  );
});
