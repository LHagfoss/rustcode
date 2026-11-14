// A pull request's review in the right panel: its diff as files (renames with
// their old path, large files folded), and its review conversations, each
// placed on the diff line it was left on. Pure, so tests drive it.

import type { PullDetail, PullLineComment } from "../acp/rustcode";
import type { FeedbackItem } from "../lib/reviewFeedback";
import { type PatchLine, parsePatch } from "./diff";

/** A file with more diff lines than this starts folded. */
export const LARGE_FILE_LINES = 400;

export interface PullDiffFile {
  path: string;
  /** The path before a rename. */
  oldPath?: string;
  /** `A`dded, `D`eleted, `R`enamed or `M`odified. */
  status: "A" | "D" | "R" | "M";
  added: number;
  removed: number;
  lines: PatchLine[];
  binary: boolean;
  /** Too long to draw open: it starts folded. */
  large: boolean;
}

/** `gh pr diff` output as files, in the order the diff lists them. */
export function pullDiffFiles(diff: string): PullDiffFile[] {
  const out: PullDiffFile[] = [];
  let file: { head: PullDiffFile; body: string[] } | null = null;
  const finish = () => {
    if (!file) return;
    const lines = parsePatch(file.body);
    out.push({
      ...file.head,
      lines,
      added: lines.filter((l) => l.kind === "add").length,
      removed: lines.filter((l) => l.kind === "del").length,
      large: lines.length > LARGE_FILE_LINES,
    });
    file = null;
  };
  for (const line of diff.split("\n")) {
    const header = /^diff --git a\/(.+?) b\/(.+)$/.exec(line);
    if (header) {
      finish();
      const [, from, to] = header;
      file = {
        head: {
          path: to!,
          status: from === to ? "M" : "R",
          ...(from !== to ? { oldPath: from! } : {}),
          added: 0,
          removed: 0,
          lines: [],
          binary: false,
          large: false,
        },
        body: [],
      };
      continue;
    }
    if (!file) continue;
    const { head, body } = file;
    if (body.length === 0) {
      // The extended header, before the first hunk.
      if (line.startsWith("new file mode")) head.status = "A";
      else if (line.startsWith("deleted file mode")) head.status = "D";
      else if (line.startsWith("rename from ")) {
        head.oldPath = line.slice("rename from ".length);
        head.status = "R";
      } else if (line.startsWith("rename to ")) head.path = line.slice("rename to ".length);
      else if (line.startsWith("Binary files ") || line === "GIT binary patch") head.binary = true;
      if (!line.startsWith("@@")) continue;
    }
    body.push(line);
  }
  finish();
  return out;
}

/** One review conversation: its first comment, the replies under it, and where it is. */
export interface Conversation {
  /** The first comment's id. */
  id: number;
  /** The provider's id for the conversation, when it was read. */
  threadId?: string;
  path: string;
  line?: number;
  startLine?: number;
  side: "LEFT" | "RIGHT";
  resolved: boolean;
  outdated: boolean;
  comments: PullLineComment[];
}

/**
 * The line comments as conversations, oldest first: a reply joins the
 * conversation its first comment opened (following a reply to a reply), and
 * the review threads read with them say which are resolved.
 */
export function reviewConversations(
  detail: Pick<PullDetail, "reviewComments" | "reviewThreads">,
): Conversation[] {
  const byId = new Map(detail.reviewComments.map((c) => [c.id, c]));
  const rootOf = (c: PullLineComment): PullLineComment => {
    let at = c;
    const seen = new Set<number>();
    while (at.replyTo !== undefined && byId.has(at.replyTo) && !seen.has(at.id)) {
      seen.add(at.id);
      at = byId.get(at.replyTo)!;
    }
    return at;
  };
  const threads = new Map((detail.reviewThreads ?? []).map((t) => [t.rootComment, t]));
  const out = new Map<number, Conversation>();
  for (const comment of detail.reviewComments) {
    const root = rootOf(comment);
    let conversation = out.get(root.id);
    if (!conversation) {
      const thread = threads.get(root.id);
      conversation = {
        id: root.id,
        ...(thread?.id ? { threadId: thread.id } : {}),
        path: root.path,
        line: root.line,
        startLine: root.startLine,
        side: root.side ?? "RIGHT",
        resolved: thread?.resolved ?? false,
        outdated: root.outdated || (thread?.outdated ?? false),
        comments: [],
      };
      out.set(root.id, conversation);
    }
    conversation.comments.push(comment);
  }
  for (const conversation of out.values()) conversation.comments.sort((a, b) => (a.at ?? 0) - (b.at ?? 0));
  return [...out.values()].sort((a, b) => (a.comments[0]?.at ?? 0) - (b.comments[0]?.at ?? 0));
}

/** Where a file's conversations go: under a diff line by index, or above the file. */
export interface Placement {
  /** Line index in the file's `lines` to the conversations drawn under it. */
  atLine: Map<number, Conversation[]>;
  /** Outdated, or on a line the diff does not show: drawn above the file. */
  unplaced: Conversation[];
}

/**
 * The file's conversations on its diff lines. A conversation goes under the
 * last line of its range on its side (the base's number on `LEFT`, the pull
 * request's on `RIGHT`); an outdated one, or one whose line is not in a hunk,
 * goes above the file.
 */
export function placeConversations(
  file: Pick<PullDiffFile, "path" | "lines">,
  conversations: readonly Conversation[],
): Placement {
  const atLine = new Map<number, Conversation[]>();
  const unplaced: Conversation[] = [];
  for (const conversation of conversations) {
    if (conversation.path !== file.path) continue;
    const at =
      conversation.outdated || conversation.line === undefined
        ? -1
        : file.lines.findIndex((l) =>
            conversation.side === "LEFT"
              ? l.kind !== "add" && l.kind !== "hunk" && l.old === conversation.line
              : l.kind !== "del" && l.kind !== "hunk" && l.new === conversation.line,
          );
    if (at < 0) unplaced.push(conversation);
    else atLine.set(at, [...(atLine.get(at) ?? []), conversation]);
  }
  return { atLine, unplaced };
}

/** A conversation as feedback items for the composer draft: its comments in order, with their place. */
export function conversationFeedback(conversation: Conversation): FeedbackItem[] {
  return conversation.comments
    .filter((c) => c.body.trim())
    .map((c, i) => ({
      key: `line:${c.id}`,
      author: c.author,
      kind: "line",
      body: c.body,
      path: conversation.path || undefined,
      line: conversation.line,
      startLine: conversation.startLine,
      outdated: conversation.outdated,
      reply: i > 0,
      url: c.url,
      at: c.at,
    }));
}

/** Every unresolved conversation's feedback, in order. */
export function unresolvedFeedback(conversations: readonly Conversation[]): FeedbackItem[] {
  return conversations.filter((c) => !c.resolved).flatMap(conversationFeedback);
}

/** The line range a conversation is on, as GitHub writes it: `12` or `10-12`. */
export function conversationRange(conversation: Pick<Conversation, "line" | "startLine">): string {
  if (conversation.line === undefined) return "";
  return conversation.startLine !== undefined && conversation.startLine !== conversation.line
    ? `${conversation.startLine}-${conversation.line}`
    : `${conversation.line}`;
}
