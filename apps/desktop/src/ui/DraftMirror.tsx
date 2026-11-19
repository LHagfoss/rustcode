import { type Mention, splitTokens } from "../state/compose";

// Keep every source character and the textarea's font metrics. These styles only change paint.
const styles = {
  heading: "[-webkit-text-stroke:0.35px]",
  marker: "text-fg-muted",
  strong: "[-webkit-text-stroke:0.35px]",
  emphasis: "underline decoration-fg-faint underline-offset-2",
  strike: "line-through",
  code: "rounded-xs bg-fill-strong [box-decoration-break:clone]",
} as const;

const markdown = new RegExp(
  [
    /(?<code>^ {0,3}(?<fence>`{3,}|~{3,})[^\n]*(?:\n[\s\S]*?(?:^ {0,3}\k<fence>[ \t]*$|(?![\s\S]))))/.source,
    /(?<heading>^ {0,3}#{1,6}(?:[ \t]+|$)[^\n]*)/.source,
    /(?<marker>^ {0,3}(?:[-+*>]|\d{1,9}[.)])(?=[ \t]))/.source,
    /(?<!\\)(?<inlineCode>`+[^`\n]+`+)/.source,
    /(?<!\\)(?<strong>\*\*(?=\S)[^*\n]+(?<=\S)\*\*|__(?=\S)[^_\n]+(?<=\S)__)/.source,
    /(?<!\\)(?<strike>~~(?=\S)[^~\n]+(?<=\S)~~)/.source,
    /(?<!\\)(?<emphasis>\*(?=\S)[^*\n]+(?<=\S)\*|(?<!\w)_(?=\S)[^_\n]+(?<=\S)_(?!\w))/.source,
  ].join("|"),
  "gm",
);

export function draftMirror(text: string, mentions?: readonly Mention[]) {
  const tokens = splitTokens(text, mentions);
  const matches = [...text.matchAll(markdown)];
  if (!matches.length && tokens.every((part) => part.text !== undefined)) return null;

  let offset = 0;
  let index = 0;
  return tokens.flatMap((part) => {
    const value = part.url ?? part.mention ?? (part.ref === undefined ? (part.text ?? "") : `@${part.ref}`);
    const start = offset;
    offset += value.length;
    if (part.text === undefined) {
      return (
        <span
          key={start}
          className={
            part.url
              ? "rounded-xs bg-accent/15 text-accent [box-decoration-break:clone]"
              : "rounded-xs bg-fill-strong [box-decoration-break:clone]"
          }
        >
          {value}
        </span>
      );
    }
    const runs = [];
    let cursor = start;
    while (cursor < offset && index < matches.length) {
      const match = matches[index];
      if (!match) break;
      const end = match.index + match[0].length;
      if (end <= cursor) {
        index++;
        continue;
      }
      if (match.index >= offset) break;
      const from = Math.max(cursor, match.index);
      const to = Math.min(offset, end);
      runs.push(text.slice(cursor, from));
      const kind = Object.keys(styles).find((name) => match.groups?.[name]) as
        | keyof typeof styles
        | undefined;
      runs.push(
        <span key={from} className={styles[kind ?? "code"]}>
          {text.slice(from, to)}
        </span>,
      );
      cursor = to;
    }
    runs.push(text.slice(cursor, offset));
    return runs;
  });
}
