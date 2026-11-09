import { describe, expect, test } from "bun:test";
import { Tooltip } from "radix-ui";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { LinkChip } from "./LinkChip";

/** A chip with its tooltip's provider, as the app draws it. */
const drawn = (url: string, variant?: "inline" | "token") =>
  renderToStaticMarkup(createElement(Tooltip.Provider, null, createElement(LinkChip, { url, variant })));

describe("a link chip", () => {
  test("in text it sits on the x-height and adds nothing to the line box", () => {
    const out = drawn("https://example.com/a");
    // 18px centred on the x-height stays inside the reply's 22.75px line; a taller chip, or one
    // aligned on a flex baseline, pushes that line apart (ui/LinkChip.tsx).
    expect(out).toContain("h-[18px]");
    expect(out).toContain("align-middle");
  });

  test("in the composer's row it takes the field's own line box", () => {
    const out = drawn("https://example.com/a", "token");
    expect(out).toContain("h-[22.75px]");
    expect(out).not.toContain("align-middle");
  });

  test("in text it is the pill the code and the links beside it are", () => {
    const out = drawn("https://example.com/a");
    expect(out).toContain("rounded-full");
    // A pixel more room on each side than the chip had.
    expect(out).toContain("pl-[7px]");
    expect(out).toContain("pr-[5px]");
  });

  test("in the composer it keeps the field tokens' own shape", () => {
    const out = drawn("https://example.com/a", "token");
    expect(out).toContain("rounded-sm");
    expect(out).not.toContain("rounded-full");
  });

  test("it is no drag source, so a drag across it selects the words", () => {
    expect(drawn("https://example.com/a")).toContain('draggable="false"');
    expect(drawn("https://example.com/a", "token")).toContain('draggable="false"');
  });

  test("a link with no favicon shows a globe, not an empty gap", () => {
    // The shell has no metadata in a test, and a favicon that will not decode takes the same path
    // (ui/LinkChip.tsx): the icon follows the text's colour rather than a site's own image.
    const out = drawn("https://example.com/a");
    expect(out).toContain("lucide-globe");
    expect(out).not.toContain("<img");
  });

  test("it says the host until the shell has a title", () => {
    const out = drawn("https://www.example.com/a");
    // The words, not the URL: a selection across the chip copies "example.com".
    expect(out).toContain('<span class="font-medium">example.com</span>');
  });
});
