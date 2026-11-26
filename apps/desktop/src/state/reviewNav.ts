import { resolveRef } from "../lib/fileRefs";
import type { PatchLine, ReviewComment } from "./diff";
import { type Conversation, type PullDiffFile, placeConversations } from "./pullReview";

/** One draft comment in review reading order, keyed for DOM scrolling and highlight. */
export interface OrderedReviewComment {
  id: string;
  comment: ReviewComment;
  path: string;
  line: number;
  side: "LEFT" | "RIGHT";
}

/** One pull request conversation in the order `PullDiff` renders it. */
export interface OrderedPullConversation {
  id: number;
  path: string;
  conversation: Conversation;
  placed: boolean;
}

/** A file and 1-based line number resolved on disk for the external editor. */
export interface EditorLineTarget {
  path: string;
  line: number;
}

/** Deterministic DOM key for each draft comment reference in a review. */
export function reviewCommentKeyMap(comments: readonly ReviewComment[]): Map<ReviewComment, string> {
  const seen = new Map<string, number>();
  const out = new Map<ReviewComment, string>();
  for (const comment of comments) {
    const base = `${comment.path}\u0000${comment.side}\u0000${comment.line}`;
    const occurrence = seen.get(base) ?? 0;
    seen.set(base, occurrence + 1);
    out.set(comment, `${base}\u0000${occurrence}`);
  }
  return out;
}

/** Draft comments ordered by file tree order, then line number, then base before head. */
export function orderedReviewComments(
  comments: readonly ReviewComment[],
  fileOrder: readonly string[],
): OrderedReviewComment[] {
  const keys = reviewCommentKeyMap(comments);
  const rank = new Map(fileOrder.map((path, index) => [path, index]));
  const items = comments.map((comment, insertion) => ({
    id: keys.get(comment)!,
    comment,
    path: comment.path,
    line: comment.line,
    side: comment.side,
    insertion,
  }));
  items.sort((a, b) => {
    const fileA = rank.get(a.path);
    const fileB = rank.get(b.path);
    if (fileA !== undefined && fileB !== undefined && fileA !== fileB) return fileA - fileB;
    if (fileA !== undefined && fileB === undefined) return -1;
    if (fileA === undefined && fileB !== undefined) return 1;
    if (a.path !== b.path) return a.path.localeCompare(b.path);
    const startA = a.comment.startLine ?? a.line;
    const startB = b.comment.startLine ?? b.line;
    if (startA !== startB) return startA - startB;
    if (a.line !== b.line) return a.line - b.line;
    if (a.side !== b.side) return a.side === "LEFT" ? -1 : 1;
    return a.insertion - b.insertion;
  });
  return items.map(({ insertion: _insertion, ...entry }) => entry);
}

/** Pull request conversations ordered to match `PullDiff`: elsewhere, then each file's unplaced and placed threads. */
export function orderedPullConversations(
  conversations: readonly Conversation[],
  files: readonly Pick<PullDiffFile, "path" | "lines">[],
): OrderedPullConversation[] {
  const out: OrderedPullConversation[] = [];
  const shown = new Set(files.map((file) => file.path));
  for (const conversation of conversations) {
    if (!shown.has(conversation.path)) {
      out.push({ id: conversation.id, path: conversation.path, conversation, placed: false });
    }
  }
  for (const file of files) {
    const placement = placeConversations(file, conversations);
    for (const conversation of placement.unplaced) {
      out.push({ id: conversation.id, path: file.path, conversation, placed: false });
    }
    const lineIndices = [...placement.atLine.keys()].sort((a, b) => a - b);
    for (const index of lineIndices) {
      for (const conversation of placement.atLine.get(index) ?? []) {
        out.push({ id: conversation.id, path: file.path, conversation, placed: true });
      }
    }
  }
  return out;
}

/** Steps forward or backward through ordered review targets, wrapping at the ends. */
export function stepTarget<T extends { id: string | number }>(
  targets: readonly T[],
  activeId: T["id"] | null,
  step: 1 | -1,
): { item: T; index: number } | null {
  if (!targets.length) return null;
  const current = activeId === null ? -1 : targets.findIndex((target) => target.id === activeId);
  const index =
    current < 0
      ? step > 0
        ? 0
        : targets.length - 1
      : (((current + step) % targets.length) + targets.length) % targets.length;
  return { item: targets[index]!, index };
}

/** Checkout root folder on disk for a Review diff source. */
export function diffRootForReview(
  sourceKind: "uncommitted" | "branch" | "turn" | "pull" | "commit",
  cwd: string,
  worktreePath?: string,
): string {
  if ((sourceKind === "branch" || sourceKind === "commit") && worktreePath) return worktreePath;
  return cwd;
}

/**
 * Resolves a diff line to an absolute path and 1-based line number for `openInEditor`,
 * or null for remote threads, deleted files, hunk headers, and removed lines.
 */
export function diffLineEditorTarget(
  root: string,
  filePath: string,
  line: Pick<PatchLine, "kind" | "new">,
  options: { local?: boolean; fileStatus?: string } = {},
): EditorLineTarget | null {
  if (options.local === false) return null;
  if (!root || !filePath) return null;
  if (options.fileStatus === "D") return null;
  if (line.kind === "del" || line.kind === "hunk") return null;
  if (line.new === undefined || line.new < 1) return null;
  return { path: resolveRef({ path: filePath, line: line.new }, root), line: line.new };
}
