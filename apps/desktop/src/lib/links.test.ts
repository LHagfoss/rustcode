import { describe, expect, test } from "bun:test";
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
