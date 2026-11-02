// A report command's answer (`/status`, `/stats`, `/context`…) read for the
// panel over the composer: a title when the text opens with a bold line, and
// label/value rows when its lines are `Label: value` (with optional headings),
// else Markdown. Pure, so the panel and the tests share it.

export interface ReportSection {
  heading?: string;
  rows: { label: string; value: string }[];
}

export interface ReportView {
  title?: string;
  /** Set when the whole body reads as label/value rows. */
  sections?: ReportSection[];
  /** The text without its title, for the Markdown fallback. */
  body: string;
}

const BOLD_LINE = /^\*\*([^*]+)\*\*:?$/;
const HEADING = /^(?:#{1,4}\s+(.+)|\*\*([^*]+)\*\*:?)$/;
const ROW = /^(?:[-*]\s+)?\**([A-Za-z][\w ./()'-]{0,40}?)\**:\s+(.+)$/;

export function readReport(text: string): ReportView {
  const lines = text.replace(/\r/g, "").trim().split("\n");
  let title: string | undefined;
  const bold = BOLD_LINE.exec(lines[0]?.trim() ?? "");
  if (bold) {
    title = bold[1]!.trim();
    lines.shift();
  }
  const body = lines.join("\n").trim();
  const filled = lines.map((l) => l.trim()).filter(Boolean);
  if (filled.filter((l) => ROW.test(l)).length < 2) return { title, body };
  const sections: ReportSection[] = [];
  let current: ReportSection = { rows: [] };
  for (const line of filled) {
    const row = ROW.exec(line);
    const heading = row ? null : HEADING.exec(line);
    if (row) {
      current.rows.push({ label: row[1]!.trim(), value: row[2]!.replace(/^`([^`]*)`$/, "$1").trim() });
    } else if (heading) {
      if (current.rows.length || current.heading) sections.push(current);
      current = { heading: (heading[1] ?? heading[2])!.trim(), rows: [] };
    } else {
      // Prose in between: the report stays Markdown rather than losing a line.
      return { title, body };
    }
  }
  if (current.rows.length || current.heading) sections.push(current);
  return { title, sections, body };
}
