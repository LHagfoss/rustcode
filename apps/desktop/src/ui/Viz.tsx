// `rustcode-viz` blocks (docs/visualizations.md) drawn natively in the
// transcript: bar_chart, table, tree (level 1) and line_chart, stat_list,
// checklist (level 2), with the documented limits. A payload that does not
// fit the schema shows its source with the reason, as the TUI does.

import { ChevronRight } from "lucide-react";
import { useMemo, useState } from "react";
import { CodeBlock, CopyButton } from "./Markdown";
import { cx } from "./primitives";

const MAX_BYTES = 16 * 1024;
const cut = (value: unknown, n: number) => {
  // Control characters and newlines become spaces, so a payload cannot break the layout.
  const s = [...String(value ?? "")]
    .map((ch) => (ch.charCodeAt(0) < 32 || ch.charCodeAt(0) === 127 ? " " : ch))
    .join("");
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
};
const num = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);

type Viz =
  | { type: "bar_chart"; items: { label: string; value: number }[]; unit?: string; more: number }
  | {
      type: "table";
      columns: string[];
      rows: (string | number | boolean | null)[][];
      sort?: { column: number; descending: boolean };
      moreCols: number;
    }
  | { type: "tree"; nodes: TreeNode[]; more: number }
  | {
      type: "line_chart";
      labels: string[];
      series: { name: string; values: (number | null)[] }[];
      unit?: string;
      more: number;
    }
  | {
      type: "stat_list";
      items: { label: string; value: string; unit?: string; note?: string }[];
      more: number;
    }
  | {
      type: "checklist";
      items: { label: string; status: Status; note?: string }[];
      more: number;
      done: number;
      total: number;
    };

type Status = "done" | "todo" | "in_progress" | "failed" | "skipped";
interface TreeNode {
  label: string;
  note?: string;
  children: TreeNode[];
}

export interface ParsedViz {
  title: string;
  summary?: string;
  viz: Viz;
}

