import { describe, expect, test } from "bun:test";
import { comparisonBase, rowBranch, shortRef } from "./gitRef";

describe("shortRef", () => {
  test("a branch name stays as it is", () => {
    expect(shortRef("main")).toBe("main");
    expect(shortRef("v1.2.3")).toBe("v1.2.3");
    expect(shortRef(" HEAD ")).toBe("HEAD");
  });

  test("a local branch keeps its slash; only a remote is dropped", () => {
    expect(shortRef("feat/login-form")).toBe("feat/login-form");
    expect(shortRef("feature/nested/slug")).toBe("feature/nested/slug");
  });

  test("a remote-tracking branch loses the remote", () => {
    expect(shortRef("origin/main")).toBe("main");
    expect(shortRef("upstream/main")).toBe("main");
    expect(shortRef("origin/feat/login-form")).toBe("feat/login-form");
  });

  test("refs/ prefixes are dropped", () => {
    expect(shortRef("refs/heads/main")).toBe("main");
    expect(shortRef("refs/remotes/origin/main")).toBe("main");
    expect(shortRef("refs/remotes/origin/feat/x")).toBe("feat/x");
    expect(shortRef("refs/tags/v1.2.3")).toBe("v1.2.3");
  });

  test("a commit sha is abbreviated", () => {
    expect(shortRef("c1ace820123456789012345678901234567890ab")).toBe("c1ace82");
    expect(shortRef("c1ace820")).toBe("c1ace820");
  });

  test("nothing in, nothing out", () => {
    expect(shortRef("")).toBe("");
  });
});

describe("comparisonBase", () => {
  test("the branch it was made from comes first", () => {
    expect(comparisonBase({ baseRef: "origin/main", base: "c1ace820" })).toBe("origin/main");
  });

  test("the commit stands in when the ref was not recorded", () => {
    expect(comparisonBase({ base: "c1ace820" })).toBe("c1ace820");
    expect(comparisonBase({ baseRef: "  ", base: "c1ace820" })).toBe("c1ace820");
  });

  test("no base is unknown, not empty", () => {
    expect(comparisonBase(undefined)).toBeUndefined();
    expect(comparisonBase({})).toBeUndefined();
    expect(comparisonBase({ baseRef: "", base: "" })).toBeUndefined();
  });
});

describe("rowBranch", () => {
  test("drops a Conventional Commits type", () => {
    expect(rowBranch("feat/desktop-smart-ui-snappier-sources")).toBe("desktop-smart-ui-snappier-sources");
    expect(rowBranch("fix/tui-composer-selection")).toBe("tui-composer-selection");
    expect(rowBranch("chore/a/b")).toBe("a/b");
  });
  test("keeps any other branch whole", () => {
    expect(rowBranch("main")).toBe("main");
    expect(rowBranch("lars/spike")).toBe("lars/spike");
    expect(rowBranch("release/1.2")).toBe("release/1.2");
    expect(rowBranch("feat/")).toBe("feat/");
  });
});
