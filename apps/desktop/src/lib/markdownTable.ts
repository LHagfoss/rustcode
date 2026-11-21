// Tables in replies: which columns hold figures (they align right, in tabular
// digits), and the two copies people want: Markdown back, CSV for a spreadsheet.

export interface PlainTable {
  header: string[];
  rows: string[][];
}

/** A figure: an optional sign and currency, digits with separators, an optional unit or percent. */
const FIGURE =
  /^[-+−±~≈]?\s?[$€£¥]?\s?\d[\d,.\s']*(\s?(%|‰|[kKmMbBtT]|ms|µs|ns|s|m|h|d|x|×|pts?|KB|MB|GB|TB|kb|mb|gb|rps|req\/s|\/s|tok|tokens))?$/;

export function isFigure(text: string): boolean {
  const t = text.trim();
  return t.length > 0 && FIGURE.test(t);
}

/** Columns where at least four in five filled cells are figures. */
export function numericColumns(table: PlainTable): boolean[] {
  return table.header.map((_, i) => {
    let filled = 0;
    let figures = 0;
    for (const row of table.rows) {
      const cell = row[i]?.trim() ?? "";
      if (!cell || cell === "-" || cell === "—") continue;
      filled++;
      if (isFigure(cell)) figures++;
    }
    return filled > 0 && figures / filled >= 0.8;
  });
}

const mdCell = (text: string) =>
  text
    .replace(/\|/g, "\\|")
    .replace(/\s*\n\s*/g, " ")
    .trim();

export function toMarkdown(table: PlainTable): string {
  const numeric = numericColumns(table);
  const line = (cells: string[]) => `| ${cells.map(mdCell).join(" | ")} |`;
  const divider = `|${table.header.map((_, i) => (numeric[i] ? " ---: " : " --- ")).join("|")}|`;
  return [line(table.header), divider, ...table.rows.map(line)].join("\n");
}

const csvCell = (text: string) => (/[",\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text);

export function toCsv(table: PlainTable): string {
  return [table.header, ...table.rows]
    .map((row) => row.map((cell) => csvCell(cell.trim())).join(","))
    .join("\r\n");
}

/** The text of a rendered table: the first row of its head, then every body row. */
export function readTable(table: HTMLTableElement): PlainTable {
  const text = (row: HTMLTableRowElement) => Array.from(row.cells, (cell) => cell.textContent ?? "");
  const head = table.tHead?.rows[0];
  const body = Array.from(table.tBodies).flatMap((tbody) => Array.from(tbody.rows));
  return { header: head ? text(head) : [], rows: body.map(text) };
}
