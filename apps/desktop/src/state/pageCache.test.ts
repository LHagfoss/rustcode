import { beforeEach, describe, expect, test } from "bun:test";
import {
  beginPageRead,
  cachedPage,
  clearPageCache,
  commitPage,
  forgetPage,
  isLatestPageRead,
  PAGE_FRESH_MS,
  pageIsFresh,
  pageKey,
  rememberPage,
  sameValue,
} from "./pageCache";

beforeEach(() => clearPageCache());

describe("page cache keys", () => {
  test("the method alone with no params, and params order does not matter", () => {
    expect(pageKey("_rustcode/capabilities")).toBe("_rustcode/capabilities");
    expect(pageKey("_rustcode/capabilities", {})).toBe("_rustcode/capabilities");
    expect(pageKey("_rustcode/x", { b: 1, a: 2 })).toBe(pageKey("_rustcode/x", { a: 2, b: 1 }));
    expect(pageKey("_rustcode/x", { a: 1 })).not.toBe(pageKey("_rustcode/x", { a: 2 }));
  });
});

describe("freshness", () => {
  test("under 30 seconds old is fresh, and a missing answer never is", () => {
    const now = 1_000_000;
    const token = beginPageRead("_rustcode/x");
    commitPage("_rustcode/x", token, { n: 1 }, now);
    expect(pageIsFresh(cachedPage("_rustcode/x"), now + PAGE_FRESH_MS - 1)).toBe(true);
    expect(pageIsFresh(cachedPage("_rustcode/x"), now + PAGE_FRESH_MS)).toBe(false);
    expect(pageIsFresh(cachedPage("_rustcode/missing"), now)).toBe(false);
  });
});

describe("answer ordering", () => {
  test("an answer to a read that started before a newer one is dropped", () => {
    const older = beginPageRead("_rustcode/x");
    const newer = beginPageRead("_rustcode/x");
    expect(commitPage("_rustcode/x", newer, { n: 2 }, 20)).toEqual({ data: { n: 2 }, readAt: 20 });
    expect(commitPage("_rustcode/x", older, { n: 1 }, 10)).toBeNull();
    expect(cachedPage("_rustcode/x")).toEqual({ data: { n: 2 }, readAt: 20 });
    expect(isLatestPageRead("_rustcode/x", newer)).toBe(true);
    expect(isLatestPageRead("_rustcode/x", older)).toBe(false);
  });

  test("an action's write outranks a read already in flight", () => {
    const read = beginPageRead("_rustcode/x");
    rememberPage("_rustcode/x", { n: 2 }, 20);
    expect(commitPage("_rustcode/x", read, { n: 1 }, 10)).toBeNull();
    expect(cachedPage("_rustcode/x")?.data).toEqual({ n: 2 });
  });

  test("a new engine drops the answers and ignores reads of the old one", () => {
    const read = beginPageRead("_rustcode/x");
    clearPageCache();
    expect(commitPage("_rustcode/x", read, { n: 1 })).toBeNull();
    expect(cachedPage("_rustcode/x")).toBeUndefined();
  });

  test("forgetting one ask leaves the others", () => {
    rememberPage("_rustcode/a", { n: 1 });
    rememberPage("_rustcode/b", { n: 2 });
    forgetPage("_rustcode/a");
    expect(cachedPage("_rustcode/a")).toBeUndefined();
    expect(cachedPage("_rustcode/b")?.data).toEqual({ n: 2 });
  });
});

describe("comparing answers", () => {
  test("same values keep the page's state; different ones do not", () => {
    expect(
      sameValue(
        { plans: [{ id: "claude", windows: [1, 2] }] },
        { plans: [{ id: "claude", windows: [1, 2] }] },
      ),
    ).toBe(true);
    expect(sameValue({ n: 1 }, { n: 2 })).toBe(false);
    expect(sameValue(null, { n: 1 })).toBe(false);
    const same = { n: 1 };
    expect(sameValue(same, same)).toBe(true);
  });
});
