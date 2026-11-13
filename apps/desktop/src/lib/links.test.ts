import { describe, expect, test } from "bun:test";
import type { Transport } from "../acp/transport";
import { hostOf, linkChipLabel, sanitizeMeta } from "./links";

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
