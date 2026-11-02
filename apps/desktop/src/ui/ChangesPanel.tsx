// The Changes panel, after ChangesPanel.swift: the thread's working tree
// against HEAD, from git in the Tauri shell. Reloads when a turn ends.

import { ChevronRight, CircleCheck, RefreshCw, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { type GitChanges, gitChanges } from "../lib/native";
import { useApp } from "../state/store";
import { Cluster, ClusterButton, cx, Spinner } from "./primitives";

const STATUS_TINT: Record<string, string> = {
  A: "text-success bg-success/15",
  U: "text-success bg-success/15",
  D: "text-danger bg-danger/15",
  R: "text-info bg-info/15",
  C: "text-info bg-info/15",
  M: "text-warning bg-warning/15",
};

/** Splits `git diff` output into per-file chunks keyed by the new path. */
export function splitUnifiedDiff(diff: string): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  let current: string[] | null = null;
  for (const line of diff.split("\n")) {
    const header = /^diff --git a\/(.+?) b\/(.+)$/.exec(line);
    if (header) {
      current = [];
      out[header[2]!] = current;
      continue;
    }
    if (!current || line.startsWith("index ") || line.startsWith("--- ") || line.startsWith("+++ ")) continue;
    current.push(line);
  }
  return out;
}

function UnifiedDiff({ lines }: { lines: string[] }) {
  return (
    <div className="selectable overflow-x-auto rounded-lg bg-fill-3 py-1 font-mono text-caption leading-[16px]">
      <div className="min-w-fit">
        {lines.map((line, i) => (
          <div
            key={i}
            className={cx(
              "px-2.5 whitespace-pre",
              line.startsWith("+") && "bg-[var(--rc-diff-add)]",
              line.startsWith("-") && "bg-[var(--rc-diff-del)]",
              line.startsWith("@@") && "text-fg-3",
            )}
          >
            {line || " "}
          </div>
        ))}
      </div>
    </div>
  );
}

export function ChangesPanel({ onClose }: { onClose: () => void }) {
  const cwd = useApp((s) => (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd);
  const running = useApp((s) => (s.selectedId ? s.threads[s.selectedId]?.running : false) ?? false);
  const [data, setData] = useState<GitChanges | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [loadedAt, setLoadedAt] = useState<number>();

  const load = useCallback(async () => {
    if (!cwd) return;
    setLoading(true);
    try {
      const result = await gitChanges(cwd);
      if (!result) setError("Changes need the desktop app: git runs in its native shell.");
      else {
        setData(result);
        setError(null);
        setLoadedAt(Date.now());
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [cwd]);

  // Reload on open, on a folder change, and when a turn ends.
  // Runs again when `running` flips, so a finished turn reloads.
  useEffect(() => {
    if (!running) void load();
  }, [load, running]);

  const chunks = useMemo(() => splitUnifiedDiff(data?.diff ?? ""), [data]);
  const totals = useMemo(
    () =>
      (data?.files ?? []).reduce((t, f) => ({ added: t.added + f.added, removed: t.removed + f.removed }), {
        added: 0,
        removed: 0,
      }),
    [data],
  );

  return (
    <aside
      aria-label="Changes"
      className="flex w-[480px] shrink-0 flex-col border-hairline border-l bg-chat rc-in"
    >
      <div
        data-tauri-drag-region
        className="flex h-[42px] items-center gap-2 border-hairline border-b px-3 pt-1.5 pb-2"
      >
        <span className="font-semibold text-body">Uncommitted</span>
        <span className="flex-1" />
        <Cluster>
          <ClusterButton label="Reload" onClick={() => void load()}>
            {loading ? <Spinner size={12} /> : <RefreshCw size={13} />}
          </ClusterButton>
          <ClusterButton label="Close changes" onClick={onClose}>
            <X size={14} />
          </ClusterButton>
        </Cluster>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-3">
        {error && <p className="text-callout text-fg-2">{error}</p>}
        {!error && data && (
          <>
            <div className="mb-0.5 flex items-baseline gap-2 text-callout tabular">
              <span className="font-semibold">
                {data.files.length} file{data.files.length === 1 ? "" : "s"}
              </span>
              <span className="text-success">+{totals.added}</span>
              <span className="text-danger">−{totals.removed}</span>
              <span className="flex-1" />
              {loadedAt && (
                <span className="text-fg-3">
                  {new Date(loadedAt).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}
                </span>
              )}
            </div>
            <p className="mb-3 text-caption text-fg-2">
              Uncommitted and untracked changes{data.branch ? ` on ${data.branch}` : ""}.
            </p>
            {!data.files.length && (
              <div className="flex flex-col items-center gap-2 py-10 text-callout text-fg-2">
                <CircleCheck size={20} />
                No uncommitted changes
              </div>
            )}
            <ul className="flex flex-col gap-0.5">
              {data.files.map((f) => (
                <li key={f.path}>
                  <button
                    type="button"
                    aria-expanded={!!open[f.path]}
                    onClick={() => setOpen({ ...open, [f.path]: !open[f.path] })}
                    className="flex w-full items-center gap-2 rounded-md p-[5px] text-left hover:bg-fill-3"
                  >
                    <ChevronRight
                      size={11}
                      className={cx("shrink-0 text-fg-3 transition-transform", open[f.path] && "rotate-90")}
                    />
                    <span
                      className={cx(
                        "flex size-4 shrink-0 items-center justify-center rounded font-bold font-mono text-[9px]",
                        STATUS_TINT[f.status] ?? "bg-fill-2 text-fg-2",
                      )}
                    >
                      {f.status}
                    </span>
                    <span className="min-w-0 flex-1 truncate font-mono text-callout" dir="rtl">
                      <bdi>{f.path}</bdi>
                    </span>
                    {f.binary ? (
                      <span className="text-caption text-fg-3">binary</span>
                    ) : (
                      <span className="font-mono text-caption tabular">
                        <span className="text-success">+{f.added}</span>{" "}
                        <span className="text-danger">−{f.removed}</span>
                      </span>
                    )}
                  </button>
                  {open[f.path] && chunks[f.path] && (
                    <div className="mt-1 mb-2 ml-6">
                      <UnifiedDiff lines={chunks[f.path]!} />
                    </div>
                  )}
                </li>
              ))}
            </ul>
          </>
        )}
        {!error && !data && loading && (
          <div className="flex items-center gap-2 text-callout text-fg-2">
            <Spinner /> Reading changes…
          </div>
        )}
      </div>
    </aside>
  );
}
