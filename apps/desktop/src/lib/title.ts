/**
 * A title from a message's first line, at most `max` characters: cut at a word and marked with
 * "…" when it is shortened, so it never ends mid-word ("…with pytest te").
 */
export function titleFrom(text: string, max: number): string {
  const line = (text.split("\n")[0] ?? "").trim();
  if (line.length <= max) return line;
  const cut = line.slice(0, max - 1);
  const space = cut.lastIndexOf(" ");
  return `${(space > max / 2 ? cut.slice(0, space) : cut).trimEnd()}…`;
}
