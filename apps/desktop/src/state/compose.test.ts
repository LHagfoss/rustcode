import { describe, expect, test } from "bun:test";
import { composedText, emptyExtras, pastedUrl, promptBlocks, splitImageMarkers, splitUrls } from "./compose";

const command = { name: "/review", description: "", takesArguments: true, report: false, action: "execute" };

describe("compose", () => {
  test("a pasted URL is a link only when it is the whole paste", () => {
    expect(pastedUrl(" https://example.com/a?b=1 ")).toBe("https://example.com/a?b=1");
    expect(pastedUrl("see https://example.com")).toBeNull();
    expect(pastedUrl("javascript:alert(1)")).toBeNull();
    expect(pastedUrl("file:///etc/passwd")).toBeNull();
  });

  test("the prompt text keeps the order the composer shows: command, links, text", () => {
    const extras = { ...emptyExtras, command, links: ["https://a.dev"] };
    expect(composedText("look at this", extras)).toBe("/review https://a.dev look at this");
    expect(composedText("", extras)).toBe("/review https://a.dev");
  });

  test("images become image blocks only when the harness takes them", () => {
    const extras = {
      ...emptyExtras,
      attachments: [{ id: "1", kind: "image" as const, name: "a.png", mimeType: "image/png", data: "AAAA" }],
    };
    const on = promptBlocks("hi", extras, { images: true, builtin: true, cwd: "/p" });
    expect(on).toEqual([
      { type: "text", text: "hi" },
      { type: "image", mimeType: "image/png", data: "AAAA" },
    ]);
    expect(promptBlocks("hi", extras, { images: false, builtin: true, cwd: "/p" })).toEqual([
      { type: "text", text: "hi" },
    ]);
    expect(promptBlocks("", extras, { images: true, builtin: true, cwd: "/p" })).toHaveLength(1);
  });

  test("files go as a list on the built-in harness and as resource links elsewhere", () => {
    const extras = {
      ...emptyExtras,
      attachments: [{ id: "1", kind: "file" as const, name: "a b.ts", path: "/p/src/a b.ts" }],
    };
    expect(promptBlocks("fix", extras, { images: true, builtin: true, cwd: "/p" })).toEqual([
      { type: "text", text: "fix\n\nAttached files:\n- `src/a b.ts`" },
    ]);
    expect(promptBlocks("fix", extras, { images: true, builtin: false, cwd: "/p" })).toEqual([
      { type: "text", text: "fix" },
      { type: "resource_link", uri: "file:///p/src/a%20b.ts", name: "a b.ts" },
    ]);
  });

  test("a file without a path is inlined", () => {
    const extras = {
      ...emptyExtras,
      attachments: [{ id: "1", kind: "file" as const, name: "n.txt", text: "x" }],
    };
    expect(promptBlocks("", extras, { images: true, builtin: false, cwd: "" })).toEqual([
      { type: "text", text: "Attached file `n.txt`:\n```\nx\n```" },
    ]);
  });

  test("replayed image markers and URLs in text", () => {
    expect(splitImageMarkers("look\n![image](file:///tmp/a.png)\n")).toEqual({ text: "look", images: 1 });
    expect(splitUrls("see https://a.dev/x. ok")).toEqual([
      { text: "see " },
      { url: "https://a.dev/x" },
      { text: ". ok" },
    ]);
  });
});
