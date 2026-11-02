// File references in replies (docs/output-style.md): inline code like
// `src/a.rs:12` or `src/a.rs:12:4`, and relative Markdown links like
// [a.rs](src/a.rs#L12), open the file in the user's editor at that line,
// resolved against the thread's folder.

export interface FileRef {
  path: string;
  line?: number;
  column?: number;
}

const SCHEME = /^[a-z][a-z0-9+.-]*:/i;
// A path (with a slash or an extension), optionally `:line` or `:line:col`. No spaces.
const INLINE = /^((?:~|\.{1,2})?\/?[\w@.+-]+(?:\/[\w@.+-]+)*)(?::(\d+)(?::(\d+))?)?$/;

/** Inline code that names a file: needs a slash or an extension, and is not a URL or a number. */
export function parseInlineRef(text: string): FileRef | null {
  const m = INLINE.exec(text.trim());
  if (!m || SCHEME.test(text)) return null;
  const path = m[1]!;
  const hasExt = /\.[A-Za-z0-9]{1,8}$/.test(path) && !/^\d+(\.\d+)*$/.test(path);
  if (!path.includes("/") && !hasExt) return null;
  if (!hasExt && !m[2]) return null;
  return { path, line: m[2] ? Number(m[2]) : undefined, column: m[3] ? Number(m[3]) : undefined };
}

/** A Markdown link target that is a file in the workspace, not a web link or an anchor. */
export function parseLinkRef(href: string): FileRef | null {
  if (!href || href.startsWith("#") || SCHEME.test(href) || href.startsWith("//")) return null;
  const [rawPath, hash] = href.split("#", 2);
  let path = rawPath ?? "";
  try {
    path = decodeURIComponent(path);
  } catch {
    // Keep it as written.
  }
  if (!path) return null;
  const lines = /^L(\d+)(?:C(\d+))?/.exec(hash ?? "");
  const inline = parseInlineRef(path);
  return {
    path: inline?.path ?? path,
    line: lines ? Number(lines[1]) : inline?.line,
    column: lines?.[2] ? Number(lines[2]) : inline?.column,
  };
}

/** The absolute path a reference names, against the thread's folder. */
export function resolveRef(ref: FileRef, cwd: string): string {
  if (ref.path.startsWith("/") || ref.path.startsWith("~/")) return ref.path;
  const parts = `${cwd.replace(/\/$/, "")}/${ref.path}`.split("/");
  const out: string[] = [];
  for (const part of parts) {
    if (part === "..") out.pop();
    else if (part !== "." && part !== "") out.push(part);
  }
  return `/${out.join("/")}`;
}
