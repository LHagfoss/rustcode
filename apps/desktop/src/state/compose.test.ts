import { describe, expect, test } from "bun:test";
import {
  composedText,
  emptyExtras,
  hasTokens,
  messageCollapses,
  promptBlocks,
  sentMessages,
  splitImageMarkers,
  splitSlashCommand,
  splitTokens,
  splitUrls,
} from "./compose";

const command = { name: "/review", description: "", takesArguments: true, report: false, action: "execute" };

describe("compose", () => {
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

describe("tokens in the draft", () => {
  test("a file reference reads as a token, its `@` outside the pill", () => {
    expect(splitTokens("see @src/main.ts please")).toEqual([
      { text: "see " },
      { ref: "src/main.ts" },
      { text: " please" },
    ]);
    // A path with a space is written quoted, and the quotes stay in the pill.
    expect(splitTokens('@"a b.ts"')).toEqual([{ ref: '"a b.ts"' }]);
  });

  test("a URL and a reference split apart, in the order they were typed", () => {
    expect(splitTokens("look @src/a.ts then https://x.dev/b ok")).toEqual([
      { text: "look " },
      { ref: "src/a.ts" },
      { text: " then " },
      { url: "https://x.dev/b" },
      { text: " ok" },
    ]);
  });

  test("an email address is not a reference", () => {
    expect(splitTokens("mail a@b.com")).toEqual([{ text: "mail a@b.com" }]);
    expect(hasTokens("mail a@b.com")).toBe(false);
  });

  test("a draft with nothing to draw has no tokens", () => {
    expect(hasTokens("plain words")).toBe(false);
    expect(hasTokens("@src/a.ts")).toBe(true);
    expect(hasTokens("https://x.dev")).toBe(true);
    // Two calls in a row on the same draft: the mirrors' patterns carry no `lastIndex` state.
    expect(hasTokens("@src/a.ts")).toBe(true);
  });
});

describe("a sent message", () => {
  test("a leading command splits off; a path or a later slash does not", () => {
    expect(splitSlashCommand("/review the diff")).toEqual({ command: "/review", rest: "the diff" });
    expect(splitSlashCommand("/compact")).toEqual({ command: "/compact", rest: "" });
    expect(splitSlashCommand("/plugin:cmd\nnext")).toEqual({ command: "/plugin:cmd", rest: "next" });
    expect(splitSlashCommand("/usr/bin/env is missing")).toBeNull();
    expect(splitSlashCommand("run /review")).toBeNull();
  });

  test("it folds by its drawn height, and never to hide a line or two", () => {
    expect(messageCollapses(176)).toBe(false);
    expect(messageCollapses(176 + 44)).toBe(false);
    expect(messageCollapses(176 + 67)).toBe(true);
  });

  test("↑ walks the user's own messages, oldest first", () => {
    expect(
      sentMessages([
        { kind: "user", text: "first" },
        { kind: "assistant", text: "reply" },
        { kind: "tool" },
        { kind: "user", text: "second" },
        // The app's own wake, a message another thread delivered, and an attachments-only one.
        { kind: "user", text: "A child finished", wake: true },
        { kind: "user", text: "From thread “Parent” (agent):\n\nDo this" },
        { kind: "user", text: "   " },
      ]),
    ).toEqual(["first", "second"]);
  });
});
