// Syntax colours for a diff's lines: the Review pane and the transcript's diffs read them the same way.

import { useEffect, useState } from "react";
import { highlight, langForPath } from "./highlight";

/** Shiki HTML for each line of `lines`, in order (null while it loads or for an unknown language). */
export function useHighlightedLines(lines: string[], path: string): (string | null)[] {
  const [html, setHtml] = useState<(string | null)[]>([]);
  const code = lines.join("\n");
  useEffect(() => {
    let live = true;
    const lang = langForPath(path);
    if (!lang || code.length > 200_000) {
      setHtml([]);
      return;
    }
    void highlight(code, lang).then((out) => {
      if (!live || !out) return;
      const doc = new DOMParser().parseFromString(out, "text/html");
      setHtml([...doc.querySelectorAll(".line")].map((el) => el.innerHTML));
    });
    return () => {
      live = false;
    };
  }, [code, path]);
  return html;
}
