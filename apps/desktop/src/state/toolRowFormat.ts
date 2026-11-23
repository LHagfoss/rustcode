import type { Json } from "../acp/jsonrpc";
import { displayCommand, stripLeadingCd } from "../lib/command";
import { basename, isAbsolute, isInside, join, normalize, shortenHome } from "../lib/paths";
import { commandOf } from "../lib/toolBody";
import type { TranscriptItem } from "./thread";
import { displayTool, usefulSummary } from "./toolNames";

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

/** Visual pieces of a single tool row, normalized across harnesses. */
export interface FormattedToolRow {
  title: string;
  titleMono?: boolean;
  pathDir?: string;
  pathFile?: string;
  lineRange?: string;
  summary?: string;
  detail?: string;
  detailMono: boolean;
}

const FILE_TITLE = /^(File|Read|Edit|Write|View|Create|Delete)\s+(.+)$/;

function isRecord(input: Json | undefined): input is Record<string, Json> {
  return !!input && typeof input === "object" && !Array.isArray(input);
}

function inputField(input: Json | undefined, ...keys: string[]): string | undefined {
  if (!isRecord(input)) return undefined;
  for (const key of keys) {
    const value = input[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return undefined;
}

function inputNumber(input: Json | undefined, key: string): number | undefined {
  if (!isRecord(input)) return undefined;
  const value = input[key];
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

/** Resolves relative `../` paths against `cwd`, drops the `cwd/` prefix, or shortens home to `~`. */
export function cleanToolPath(rawPath: string, cwd: string): string {
  const trimmed = rawPath.trim();
  if (!trimmed) return "";
  const resolved =
    !isAbsolute(trimmed) && cwd ? join(cwd, trimmed) : isAbsolute(trimmed) ? normalize(trimmed) : trimmed;
  if (cwd && isAbsolute(resolved) && isInside(resolved, cwd)) {
    const root = normalize(cwd).replace(/[\\/]+$/, "");
    const rel = resolved.slice(root.length).replace(/^[\\/]+/, "");
    return rel || basename(resolved);
  }
  return isAbsolute(resolved) ? shortenHome(resolved) : resolved;
}

function splitPathDisplay(display: string): { pathDir?: string; pathFile: string } {
  const slash = Math.max(display.lastIndexOf("/"), display.lastIndexOf("\\"));
  if (slash < 0) return { pathFile: display };
  return {
    pathDir: display.slice(0, slash + 1),
    pathFile: display.slice(slash + 1),
  };
}

function fileLineRange(input: Json | undefined): string | undefined {
  const start = inputNumber(input, "StartLine");
  const end = inputNumber(input, "EndLine");
  if (start !== undefined && end !== undefined) return `L${start}–${end}`;
  if (start !== undefined) return `L${start}+`;
  const offset = inputNumber(input, "offset");
  const limit = inputNumber(input, "limit");
  if (offset !== undefined && limit !== undefined && limit > 0) return `L${offset}–${offset + limit - 1}`;
  if (offset !== undefined) return `L${offset}+`;
  return undefined;
}

function fileVerb(kind: string, titleVerb: string | undefined, item: ToolItem): string {
  if (kind === "read") return titleVerb === "View" ? "View" : "Read";
  if (kind === "edit") {
    if (titleVerb === "Write" || titleVerb === "Create") return "Write";
    if (
      isRecord(item.input) &&
      typeof item.input.Content === "string" &&
      item.input.Target === undefined &&
      item.diffs.every((d) => d.oldText === undefined)
    ) {
      return "Write";
    }
    return "Edit";
  }
  if (kind === "delete") return "Delete";
  if (kind === "move") return "Move";
  return titleVerb && titleVerb !== "File" ? titleVerb : "Read";
}

function fallbackDetail(item: ToolItem, kind: string): { text?: string; mono: boolean } {
  if (item.detail) return { text: item.detail, mono: true };
  if (kind === "execute") return { text: commandOf(item.input), mono: true };
  if (kind === "search") return { text: inputField(item.input, "pattern", "query", "path"), mono: true };
  if (kind === "fetch") return { text: inputField(item.input, "url", "query"), mono: false };
  return { text: inputField(item.input, "Path", "path", "file_path", "filePath", "file"), mono: true };
}

/**
 * Turns any harness's tool call into crisp row pieces: normalized verbs and
 * resolved paths for file calls, concise `toolSummary` + cleaned commands for
 * shells, and readable names for MCP tools.
 */
export function formatToolRow(item: ToolItem, kind: string, cwd: string): FormattedToolRow {
  const fileMatch = FILE_TITLE.exec(item.title.trim());
  const isFileKind = kind === "read" || kind === "edit" || kind === "delete" || kind === "move";
  if (isFileKind || (fileMatch && kind !== "execute" && kind !== "search" && kind !== "fetch")) {
    const rawPath =
      inputField(item.input, "Path", "path", "file_path", "filePath", "file") ??
      item.diffs[0]?.path ??
      fileMatch?.[2];
    if (rawPath) {
      const display = cleanToolPath(rawPath, cwd);
      const { pathDir, pathFile } = splitPathDisplay(display);
      return {
        title: fileVerb(kind, fileMatch?.[1], item),
        pathDir,
        pathFile,
        lineRange: kind === "read" ? fileLineRange(item.input) : undefined,
        summary: usefulSummary(item.summary),
        detailMono: true,
      };
    }
  }

  if (kind === "execute") {
    const rawCmd = commandOf(item.input) ?? item.detail ?? item.title;
    const cleaned = stripLeadingCd(displayCommand(rawCmd), cwd);
    const toolSummary = usefulSummary(inputField(item.input, "toolSummary"));
    if (toolSummary && toolSummary !== cleaned) {
      return {
        title: toolSummary,
        detail: cleaned || undefined,
        detailMono: true,
      };
    }
    const label = displayTool(item.title, item.input, item.status);
    const title = stripLeadingCd(displayCommand(label.title), cwd);
    const detail = cleaned && !title.includes(cleaned) ? cleaned : undefined;
    return {
      title,
      titleMono: true,
      summary: usefulSummary(item.summary) ?? label.summary,
      detail,
      detailMono: true,
    };
  }

  const label = displayTool(item.title, item.input, item.status);
  const title = displayCommand(label.title);
  const summary = usefulSummary(item.summary) ?? label.summary;
  const rawDetail = fallbackDetail(item, kind);
  const relative =
    rawDetail.text && cwd && rawDetail.text.startsWith(`${cwd}/`)
      ? rawDetail.text.slice(cwd.length + 1)
      : rawDetail.text;
  const trimmed = relative ? displayCommand(relative) : undefined;
  const detail = trimmed && !title.includes(trimmed) && !summary?.includes(trimmed) ? trimmed : undefined;
  return {
    title,
    summary,
    detail,
    detailMono: rawDetail.mono,
  };
}