/** The payload, or why it cannot be drawn. */
export function parseViz(source: string): ParsedViz | { problem: string } {
  if (new TextEncoder().encode(source).length > MAX_BYTES) return { problem: "over 16 KiB" };
  let raw: unknown;
  try {
    raw = JSON.parse(source);
  } catch {
    return { problem: "malformed JSON" };
  }
  if (!isObj(raw)) return { problem: "not a JSON object" };
  if (raw.version !== 1) return { problem: "version is not 1" };
  const type = raw.type;
  const title = cut(raw.title ?? String(type ?? "").replace("_", " "), 80);
  const summary = typeof raw.summary === "string" ? cut(raw.summary, 300) : undefined;
  const items = Array.isArray(raw.items) ? raw.items : null;
  const unit = typeof raw.unit === "string" ? cut(raw.unit, 12) : undefined;
  switch (type) {
    case "bar_chart": {
      if (!items) return { problem: "items is missing" };
      const out: { label: string; value: number }[] = [];
      for (const it of items) {
        if (!isObj(it) || !num(it.value)) return { problem: "a bar has no number value" };
        out.push({ label: cut(it.label, 40), value: it.value });
      }
      return {
        title,
        summary,
        viz: { type, items: out.slice(0, 30), unit, more: Math.max(0, out.length - 30) },
      };
    }
    case "table": {
      if (!Array.isArray(raw.columns) || !Array.isArray(raw.rows))
        return { problem: "columns or rows is missing" };
      const columns = raw.columns.map((c) => cut(c, 40));
      const rows: (string | number | boolean | null)[][] = [];
      for (const r of raw.rows) {
        if (!Array.isArray(r)) return { problem: "a row is not a list" };
        rows.push(
          r
            .slice(0, 8)
            .map((c) => (c === null || typeof c === "number" || typeof c === "boolean" ? c : cut(c, 60))),
        );
      }
      let sort: { column: number; descending: boolean } | undefined;
      if (isObj(raw.sort_by)) {
        const col = raw.sort_by.column;
        const shown = columns.slice(0, 8);
        const index =
          typeof col === "number"
            ? col
            : typeof col === "string" && /^\d+$/.test(col)
              ? Number(col)
              : shown.findIndex((c) => c.trim().toLowerCase() === String(col).trim().toLowerCase());
        if (index >= 0 && index < shown.length)
          sort = { column: index, descending: raw.sort_by.descending === true };
      }
      return {
        title,
        summary,
        viz: { type, columns: columns.slice(0, 8), rows, sort, moreCols: Math.max(0, columns.length - 8) },
      };
    }
    case "tree": {
      if (!Array.isArray(raw.nodes)) return { problem: "nodes is missing" };
      let count = 0;
      let more = 0;
      const walk = (list: unknown[], depth: number): TreeNode[] | null => {
        const out: TreeNode[] = [];
        for (const n of list) {
          if (!isObj(n)) return null;
          if (count >= 120 || depth >= 8) {
            more++;
            continue;
          }
          count++;
          const children = Array.isArray(n.children) ? walk(n.children, depth + 1) : [];
          if (!children) return null;
          out.push({ label: cut(n.label, 60), note: n.note ? cut(n.note, 60) : undefined, children });
        }
        return out;
      };
      const nodes = walk(raw.nodes, 0);
      if (!nodes) return { problem: "a node is not an object" };
      return { title, summary, viz: { type, nodes, more } };
    }
    case "line_chart": {
      if (!Array.isArray(raw.labels) || !Array.isArray(raw.series))
        return { problem: "labels or series is missing" };
      const labels = raw.labels.slice(0, 100).map((l) => cut(l, 40));
      const series = raw.series.slice(0, 4).map((s, i) => ({
        name: isObj(s) && typeof s.name === "string" ? cut(s.name, 40) : `series ${i + 1}`,
        values: (isObj(s) && Array.isArray(s.values) ? s.values : [])
          .slice(0, 100)
          .map((v) => (num(v) ? v : null)),
      }));
      if (!series.some((s) => s.values.some((v) => v !== null)))
        return { problem: "the line chart has no number" };
      const more = Math.max(0, raw.labels.length - 100) + Math.max(0, raw.series.length - 4);
      return { title, summary, viz: { type, labels, series, unit, more } };
    }
    case "stat_list": {
      if (!items) return { problem: "items is missing" };
      const out: { label: string; value: string; unit?: string; note?: string }[] = [];
      for (const it of items) {
        if (!isObj(it) || !(typeof it.value === "string" || num(it.value)))
          return { problem: "a stat has no value" };
        out.push({
          label: cut(it.label, 40),
          value: cut(it.value, 40),
          unit: typeof it.unit === "string" ? cut(it.unit, 12) : undefined,
          note: it.note ? cut(it.note, 60) : undefined,
        });
      }
      return { title, summary, viz: { type, items: out.slice(0, 12), more: Math.max(0, out.length - 12) } };
    }
    case "checklist": {
      if (!items) return { problem: "items is missing" };
      const out: { label: string; status: Status; note?: string }[] = [];
      for (const it of items) {
        if (!isObj(it)) return { problem: "an item is not an object" };
        const status = (it.status ?? "todo") as Status;
        if (!["done", "todo", "in_progress", "failed", "skipped"].includes(status)) {
          return { problem: `unknown status ${String(it.status)}` };
        }
        out.push({ label: cut(it.label, 60), status, note: it.note ? cut(it.note, 60) : undefined });
      }
      return {
        title,
        summary,
        viz: {
          type,
          items: out.slice(0, 50),
          more: Math.max(0, out.length - 50),
          done: out.filter((i) => i.status === "done").length,
          total: out.length,
        },
      };
    }
    default:
      return { problem: `unsupported type ${String(type)}` };
  }
}

const SERIES = ["var(--rc-brand)", "var(--rc-orange)", "var(--rc-green)", "var(--rc-blue)"];
const fmt = (v: number, unit?: string) =>
  `${Number.isInteger(v) ? v : v.toFixed(Math.abs(v) < 10 ? 2 : 1)}${unit ? ` ${unit}` : ""}`;
const More = ({ n, what }: { n: number; what: string }) =>
  n > 0 ? (
    <div className="mt-1 text-caption text-fg-3">
      … {n} more {what} not shown
    </div>
  ) : null;

function Bars({ viz }: { viz: Extract<Viz, { type: "bar_chart" }> }) {
  const max = Math.max(...viz.items.map((i) => Math.abs(i.value)), 1e-9);
  return (
    <div className="grid grid-cols-[minmax(60px,max-content)_1fr_auto] items-center gap-x-2.5 gap-y-1">
      {viz.items.map((it, i) => (
        <div key={i} className="contents" title={`${it.label}: ${fmt(it.value, viz.unit)}`}>
          <span className="truncate text-callout text-fg-2">{it.label}</span>
          <span className="h-[10px] overflow-hidden rounded-full bg-fill-3">
            <span
              className="block h-full rounded-full transition-[width] duration-300 ease-out"
              style={{
                width: `${(Math.abs(it.value) / max) * 100}%`,
                background: it.value < 0 ? "var(--rc-red)" : "var(--rc-brand)",
              }}
            />
          </span>
          <span className="text-right font-mono text-caption tabular">{fmt(it.value, viz.unit)}</span>
        </div>
      ))}
      <More n={viz.more} what="items" />
    </div>
  );
}

