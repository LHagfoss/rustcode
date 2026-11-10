// What a tool call printed, as the transcript shows it. Some harnesses wrap a
// tool result in a Markdown fence of its own (Claude Code's adapter sends a
// background command's answer as a ```console block), which is a code block in
// the harness's chat, not text of the output: drawn raw, the backticks and the
// language name would read as the command's own words.

/** A fence that wraps the whole text: an opening line with any info string, and one closing run. */
const WRAPPED = /^\s*(`{3,}|~{3,})[^\n]*\r?\n([\s\S]*?)\r?\n?(?:`{3,}|~{3,})[ \t]*\r?\n?\s*$/;

/**
 * The output a fenced tool result carries, without the fence around it. Text
 * that only holds a fence inside it, or none at all, comes back unchanged:
 * a file's Markdown printed by `cat` is the output, fences and all.
 */
export function stripOutputFence(text: string): string {
  const match = WRAPPED.exec(text);
  return match ? (match[2] ?? "") : text;
}
