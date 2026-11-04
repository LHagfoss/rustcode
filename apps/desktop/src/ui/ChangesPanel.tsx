// The Changes panel, after ChangesPanel.swift: the thread's working tree
// against HEAD, from git in the Tauri shell. Each file keeps its header in
// place while its diff scrolls under it, and can be folded away; the panel
// draws the diff unified, or side by side when the window is wide enough.
// Reloads when a turn ends.

import { ChevronRight, CircleCheck, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { setDiffMode, useDiffLayout, useDiffWidth } from "../lib/diffMode";
import { type GitChanges, gitChanges } from "../lib/native";
import { parsePatch } from "../state/diff";
import { useApp } from "../state/store";
import { Patch } from "./DiffView";
import { Cluster, ClusterButton, cx, Segments, Spinner, Tooltip } from "./primitives";
import { DiffCount } from "./Rows";

const STATUS_TINT: Record<string, string> = {
  A: "text-success bg-success/15",
  U: "text-success bg-success/15",
  D: "text-danger bg-danger/15",
  R: "text-accent bg-accent/15",
  C: "text-accent bg-accent/15",
  M: "text-warning bg-warning/15",
};

/** The letter a file's change is marked with, in the colour it means. */
export function StatusMark({ status, size = 4 }: { status: string; size?: 4 | 5 }) {
  return (
    <span
      aria-hidden
      className={cx(
        "flex shrink-0 items-center justify-center rounded-xs font-bold font-mono text-[9px]",
        size === 5 ? "size-5" : "size-4",
        STATUS_TINT[status] ?? "bg-fill text-fg-muted",
      )}
    >
      {status.slice(0, 1)}
    </span>
  );
}

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

/** One file's section: its header stays put, its diff folds away under it. */
function FileSection({
  file,
  chunk,
  mode,
}: {
  file: GitChanges["files"][number];
  chunk: string[] | undefined;
  mode: "unified" | "split";
}) {
  const [collapsed, setCollapsed] = useState(false);
  const lines = useMemo(() => parsePatch(chunk ?? []), [chunk]);
  return (
    <section className="mb-3">
      <button
        type="button"
        aria-expanded={!collapsed}
        onClick={() => setCollapsed(!collapsed)}
        // Full-bleed while it holds its place: nothing of the file above shows beside it.
        className="sticky top-0 z-10 -mx-3 flex w-[calc(100%+1.5rem)] items-center gap-2 bg-surface-2 px-4 py-1.5 text-left hover:bg-fill-subtle"
      >
        <ChevronRight
          size={11}
          strokeWidth={2.5}
          className={cx(
            "shrink-0 text-fg-faint transition-transform duration-150",
            !collapsed && "rotate-90",
          )}
        />
        <StatusMark status={file.status} />
        <span className="min-w-0 flex-1 truncate font-mono text-callout" dir="rtl">
          <bdi>{file.path}</bdi>
        </span>
        {file.binary ? (
          <span className="text-caption text-fg-faint">binary</span>
        ) : (
          <DiffCount added={file.added} removed={file.removed} />
        )}
      </button>
      {!collapsed &&
        (lines.length ? (
          <div className="mb-2 ml-2">
            <Patch lines={lines} path={file.path} mode={mode} />
          </div>
        ) : (
          <p className="mb-2 ml-2 px-2 py-1 text-caption text-fg-faint">
            {file.binary ? "A binary file: no lines to show." : "No text diff to show."}
          </p>
        ))}
    </section>
  );
}

export function ChangesPanel() {
  const cwd = useApp((s) => (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd);
  const running = useApp((s) => (s.selectedId ? s.threads[s.selectedId]?.running : false) ?? false);
  const [data, setData] = useState<GitChanges | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadedAt, setLoadedAt] = useState<number>();
  // Split is measured on this pane's own column, not the window: a wide window
  // with a narrow panel still has no room for two columns.
  const [bodyRef, bodyWidth] = useDiffWidth<HTMLDivElement>();
  const { mode, picked, wide } = useDiffLayout(bodyWidth);

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
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 px-3">
        <span className="font-semibold text-body">Uncommitted</span>
        <span className="flex-1" />
        <Tooltip label={wide ? "Unified or side by side" : "Side by side needs a wider view"}>
          <span>
            <Segments
              label="Layout"
              value={mode}
              onChange={(v) => setDiffMode(v)}
              options={[
                { value: "unified", label: "Unified" },
                { value: "split", label: "Split", disabled: !wide },
              ]}
            />
          </span>
        </Tooltip>
        <Cluster>
          <ClusterButton label="Reload" onClick={() => void load()}>
            {loading ? <Spinner size={12} /> : <RefreshCw size={13} />}
          </ClusterButton>
        </Cluster>
      </div>
      <div ref={bodyRef} className="min-h-0 flex-1 overflow-y-auto px-3 py-3">
        {error && <p className="text-callout text-fg-muted">{error}</p>}
        {!wide && picked === "split" && (
          <p className="mb-3 text-caption text-fg-faint">
            Side by side needs 900px of width here; these files read unified until then.
          </p>
        )}
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
                <span className="text-fg-faint">
                  {new Date(loadedAt).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}
                </span>
              )}
            </div>
            <p className="mb-3 text-caption text-fg-muted">
              Uncommitted and untracked changes{data.branch ? ` on ${data.branch}` : ""}.
            </p>
            {!data.files.length && (
              <div className="flex flex-col items-center gap-2 py-10 text-callout text-fg-muted">
                <CircleCheck size={20} />
                No uncommitted changes
              </div>
            )}
            {data.files.map((file) => (
              <FileSection key={file.path} file={file} chunk={chunks[file.path]} mode={mode} />
            ))}
          </>
        )}
        {!error && !data && loading && (
          <div className="flex items-center gap-2 text-callout text-fg-muted">
            <Spinner /> Reading changes…
          </div>
        )}
      </div>
    </div>
  );
}
