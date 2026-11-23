import { useLayoutEffect, useRef, useState } from "react";
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

function HeadingPreview({ source, label, level }: { source: string; label: string; level: number }) {
  const raw = useRef<HTMLSpanElement>(null);
  const preview = useRef<HTMLSpanElement>(null);
  const [fits, setFits] = useState(false);

  useLayoutEffect(() => {
    const line = raw.current;
    const title = preview.current;
    const mirror = line?.closest(".rc-field-fade");
    if (!line || !title || !mirror) return;
    const measure = () => {
      setFits(line.getClientRects().length === 1 && title.offsetWidth <= mirror.clientWidth);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(mirror);
    observer.observe(title);
    return () => observer.disconnect();
  }, []);

  return (
    <span className="relative">
      <span ref={raw} className={fits ? "invisible" : styles.heading}>
        {source}
      </span>
      <span
        ref={preview}
        className="absolute top-1/2 left-0 -translate-y-1/2 whitespace-pre font-semibold leading-none"
        style={{
          fontSize: level === 1 ? "1.35em" : level === 2 ? "1.2em" : "1.1em",
          visibility: fits ? "visible" : "hidden",
        }}
      >
        {label}
      </span>
    </span>
  );
}

export function listContinuation(text: string, start: number, end: number) {
  if (start !== end || (start < text.length && text[start] !== "\n")) return null;
  const lineStart = text.lastIndexOf("\n", start - 1) + 1;
  const line = text.slice(lineStart, start);
  const item = /^([ \t]*)(?:(\d{1,9})([.)])|([-+*]))[ \t]+(.*)$/.exec(line);
  if (!item) return null;
  for (const block of text.matchAll(markdown)) {
    if (block.index >= lineStart) break;
    if (block.groups?.code && block.index + block[0].length > lineStart) return null;
  }
  const [, indent, number, delimiter, bullet, body] = item;
  if (!body?.trim()) return { start: lineStart, text: "" };
  const next = number ? String(Number(number) + 1).padStart(number.length, "0") : "";
  if (next.length > 9) return null;
  return { start, text: `\n${indent}${bullet ?? `${next}${delimiter}`} ` };
}

export function draftMirror(
  text: string,
  mentions?: readonly Mention[],
  selection?: { start: number; end: number },
) {
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
      const heading =
        kind === "heading" &&
        from === match.index &&
        to === end &&
        (!selection || selection.end < from || selection.start > to)
          ? /^ {0,3}(#{1,6})[ \t]+(.+)$/.exec(match[0])
          : null;
      if (heading?.[1] && heading[2]?.trim()) {
        const label = heading[2].replace(/[ \t]+#+[ \t]*$/, "").trim();
        if (label) {
          runs.push(<HeadingPreview key={from} source={match[0]} label={label} level={heading[1].length} />);
          cursor = to;
          continue;
        }
      }
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
