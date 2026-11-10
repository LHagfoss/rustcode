// The files one thread's agent attaches to a message it sends another one
// (`message_session`, docs/orchestrator.md). A path is accepted when it is
// absolute and resolves inside the caller's project or its worktree, so a
// delegate cannot hand a sibling the user's keys. The rules are pure and
// tested here; the shell measures the file, which the web view cannot: it
// refuses one over 5 MB and one whose resolved path leaves the roots
// (`src-tauri/src/native.rs` `attached_files`, `MAX_ATTACHMENT_BYTES`).

import type { Attachment, ComposeExtras } from "./compose";
import { type Failure, failure, projectOf, type RuleThread } from "./orchestration";

/** How many files one `message_session` attaches. The shell counts the same. */
export const MAX_ATTACHMENT_FILES = 10;

/** An absolute path as `native.rs` reads one: `/…` on macOS and Linux, `C:\…` on Windows. */
const ABSOLUTE = /^(?:[A-Za-z]:)?[\\/]/;

/**
 * A path as one spelling for comparison: one separator, no trailing slash, no
 * `.` or `..` segment that would walk out of a root textually, and macOS's
 * `/private/tmp` read as `/tmp`. A Windows path keeps its drive, in the case it
 * was written in, since `C:` and `c:` name the same folder.
 */
function clean(path: string): string {
  const slashed = path.replace(/\\/g, "/");
  const drive = /^([A-Za-z]:)/.exec(slashed)?.[1] ?? "";
  const rest = slashed.slice(drive.length);
  const parts: string[] = [];
  for (const part of rest.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  const normalized = `${drive}/${parts.join("/")}`;
  return !drive && /^\/private\/(tmp|var|etc)(\/|$)/.test(normalized)
    ? normalized.slice("/private".length)
    : normalized;
}

/** Where an attached file may live: the caller's own folder (its worktree, when
 * it runs in one) and its project. All are absolute, resolved paths. */
export function attachmentRoots(caller: RuleThread): string[] {
  const roots = [caller.folder, caller.worktree?.path, projectOf(caller)].flatMap((path) =>
    path && ABSOLUTE.test(path) ? [clean(path)] : [],
  );
  return [...new Set(roots)];
}

/**
 * Why `path` cannot be attached, or null when it can hold. A relative path is
 * read against no folder at all: an agent names an absolute file it can see,
 * so one that does not is a mistake worth saying back, never a guess. The
 * comparison is per segment (`root/…`, not `root-other`), which is what the
 * shell's `PathBuf::starts_with` does to the resolved path.
 */
export function attachmentRefusal(path: string, roots: readonly string[]): Failure | null {
  if (!ABSOLUTE.test(path)) {
    return failure("invalid", `${path} is not an absolute path`);
  }
  const file = clean(path);
  if (!roots.length) return failure("not_allowed", "this thread has no folder to attach a file from");
  const inside = roots.some((root) => {
    const base = clean(root);
    // Windows names a folder case-insensitively; a POSIX path is not.
    const same = /^[A-Za-z]:/.test(base) ? (path: string) => path.toLowerCase() : (path: string) => path;
    const [a, b] = [same(file), same(base)];
    return a === b || a.startsWith(`${b}/`);
  });
  if (!inside) {
    return failure(
      "not_allowed",
      `${path} is outside this thread's project; only files under it can be attached`,
    );
  }
  return null;
}

/**
 * What the composer's attachment path needs for one message: each file checked
 * and measured by the shell, so the turn carries the same chips a person's own
 * send does.
 */
export interface AttachedFile {
  path: string;
  name: string;
  bytes: number;
}

/**
 * The attachments a message sends: the composer's `file` kind, one per path,
 * with the name the shell measured. Refuses the whole message when one file
 * cannot go, so nothing is half-sent.
 */
export function attachedExtras(files: readonly AttachedFile[], links: readonly string[] = []): ComposeExtras {
  const attachments: Attachment[] = files.map((file) => ({
    id: `file-${file.path}`,
    kind: "file",
    name: file.name,
    path: file.path,
  }));
  return { links: [...links], attachments };
}
