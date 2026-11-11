// The one line a delegate's row shows of what it answered: plain words, never the markup around
// them. A reply that opens with a table gave the row `| Item | Value |` in the failure colour.

const TABLE_ROW = /^\|.*\|?$/;
const TABLE_RULE = /^\|?[\s:|-]+\|?$/;

/** A markdown line as plain text: no list or heading marker, emphasis, code ticks or link syntax. */
function plain(line: string): string {
  return line
    .replace(/^(?:[>\-*+]\s+|#{1,6}\s+|\d+[.)]\s+)+/, "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/(\*\*|__|[*`])/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * The first line of an answer worth showing, as plain text. A table row is not a sentence: the
 * first prose line wins, and an answer that is only a table reads as its first row's cells.
 */
export function answerPreview(text: string | null | undefined): string | undefined {
  const lines = (text ?? "")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("```") && !/^(?:-{3,}|\*{3,}|_{3,})$/.test(line));
  const prose = lines.find((line) => !TABLE_ROW.test(line));
  if (prose) return plain(prose) || undefined;
  const row = lines.find((line) => !TABLE_RULE.test(line));
  if (!row) return undefined;
  const cells = row
    .split("|")
    .map(plain)
    .filter((cell) => cell);
  return cells.join(" · ") || undefined;
}
