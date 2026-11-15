import { describe, expect, test } from "bun:test";
import { Tooltip } from "radix-ui";
import { renderToStaticMarkup } from "react-dom/server";
import type { PullRequest } from "../acp/rustcode";
import { MergeButton } from "./PullRequests";

const PULL: PullRequest = {
  number: 2781,
  title: "fix(engine): keep the steer queue across a restart",
  author: "LHagfoss",
  state: "open",
  draft: false,
  checks: { passed: 6, failed: 0, pending: 0, skipped: 0 },
  headRef: "fix/engine-steer-restart",
  baseRef: "main",
  headSha: "a1b2c3d4",
  updatedAt: 0,
  additions: 40,
  deletions: 9,
  changedFiles: 3,
} as PullRequest;

const run = (name: string, status: string) => ({ name, status });

function render(pull: Partial<PullRequest>, runs?: { name: string; status: string }[]): string {
  return renderToStaticMarkup(
    <Tooltip.Provider>
      <MergeButton cwd="/repo" pull={{ ...PULL, ...pull }} runs={runs} />
    </Tooltip.Provider>,
  );
}

// The attribute, not the `disabled:` styling every button's classes carry.
const enabled = (html: string) => html.includes("<button") && !/<button[^>]* disabled=""/.test(html);

describe("the merge button", () => {
  test("merges once every check passed", () => {
    const html = render({});
    expect(html).toContain("Merge");
    expect(html).not.toContain("Merge when checks pass");
    expect(enabled(html)).toBe(true);
  });

  test("waits for running checks", () => {
    const html = render({ checks: { passed: 4, failed: 0, pending: 2, skipped: 0 } });
    expect(html).toContain("Merge when checks pass");
    expect(enabled(html)).toBe(true);
  });

  test("is disabled by a failing check, without repeating the count the Checks headline has", () => {
    const html = render({}, [run("lint", "passed"), run("Desktop UI", "failed")]);
    expect(html).toContain("Merge");
    expect(html).not.toContain("failed");
    expect(enabled(html)).toBe(false);
  });

  test("does not let an advisory job block it, as the engine's merge does not", () => {
    const runs = [run("lint", "passed"), run("Windows and macOS portability coverage", "failed")];
    // The list counted the advisory failure; the detail's runs say what it was.
    const html = render({ checks: { passed: 1, failed: 1, pending: 0, skipped: 0 } }, runs);
    expect(enabled(html)).toBe(true);
    expect(html).not.toContain("Merge when checks pass");
  });

  test("offers nothing for a draft or a finished pull", () => {
    expect(render({ draft: true })).toBe("");
    expect(render({ state: "merged" })).toBe("");
    expect(render({ state: "closed" })).toBe("");
  });
});
