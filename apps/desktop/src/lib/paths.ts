/** Lexical paths for either host: no filesystem or symlink resolution. */
function windows(path: string): boolean {
  return /^[A-Za-z]:/.test(path) || path.includes("\\") || path.startsWith("//");
}

export function isAbsolute(path: string): boolean {
  return /^(?:[A-Za-z]:[\\/]|[\\/])/.test(path);
}

/** Collapse separators and dot segments, keeping drive and UNC share roots. */
export function normalize(path: string): string {
  const separator = windows(path) ? "\\" : "/";
  const slashed = path.replace(/\\/g, "/");
  const root = /^(?:[A-Za-z]:\/|\/\/[^/]+\/[^/]+(?:\/|$)|\/)/.exec(slashed)?.[0] ?? "";
  const drive = !root ? (/^[A-Za-z]:/.exec(slashed)?.[0] ?? "") : "";
  const parts: string[] = [];
  for (const part of slashed.slice(root.length + drive.length).split("/")) {
    if (!part || part === ".") continue;
    if (part === ".." && parts.length && parts.at(-1) !== "..") parts.pop();
    else if (part !== ".." || !root) parts.push(part);
  }
  const prefix = root && !root.endsWith("/") ? `${root}/` : root;
  return (prefix + drive + parts.join("/") || ".").replace(/\//g, separator);
}

export function join(root: string, path: string): string {
  if (isAbsolute(path)) return normalize(path);
  return normalize(`${root}${windows(root) ? "\\" : "/"}${path}`);
}

export function basename(path: string): string {
  const normalized = normalize(path).replace(/[\\/]+$/, "");
  return normalized.slice(Math.max(normalized.lastIndexOf("/"), normalized.lastIndexOf("\\")) + 1);
}

export function dirname(path: string): string {
  const normalized = normalize(path);
  if (/^(?:[A-Za-z]:[\\/]|[\\/]{2}[^\\/]+[\\/][^\\/]+[\\/]?)$/.test(normalized)) return normalized;
  const index = Math.max(normalized.lastIndexOf("/"), normalized.lastIndexOf("\\"));
  if (index < 0) return ".";
  const parent = normalized.slice(0, index + 1);
  return normalize(parent);
}

/** Includes the root itself; compares whole segments, ignoring case on Windows. */
export function isInside(child: string, root: string): boolean {
  if (!root || isAbsolute(child) !== isAbsolute(root)) return false;
  const compare = (path: string) => {
    const normalized = normalize(path).replace(/\\/g, "/").replace(/\/$/, "");
    return windows(root) ? normalized.toLowerCase() : normalized;
  };
  const base = compare(root);
  const path = compare(child);
  return path === base || path.startsWith(`${base}/`);
}
