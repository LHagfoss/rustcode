import { PULL_MARK, type QueuedPrompt, stripMarks, withPullMark } from "./thread";

/** What one queued notice says, per pull request, in the order the news arrived. */
export type PullNews = [number, string[]][];

const OUTCOMES = new Set(["Merged", "Closed without merging"]);
const isCheckNews = (message: string) =>
  message.startsWith("Checks failed") || message === "All checks passed";
const isSupersededByOutcome = (message: string) =>
  isCheckNews(message) || message === "Has merge conflicts with its base";

/**
 * Only the news that still holds: a merge or close makes earlier check and conflict news moot, and
 * of several check results only the newest counts. A thread told "Checks failed" after the merge it
 * already made spends a whole turn answering news that is no longer true.
 */
export function currentNews(messages: readonly string[]): string[] {
  const closed = messages.some((message) => OUTCOMES.has(message));
  const lastCheck = messages.findLastIndex(isCheckNews);
  const kept = messages.filter((message, at) => {
    if (closed && isSupersededByOutcome(message)) return false;
    return !isCheckNews(message) || at === lastCheck;
  });
  return [...new Set(kept)];
}

/** A pull notice as `pullNotice` writes it, one line per batch; null for any other message. */
export function parsePullNotice(text: string): PullNews | null {
  if (!text.includes(PULL_MARK)) return null;
  const body = stripMarks(text);
  if (body.includes("\n")) return null;
  const head = /^Pull request (#\d+: .*)$/.exec(body);
  if (!head?.[1]) return null;
  const news: PullNews = [];
  for (const part of head[1].split(/ · (?=#\d+: )/)) {
    const m = /^#(\d+): (.+)$/.exec(part);
    if (!m?.[1] || !m[2]) return null;
    news.push([Number(m[1]), m[2].split(" · ")]);
  }
  return news;
}

/** The one notice line for `news`: "Pull request #12: Merged · #14: All checks passed". */
export function pullNoticeText(news: PullNews): string {
  return withPullMark(
    `Pull request ${news.map(([number, said]) => `#${number}: ${said.join(" · ")}`).join(" · ")}`,
  );
}

/**
 * A busy thread's queue with this pull notice in it: folded into the newest queued pull notice, so
 * the thread wakes once for all of its pull requests' news and hears only what still holds, else a
 * row of its own. The row keeps the id it was queued with.
 */
export function queuePullNotice(
  queue: readonly QueuedPrompt[],
  text: string,
  row: () => QueuedPrompt,
): QueuedPrompt[] {
  const incoming = parsePullNotice(text);
  if (incoming) {
    for (let at = queue.length - 1; at >= 0; at--) {
      const queued = queue[at]!;
      const news = queued.extras ? null : parsePullNotice(queued.text);
      if (!news) continue;
      const merged = new Map(news.map(([number, said]) => [number, [...said]]));
      for (const [number, said] of incoming) merged.set(number, [...(merged.get(number) ?? []), ...said]);
      const folded: PullNews = [...merged].map(([number, said]) => [number, currentNews(said)]);
      const rows = [...queue];
      rows[at] = { ...queued, text: pullNoticeText(folded) };
      return rows;
    }
  }
  return [...queue, row()];
}
