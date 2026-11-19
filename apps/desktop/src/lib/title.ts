import { clip } from "./clip";

/**
 * A title from a message's first line that says something, at most `max` characters: cut at a
 * word and marked with "…" when it is shortened, so it never ends mid-word ("…with pytest te").
 * A blank first line (a message that opens with a newline) is skipped, so a title is never
 * empty — no thread is left to be named by the message it is sent later.
 */
export function titleFrom(text: string, max: number): string {
  const line =
    text
      .split("\n")
      .map((line) => {
        const heading = /^ {0,3}#{1,6}(?=[ \t]|$)(.*)$/.exec(line.trimEnd());
        return (heading ? (heading[1] ?? "").replace(/[ \t]+#+$/, "") : line).trim();
      })
      .find(Boolean) ?? "";
  if (line.length <= max) return line;
  // A character (an emoji, an accented letter) is never cut in half by the "…".
  const cut = clip(line, max - 1);
  const space = cut.lastIndexOf(" ");
  return `${(space > max / 2 ? cut.slice(0, space) : cut).trimEnd()}…`;
}

/**
 * A started child's title: the one its parent gave it, else the task's first
 * words. `newThread` in state/host.ts uses this so a child never goes unnamed —
 * a session without a title takes the message it is sent ("From thread …") as
 * one, and the sidebar would show that header for good.
 */
export function childTitle(title: string | undefined, task: string): string {
  return title?.trim() || titleFrom(task, 60);
}
