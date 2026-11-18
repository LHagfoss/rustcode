import { describe, expect, test } from "bun:test";
import type { Transport } from "../acp/transport";
import { githubRefOf, githubTitle, hostOf, linkChipLabel, sanitizeMeta } from "./links";

describe("link metadata", () => {
  test("keeps only safe values", () => {
    expect(
      sanitizeMeta({
        title: "  A\n title ",
        icon: "https://evil.example/x.png",
        color: "red; background: url(x)",
      }),
    ).toEqual({ title: "A title", siteName: undefined, icon: undefined, color: undefined });
    expect(sanitizeMeta({ icon: "data:image/png;base64,AAAA", color: "#1e2327" })?.color).toBe("#1e2327");
    expect(sanitizeMeta(null)).toBeNull();
    expect(sanitizeMeta({})).toBeNull();
  });
});

describe("a GitHub issue or pull request link", () => {
  const pr = "https://github.com/acme/app/pull/3975";
  test("is recognised with anything after its number", () => {
    expect(githubRefOf(pr)).toEqual({ repo: "acme/app", kind: "pull", number: 3975 });
    expect(githubRefOf("https://github.com/acme/app/issues/12#issuecomment-1")).toEqual({
      repo: "acme/app",
      kind: "issue",
      number: 12,
    });
    expect(githubRefOf(`${pr}/files`)?.number).toBe(3975);
    expect(githubRefOf("https://github.com/acme/app/pull/12x")).toBeUndefined();
    expect(githubRefOf("https://github.com/acme/app")).toBeUndefined();
    expect(githubRefOf("https://example.com/acme/app/pull/1")).toBeUndefined();
  });

  test("keeps only its own title from GitHub's page title", () => {
    const ref = githubRefOf(pr)!;
    expect(githubTitle("Keep the name by someone · Pull Request #3975 · acme/app · GitHub", ref)).toBe(
      "Keep the name",
    );
    expect(githubTitle("Keep the name by dependabot[bot] · Pull Request #3975 · acme/app", ref)).toBe(
      "Keep the name",
    );
    const issue = githubRefOf("https://github.com/acme/app/issues/12")!;
    expect(githubTitle("Crash on start · Issue #12 · acme/app · GitHub", issue)).toBe("Crash on start");
    // A private repository's page, or another number's, is not this one's title.
    expect(githubTitle("Page not found · GitHub", issue)).toBeUndefined();
    expect(githubTitle("Crash · Issue #13 · acme/app", issue)).toBeUndefined();
  });

  test("reads as its number and title, or owner/repo#number until the title is known", () => {
    expect(linkChipLabel(pr, null)).toMatchObject({ title: "acme/app#3975", showsHost: false });
    expect(
      linkChipLabel(pr, { title: "Keep the name by someone · Pull Request #3975 · acme/app · GitHub" }),
    ).toMatchObject({ tag: "#3975", title: "Keep the name", host: "app", showsHost: true });
  });
});

describe("the words a link chip shows", () => {
  test("the page's title over its host, so a selection copies both", () => {
    expect(linkChipLabel("https://www.example.com/a", { title: "An example" })).toEqual({
      title: "An example",
      host: "example.com",
      showsHost: true,
    });
  });

  test("the host alone without a title, or when the title is the host", () => {
    expect(linkChipLabel("https://www.example.com/a", null)).toEqual({
      title: "example.com",
      host: "example.com",
      showsHost: false,
    });
    expect(linkChipLabel("https://example.com/", { title: " example.com " })).toEqual({
      title: "example.com",
      host: "example.com",
      showsHost: false,
    });
  });

  test("a string that is not a URL is its own label", () => {
    expect(hostOf("not a url")).toBe("not a url");
    expect(linkChipLabel("not a url", null)).toEqual({
      title: "not a url",
      host: "not a url",
      showsHost: false,
    });
  });
});

describe("the window's cache", () => {
  test("keeps at most MAX_CACHED pages, dropping the least recently used", async () => {
    const { clearLinkCacheForTests, linkMetaCached, loadLinkMeta, MAX_CACHED } = await import("./links");
    const { setShellTransport } = await import("./shell");
    const asked: string[] = [];
    setShellTransport({
      call: async (_command: string, args: Record<string, unknown>) => {
        asked.push(String(args.url));
        return { title: `page ${String(args.url)}` };
      },
    } as unknown as Transport);
    clearLinkCacheForTests();
    try {
      const url = (i: number) => `https://example.com/${i}`;
      await loadLinkMeta(url(0));
      for (let i = 1; i < MAX_CACHED; i++) await loadLinkMeta(url(i));
      // The first page is used again, so the next one in is what goes.
      expect(linkMetaCached(url(0))?.title).toBe(`page ${url(0)}`);
      await loadLinkMeta(url(MAX_CACHED));
      expect(linkMetaCached(url(1))).toBeNull();
      expect(linkMetaCached(url(0))).not.toBeNull();
      expect(linkMetaCached(url(MAX_CACHED))).not.toBeNull();
      // A page that went is asked for again; a kept one is not.
      const before = asked.length;
      await loadLinkMeta(url(0));
      expect(asked.length).toBe(before);
      await loadLinkMeta(url(1));
      expect(asked.length).toBe(before + 1);
    } finally {
      setShellTransport(null);
      clearLinkCacheForTests();
    }
  });
});
