// What the composer's drawer rows share (ui/Composer.tsx, ui/machineMentions.tsx): the name's
// column, and the letters of it that what is typed matched.

import { Fragment } from "react";
import { matchSpans } from "../lib/folderList";

export const DRAWER_NAME = "min-w-[96px] max-w-[45%] shrink-0 truncate font-medium text-fg text-sub";

/** `text` with the letters the query matched marked, as the folder picker marks a folder's name. */
export function Marked({ text, query }: { text: string; query: string }) {
  return (
    <>
      {matchSpans(text, query).map((span, i) =>
        span.hit ? (
          <mark key={i} className="rounded-xs bg-fill-strong text-fg">
            {span.text}
          </mark>
        ) : (
          <Fragment key={i}>{span.text}</Fragment>
        ),
      )}
    </>
  );
}
