// Review feedback into a thread: the comments of a pull request the
// user picks become a draft in the composer, with the file and line each one
// is on. Nothing here sends, replies on GitHub or resolves a conversation.

import type { PullDetail, PullRequest } from "../acp/rustcode";

/** One piece of feedback the user can pick: a line comment, a review or a conversation comment. */
export interface FeedbackItem {
  /** Stable across reads of the same pull request, so a selection survives a refresh. */
  key: string;
  author: string;
  /** `line` for a comment on the diff, else the activity's kind (`commented`, `changes_requested`…). */
  kind: string;
  body: string;
  path?: string;
  line?: number;
  startLine?: number;
  outdated?: boolean;
  /** A line comment that answers another. */
  reply?: boolean;
  url?: string;
  at?: number;
}

/** The pull request's feedback that says something: line comments, then reviews and comments with a body. */
export function feedbackItems(detail: PullDetail): FeedbackItem[] {
  const lines: FeedbackItem[] = detail.reviewComments
    .filter((c) => c.body.trim())
    .map((c) => ({
      key: `line:${c.id}`,
      author: c.author,
      kind: "line",
      body: c.body,
      path: c.path || undefined,
      line: c.line,
      startLine: c.startLine,
      outdated: c.outdated,
      reply: c.replyTo !== undefined,
      url: c.url,
      at: c.at,
    }));
  const activity: FeedbackItem[] = detail.activity
    .filter((a) => a.body.trim())
    .map((a) => ({
      key: `activity:${a.kind}:${a.author}:${a.at ?? ""}:${a.url ?? ""}`,
      author: a.author,
      kind: a.kind.toLowerCase(),
      body: a.body,
      url: a.url,
      at: a.at,
    }));
  return [...lines, ...activity];
}

/** The keys still on the pull request: a comment deleted since it was picked drops out. */
export function keepSelected(selected: ReadonlySet<string>, items: readonly FeedbackItem[]): Set<string> {
  const present = new Set(items.map((item) => item.key));
  return new Set([...selected].filter((key) => present.has(key)));
}

/** Where a line comment is: `path:line`, `path:start-end` for a range, the path alone without a line. */
export function feedbackLocation(item: FeedbackItem): string | undefined {
  if (!item.path) return undefined;
  if (item.line === undefined) return item.path;
  const range =
    item.startLine !== undefined && item.startLine !== item.line
      ? `${item.startLine}-${item.line}`
      : `${item.line}`;
  return `${item.path}:${range}`;
}

const KIND_LABEL: Record<string, string> = {
  commented: "commented",
  approved: "approved",
  changes_requested: "requested changes",
  dismissed: "review dismissed",
};

/** The draft for `items`: what to do, the pull request, then one bullet per comment with its body quoted. */
export function formatReviewFeedback(
  items: readonly FeedbackItem[],
  pr: Pick<PullRequest, "number" | "title" | "url" | "headSha">,
): string {
  if (items.length === 0) return "";
  const head = [`Address this review feedback on PR #${pr.number}`];
  if (pr.title) head.push(` (${pr.title})`);
  head.push(":");
  const lines = [head.join("")];
  const about = [pr.url, pr.headSha ? `at commit ${pr.headSha.slice(0, 7)}` : undefined].filter(Boolean);
  if (about.length) lines.push(about.join(" "));
  lines.push("");
  for (const item of items) {
    const who = item.author || "a deleted user";
    const where = feedbackLocation(item);
    const notes = [item.outdated && "outdated: the code has changed since", item.reply && "reply"].filter(
      Boolean,
    );
    const said = item.kind === "line" ? "" : ` ${KIND_LABEL[item.kind] ?? item.kind.replace(/_/g, " ")}`;
    let bullet = `- **${who}**${said}`;
    if (where) bullet += ` on \`${where}\``;
    if (notes.length) bullet += ` (${notes.join(", ")})`;
    if (item.url) bullet += ` ${item.url}`;
    lines.push(`${bullet}:`);
    for (const line of item.body.trim().split("\n")) lines.push(line ? `  > ${line}` : "  >");
    lines.push("");
  }
  return `${lines.join("\n").trimEnd()}\n`;
}

/** `draft` with `addition` after it, a blank line between when the draft already says something. */
export function appendToDraft(draft: string, addition: string): string {
  if (!draft.trim()) return addition;
  return `${draft.trimEnd()}\n\n${addition}`;
}
