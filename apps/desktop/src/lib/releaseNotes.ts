// A release's notes, read into what What's New draws (ui/Updates.tsx).
//
// Two shapes arrive here. The repo's CHANGELOG.md, which the shell prefers
// (src-tauri/src/updater.rs), is already curated: `### Features` and then
// `- a note (#N)` lines, written by scripts/release.sh. GitHub's generated
// release body says the same things in the wire format of a pull request list:
// an HTML comment, a `## What's Changed` heading, `* a title by @user in
// <pull url>` entries, the `chore(ci): release vX` commit that made the tag,
// and a `**Full Changelog**` line. Both are read into groups here, so the
// dialog can draw each category as its own small badge and drop what is not a
// change the user can see.

/** One version's notes: its categories, and where the diff between versions is. */
export interface ReleaseNotes {
  /** The categories in the order the notes list them; a note before any heading has `""`. */
  groups: ReleaseGroup[];
  /** The compare link GitHub ends a generated body with, when it names two versions. */
  compare: { label: string; url: string } | null;
}

export interface ReleaseGroup {
  /** The category heading as written (`Features`, `Fixes`), `""` for a lead paragraph. */
  heading: string;
  /** Each note's Markdown, with GitHub's `by @user in …` tail turned into a pull request link. */
  items: string[];
}

const HEADING = /^\s{0,3}#{1,6}\s+(.*)$/;
const BULLET = /^\s*[-*+]\s+(.*)$/;
/** `**Full Changelog**: <url>`, with or without the bold, colon and bullet. */
const FULL_CHANGELOG = /^\**full[ -]?changelog\**:?\s*(\S+?)\s*$/i;
/** The tail GitHub appends to every generated entry. */
const PR_TAIL = /\s+by\s+@[A-Za-z0-9-]+(?:\s+in\s+(https?:\/\/\S+\/pull\/(\d+)))?$/i;
/** A trailing `(#N)` in a curated note, which is a pull request the same way. */
const PR_NUMBER = /\s\(#(\d+)\)$/;
/** The release commit itself: it is how the version was cut, not a change. */
const RELEASE_ENTRY = /^chore\(ci\):\s*release\b/i;
const VERSION = /^v?\d+(\.\d+){1,3}(-[0-9A-Za-z.]+)?$/;

/**
 * The notes as groups of Markdown notes. `repo` (`owner/name`) is what turns a
 * curated note's `(#N)` into the same link GitHub's body carries.
 */
export function parseReleaseNotes(raw: string, repo?: string): ReleaseNotes {
  // The generator's comment, and any other HTML comment, is not for a person.
  const text = raw.replace(/<!--[\s\S]*?-->/g, "");

  const groups: ReleaseGroup[] = [];
  let heading = "";
  let items: string[] = [];
  let paragraph: string[] = [];
  let compare: ReleaseNotes["compare"] = null;

  const closeGroup = () => {
    if (items.length) groups.push({ heading, items });
    items = [];
  };
  const addItem = (raw: string) => {
    const item = cleanItem(raw, repo);
    if (item) items.push(item);
  };
  const flushParagraph = () => {
    if (paragraph.length) addItem(paragraph.join(" "));
    paragraph = [];
  };

  for (const line of text.split("\n")) {
    const bare = line.trim();
    if (!bare) {
      flushParagraph();
      continue;
    }
    const bullet = BULLET.exec(line);
    const body = (bullet ? bullet[1]! : bare).trim();

    // `**Full Changelog**: …` closes the list; the dialog draws it under the notes.
    const changelog = FULL_CHANGELOG.exec(body);
    if (changelog?.[1]) {
      flushParagraph();
      compare = compareLink(changelog[1]);
      continue;
    }
    // A bullet is a note whatever it says, including one that looks like a heading.
    if (bullet) {
      flushParagraph();
      addItem(body);
      continue;
    }
    const title = HEADING.exec(line);
    if (title?.[1]) {
      flushParagraph();
      const name = title[1].trim();
      // GitHub's own heading over the category headings: it heads nothing here.
      if (/^what'?s changed\b/i.test(name)) continue;
      closeGroup();
      heading = name;
      continue;
    }
    paragraph.push(bare);
  }
  flushParagraph();
  closeGroup();

  return { groups, compare };
}

/** One note, with the pull request its tail names. `null` for what is not a note. */
function cleanItem(raw: string, repo?: string): string | null {
  let item = raw
    .replace(PR_TAIL, (_all, url?: string, number?: string) =>
      // `(#N)` is the whole of what that tail said, in the width of a word.
      url && number ? ` ([#${number}](${url}))` : "",
    )
    .trim();
  if (!item || RELEASE_ENTRY.test(item)) return null;
  if (repo) item = item.replace(PR_NUMBER, (_all, number: string) => prLink(number, repo));
  return item;
}

function prLink(number: string, repo: string): string {
  return ` ([#${number}](https://github.com/${repo}/pull/${number}))`;
}

/** `Compare v1.20.0…v1.20.1`, or the whole changelog when the range is not two versions. */
function compareLink(url: string): { label: string; url: string } {
  const range = /\/compare\/(.+)\.\.\.(.+)$/.exec(url);
  const from = range?.[1];
  const to = range?.[2];
  if (from && to && VERSION.test(from) && VERSION.test(to)) return { label: `Compare ${from}…${to}`, url };
  return { label: "Full changelog", url };
}
