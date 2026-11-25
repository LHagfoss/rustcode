export const QUOTE_MAX_CHARS = 4000;

/**
 * Marked text as a Markdown block quote for the composer: each line behind `> `, outer blank
 * lines dropped and runs of blank lines kept as one, cut at `QUOTE_MAX_CHARS` with an ellipsis.
 * Empty when nothing but whitespace was marked.
 */
export function quoteMarkdown(selected: string): string {
  const trimmed = selected.replace(/\r\n?/g, "\n").replace(/ /g, " ").trim();
  if (!trimmed) return "";
  const cut = trimmed.length > QUOTE_MAX_CHARS ? `${trimmed.slice(0, QUOTE_MAX_CHARS).trimEnd()}…` : trimmed;
  return cut
    .split("\n")
    .map((line) => line.trimEnd())
    .filter((line, i, lines) => line !== "" || lines[i - 1] !== "")
    .map((line) => (line ? `> ${line}` : ">"))
    .join("\n");
}
