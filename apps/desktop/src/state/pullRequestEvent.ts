/** What a pull request wake line reports, read from its words: a merge, a failure, or a comment. */
export type PullRequestEvent = "merged" | "failed" | "comment";

/**
 * The event a wake divider names, for its icon and tone. A line naming several pull requests is
 * as bad as its worst: a failure or conflict anywhere in it makes it a failure.
 */
export function pullRequestEvent(line: string): PullRequestEvent | undefined {
  if (!/^Pull request #\d+/.test(line)) return undefined;
  const words = line.toLowerCase();
  if (/\bfailed\b|conflict|nothing was merged/.test(words)) return "failed";
  if (/merged/.test(words)) return "merged";
  if (/review|comment/.test(words)) return "comment";
  return undefined;
}
