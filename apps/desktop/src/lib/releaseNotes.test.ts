import { describe, expect, test } from "bun:test";
import { parseReleaseNotes } from "./releaseNotes";

/** v1.20.1's release body, exactly as GitHub generated it. */
const GENERATED = `<!-- Release notes generated using configuration in .github/release.yml at v1.20.1 -->

## What's Changed
### Features
* feat(desktop): use the lavender accent by default by @LHagfoss in https://github.com/LHagfoss/rustcode/pull/3192
### Chores
* chore(ci): release v1.20.1 by @LHagfoss in https://github.com/LHagfoss/rustcode/pull/3193


**Full Changelog**: https://github.com/LHagfoss/rustcode/compare/v1.20.0...v1.20.1
`;

/** The same release in CHANGELOG.md, as scripts/release.sh writes it. */
const CURATED = `### Features
- The desktop app uses the lavender accent by default, matching the built-in themes. (#3192)
### Fixes
- The composer keeps its corner when the window narrows. (#3190)
`;

describe("release notes", () => {
  test("a generated release body reads as its categories of changes", () => {
    expect(parseReleaseNotes(GENERATED)).toEqual({
      groups: [
        {
          heading: "Features",
          items: [
            "feat(desktop): use the lavender accent by default ([#3192](https://github.com/LHagfoss/rustcode/pull/3192))",
          ],
        },
      ],
      compare: {
        label: "Compare v1.20.0…v1.20.1",
        url: "https://github.com/LHagfoss/rustcode/compare/v1.20.0...v1.20.1",
      },
    });
  });

  test("the generator's comment, the What's Changed heading and the release commit are gone", () => {
    const { groups } = parseReleaseNotes(GENERATED);
    const written = JSON.stringify(groups);
    expect(written).not.toContain("Release notes generated");
    expect(written).not.toContain("What's Changed");
    expect(written).not.toContain("chore(ci): release");
    // The Chores group held nothing else, so it goes too.
    expect(groups.map((g) => g.heading)).toEqual(["Features"]);
  });

  test("a curated section links the pull request its note names", () => {
    expect(parseReleaseNotes(CURATED, "LHagfoss/rustcode").groups).toEqual([
      {
        heading: "Features",
        items: [
          "The desktop app uses the lavender accent by default, matching the built-in themes. ([#3192](https://github.com/LHagfoss/rustcode/pull/3192))",
        ],
      },
      {
        heading: "Fixes",
        items: [
          "The composer keeps its corner when the window narrows. ([#3190](https://github.com/LHagfoss/rustcode/pull/3190))",
        ],
      },
    ]);
  });

  test("a note with no heading of its own keeps its own group", () => {
    expect(parseReleaseNotes("The updater now remembers the beta you skipped.\n")).toEqual({
      groups: [{ heading: "", items: ["The updater now remembers the beta you skipped."] }],
      compare: null,
    });
  });

  test("a paragraph of several lines is one note", () => {
    expect(parseReleaseNotes("One line\nand its second line.\n\nAnother note.").groups).toEqual([
      { heading: "", items: ["One line and its second line.", "Another note."] },
    ]);
  });

  test("a tail with no pull request, and a compare of two arbitrary refs", () => {
    const notes = parseReleaseNotes(
      "* fix(tui): keep the selection by @someone\n**Full Changelog**: https://github.com/x/y/compare/abc123...def456",
    );
    expect(notes.groups).toEqual([{ heading: "", items: ["fix(tui): keep the selection"] }]);
    expect(notes.compare).toEqual({
      label: "Full changelog",
      url: "https://github.com/x/y/compare/abc123...def456",
    });
  });

  test("nothing at all is no notes", () => {
    expect(parseReleaseNotes("<!-- only a comment -->\n\n")).toEqual({ groups: [], compare: null });
  });
});
