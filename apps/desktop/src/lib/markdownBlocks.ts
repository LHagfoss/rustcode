// Splits Markdown into top-level blocks that render the same one at a time as
// together, so a streamed reply re-parses only its last block per chunk.
// Splits only at a blank line outside a code fence, and never in front of a
// line that could continue the block above (indented, a list item, a table
// row, a footnote or link definition), so lists, loose lists, indented code
// and tables stay whole.

const FENCE = /^ {0,3}(`{3,}|~{3,})/;
const CONTINUES = /^(\s|[-*+] |\d{1,9}[.)] |\||\[[^\]]*\]:|\[\^)/;

export function markdownBlocks(text: string): string[] {
  const lines = text.split("\n");
  const blocks: string[] = [];
  let current: string[] = [];
  let fence: string | null = null;
  let blank = false;
  // A definition anywhere ([x]: url, [^1]: note) can be used by any block: keep the reply whole.
  if (/^ {0,3}\[[^\]]+\]:/m.test(text)) return [text];
  for (const line of lines) {
    if (fence) {
      current.push(line);
      const close = FENCE.exec(line);
      if (
        close &&
        close[1]![0] === fence[0] &&
        close[1]!.length >= fence.length &&
        !line.trim().slice(close[0].trim().length).trim()
      )
        fence = null;
      continue;
    }
    if (!line.trim()) {
      blank = true;
      current.push(line);
      continue;
    }
    if (blank && current.some((l) => l.trim()) && !CONTINUES.test(line)) {
      // Drop the blank lines between blocks; each block renders on its own.
      while (current.length && !current[current.length - 1]!.trim()) current.pop();
      blocks.push(current.join("\n"));
      current = [];
    }
    blank = false;
    current.push(line);
    const open = FENCE.exec(line);
    if (open) fence = open[1]!;
  }
  if (current.length) blocks.push(current.join("\n"));
  return blocks.length ? blocks : [""];
}