function Table({ viz }: { viz: Extract<Viz, { type: "table" }> }) {
  const [sort, setSort] = useState(viz.sort);
  const rows = useMemo(() => {
    if (!sort) return viz.rows;
    const out = [...viz.rows].sort((a, b) => {
      const x = a[sort.column];
      const y = b[sort.column];
      const c =
        typeof x === "number" && typeof y === "number"
          ? x - y
          : String(x ?? "").localeCompare(String(y ?? ""));
      return sort.descending ? -c : c;
    });
    return out;
  }, [viz.rows, sort]);
  const numeric = viz.columns.map((_, i) => viz.rows.every((r) => typeof r[i] === "number" || r[i] == null));
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-left text-callout">
        <thead>
          <tr>
            {viz.columns.map((c, i) => (
              <th
                key={i}
                className={cx("border-hairline border-b px-2 py-1 font-semibold", numeric[i] && "text-right")}
              >
                <button
                  type="button"
                  onClick={() =>
                    setSort(
                      sort?.column === i
                        ? { column: i, descending: !sort.descending }
                        : { column: i, descending: false },
                    )
                  }
                  className="hover:text-brand"
                >
                  {c}
                  {sort?.column === i ? (sort.descending ? " ↓" : " ↑") : ""}
                </button>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.slice(0, 50).map((r, i) => (
            <tr key={i} className="hover:bg-fill-4">
              {viz.columns.map((_, j) => (
                <td key={j} className={cx("px-2 py-1", numeric[j] && "text-right font-mono tabular")}>
                  {r[j] === null || r[j] === undefined ? "" : String(r[j])}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      <More n={Math.max(0, rows.length - 50)} what="rows" />
      <More n={viz.moreCols} what="columns" />
    </div>
  );
}

function TreeItem({ node, depth }: { node: TreeNode; depth: number }) {
  const [open, setOpen] = useState(depth < 2);
  return (
    <li>
      <button
        type="button"
        disabled={!node.children.length}
        onClick={() => setOpen(!open)}
        className="flex items-center gap-1 rounded px-1 text-left text-callout enabled:hover:bg-fill-3"
      >
        <ChevronRight
          size={11}
          className={cx(
            "text-fg-3 transition-transform",
            open && "rotate-90",
            !node.children.length && "invisible",
          )}
        />
        {node.label}
        {node.note && <span className="text-fg-2">{node.note}</span>}
      </button>
      {open && node.children.length > 0 && (
        <ul className="ml-[11px] border-hairline border-l pl-2">
          {node.children.map((c, i) => (
            <TreeItem key={i} node={c} depth={depth + 1} />
          ))}
        </ul>
      )}
    </li>
  );
}

function Lines({ viz }: { viz: Extract<Viz, { type: "line_chart" }> }) {
  const [hover, setHover] = useState<number | null>(null);
  const W = 600;
  const H = 180;
  const values = viz.series.flatMap((s) => s.values.filter((v): v is number => v !== null));
  const lo = Math.min(...values);
  const hi = Math.max(...values);
  const span = hi - lo || 1;
  const x = (i: number) => (viz.labels.length <= 1 ? W / 2 : (i / (viz.labels.length - 1)) * (W - 20) + 10);
  const y = (v: number) => H - 14 - ((v - lo) / span) * (H - 28);
  return (
    <div>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        className="h-[180px] w-full"
        role="img"
        aria-label="Line chart"
        onMouseLeave={() => setHover(null)}
        onMouseMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          const rel = ((e.clientX - r.left) / r.width) * W;
          setHover(
            Math.max(
              0,
              Math.min(viz.labels.length - 1, Math.round(((rel - 10) / (W - 20)) * (viz.labels.length - 1))),
            ),
          );
        }}
      >
        <line x1={0} x2={W} y1={H - 14} y2={H - 14} stroke="var(--rc-hairline)" />
        {viz.series.map((s, k) => {
          let d = "";
          s.values.forEach((v, i) => {
            if (v === null) return;
            d += `${d && s.values[i - 1] !== null ? "L" : "M"}${x(i).toFixed(1)},${y(v).toFixed(1)}`;
          });
          return <path key={k} d={d} fill="none" stroke={SERIES[k]} strokeWidth={2} strokeLinejoin="round" />;
        })}
        {hover !== null && (
          <line
            x1={x(hover)}
            x2={x(hover)}
            y1={4}
            y2={H - 14}
            stroke="var(--rc-fg-3)"
            strokeDasharray="3 3"
          />
        )}
      </svg>
      <div className="flex flex-wrap items-center gap-3 text-caption text-fg-2">
        {hover !== null ? (
          <>
            <b className="text-fg">{viz.labels[hover]}</b>
            {viz.series.map((s, k) => (
              <span key={k} style={{ color: SERIES[k] }}>
                {s.name}: {s.values[hover] == null ? "–" : fmt(s.values[hover]!, viz.unit)}
              </span>
            ))}
          </>
        ) : (
          <>
            <span>
              {viz.labels[0]} → {viz.labels.at(-1)}
            </span>
            {viz.series.length > 1 &&
              viz.series.map((s, k) => (
                <span key={k} className="flex items-center gap-1">
                  <span className="size-2 rounded-full" style={{ background: SERIES[k] }} />
                  {s.name}
                </span>
              ))}
          </>
        )}
      </div>
      <More n={viz.more} what="points/series" />
    </div>
  );
}

const CHECK: Record<Status, string> = { done: "✓", todo: "○", in_progress: "◐", failed: "✕", skipped: "–" };
const CHECK_TINT: Record<Status, string> = {
  done: "text-success",
  todo: "text-fg-3",
  in_progress: "text-info",
  failed: "text-danger",
  skipped: "text-fg-3",
};

export function VizView({ source, complete }: { source: string; complete: boolean }) {
  const parsed = useMemo(() => (complete ? parseViz(source) : null), [source, complete]);
  if (!complete) {
    return (
      <div className="rounded-xl bg-fill-4 px-3 py-2.5 text-callout text-fg-2 rc-shimmer">
        Building visualization…
      </div>
    );
  }
  if (!parsed || "problem" in parsed) {
    return (
      <div className="flex flex-col gap-1">
        <span className="text-caption text-fg-3">
          ◇ Couldn't draw this visualization ({parsed?.problem}), showing its source.
        </span>
        <CodeBlock code={source} lang="json" />
      </div>
    );
  }
  const { viz } = parsed;
  return (
    <figure
      className="group relative m-0 rounded-[14px] border border-hairline bg-fill-4 p-3 rc-in"
      aria-label={parsed.title}
    >
      <div className="absolute top-2 right-2 opacity-0 transition-opacity duration-150 group-hover:opacity-100">
        <CopyButton text={`\`\`\`rustcode-viz\n${source}\n\`\`\``} iconOnly label="Copy source" />
      </div>
      <figcaption className="mb-2 pr-8">
        <div className="font-semibold text-body">{parsed.title}</div>
        {parsed.summary && <div className="text-callout text-fg-2">{parsed.summary}</div>}
      </figcaption>
      {viz.type === "bar_chart" && <Bars viz={viz} />}
      {viz.type === "table" && <Table viz={viz} />}
      {viz.type === "tree" && (
        <>
          <ul className="flex flex-col">
            {viz.nodes.map((n, i) => (
              <TreeItem key={i} node={n} depth={0} />
            ))}
          </ul>
          <More n={viz.more} what="nodes" />
        </>
      )}
      {viz.type === "line_chart" && <Lines viz={viz} />}
      {viz.type === "stat_list" && (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(130px,1fr))] gap-2">
          {viz.items.map((it, i) => (
            <div key={i} className="rounded-xl bg-fill-3 px-3 py-2">
              <div className="truncate text-caption text-fg-2">{it.label}</div>
              <div className="font-semibold text-title3 tabular">
                {it.value}
                {it.unit && <span className="ml-1 font-normal text-callout text-fg-2">{it.unit}</span>}
              </div>
              {it.note && <div className="truncate text-caption text-fg-3">{it.note}</div>}
            </div>
          ))}
          <More n={viz.more} what="items" />
        </div>
      )}
      {viz.type === "checklist" && (
        <div>
          <div className="mb-2 flex items-center gap-2 text-caption text-fg-2">
            <span className="h-[6px] flex-1 overflow-hidden rounded-full bg-fill-3">
              <span
                className="block h-full rounded-full bg-success transition-[width] duration-300"
                style={{ width: `${(viz.done / Math.max(1, viz.total)) * 100}%` }}
              />
            </span>
            {viz.done} of {viz.total} done
          </div>
          <ul className="flex flex-col gap-0.5">
            {viz.items.map((it, i) => (
              <li key={i} className="flex items-baseline gap-2 text-callout">
                <span className={cx("w-3 shrink-0 text-center font-semibold", CHECK_TINT[it.status])}>
                  {CHECK[it.status]}
                </span>
                <span className={cx(it.status === "skipped" && "text-fg-3 line-through")}>{it.label}</span>
                {it.note && <span className="text-caption text-fg-2">{it.note}</span>}
              </li>
            ))}
          </ul>
          <More n={viz.more} what="items" />
        </div>
      )}
    </figure>
  );
}
