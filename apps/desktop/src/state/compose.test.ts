import { describe, expect, test } from "bun:test";
import {
  commandSuggestions,
  composedText,
  emptyExtras,
  hasTokens,
  insertCommand,
  messageCollapses,
  promptBlocks,
  sentAttachments,
  sentMessages,
  slashToken,
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

  test("a skill is words on a harness that did not advertise it, and a command there", () => {
    const skill = { ...command, name: "/release-automation", action: "skill" };
    const options = { images: false, cwd: "/p" };
    const sent = (harness: boolean, text: string, extras = { ...emptyExtras, command: skill }) =>
      promptBlocks(text, extras, { ...options, builtin: harness });

    // The built-in harness reads `/name` and routes it to `use_skill`.
    expect(sent(true, "cut a beta")).toEqual([{ type: "text", text: "/release-automation cut a beta" }]);
    expect(sent(true, "")).toEqual([{ type: "text", text: "/release-automation" }]);
    // Another harness may reject a leading slash command it does not know: it is told in words.
    expect(sent(false, "cut a beta")).toEqual([
      { type: "text", text: "Use the release-automation skill. cut a beta" },
    ]);
    expect(sent(false, "")).toEqual([{ type: "text", text: "Use the release-automation skill." }]);
    expect(sent(false, "", { ...emptyExtras, command: skill, links: ["https://a.dev"] })).toEqual([
      { type: "text", text: "Use the release-automation skill. https://a.dev" },
    ]);
    // A command the harness itself advertised goes out as typed, on either harness.
    expect(sent(false, "the diff", { ...emptyExtras, command })).toEqual([
      { type: "text", text: "/review the diff" },
    ]);
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

  test("a long paste is inlined as a fence of its own, after the text", () => {
    const extras = {
      ...emptyExtras,
      attachments: [{ id: "1", kind: "text" as const, text: "error: it broke\n  at main.rs:1" }],
    };
    expect(promptBlocks("why does this fail?", extras, { images: true, builtin: true, cwd: "" })).toEqual([
      { type: "text", text: "why does this fail?\n\n```\nerror: it broke\n  at main.rs:1\n```" },
    ]);
  });

  test("a paste holding a fence closes it with a longer one", () => {
    const extras = {
      ...emptyExtras,
      attachments: [{ id: "1", kind: "text" as const, text: "```\ncode\n```" }],
    };
    expect(promptBlocks("", extras, { images: true, builtin: true, cwd: "" })).toEqual([
      { type: "text", text: "````\n```\ncode\n```\n````" },
    ]);
  });

  test("a sent paste keeps its chip and its text", () => {
    const text = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");
    expect(sentAttachments({ ...emptyExtras, attachments: [{ id: "1", kind: "text", text }] })).toEqual([
      { kind: "text", name: "Pasted text · 20 lines", text },
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

describe("the slash token", () => {
  test("opens at the start or after whitespace, at any line", () => {
    expect(slashToken("/", 1)).toEqual({ start: 0, query: "" });
    expect(slashToken("/rev", 4)).toEqual({ start: 0, query: "rev" });
    expect(slashToken("please /rev", 11)).toEqual({ start: 7, query: "rev" });
    expect(slashToken("one\ntwo /rel", 12)).toEqual({ start: 8, query: "rel" });
  });

  test("a path, an email-like word or a finished token is not one", () => {
    expect(slashToken("/usr/bin", 8)).toBeNull();
    expect(slashToken("and/or", 7)).toBeNull();
    expect(slashToken("/rev fix it", 11)).toBeNull();
    expect(slashToken("no slash here", 13)).toBeNull();
  });

  test("the caret inside a word opens nothing", () => {
    expect(slashToken("/review", 3)).toBeNull();
  });
});

describe("command suggestions", () => {
  const commands = [
    {
      name: "/review",
      description: "Review the diff",
      takesArguments: false,
      report: false,
      action: "execute",
    },
    {
      name: "/release-automation",
      description: "Cut a release",
      takesArguments: true,
      report: false,
      action: "skill",
    },
    { name: "/status", description: "Show status", takesArguments: false, report: true, action: "execute" },
  ];

  test("names that start with what is typed come first, then the ones that contain it", () => {
    expect(commandSuggestions("re", commands).map((c) => c.name)).toEqual(["/review", "/release-automation"]);
    expect(commandSuggestions("status", commands).map((c) => c.name)).toEqual(["/status"]);
    expect(commandSuggestions("", commands).map((c) => c.name)).toEqual([
      "/review",
      "/release-automation",
      "/status",
    ]);
  });

  test("it keeps to a bound and drops what does not match", () => {
    expect(commandSuggestions("zzz", commands)).toEqual([]);
    expect(commandSuggestions("", commands, 2)).toHaveLength(2);
  });
});

describe("inserting a command token", () => {
  test("replaces the token and leaves a space to keep typing", () => {
    expect(insertCommand("please /rel now", { start: 7, query: "rel" }, "/release-automation")).toEqual({
      text: "please /release-automation now",
      caret: 27,
    });
  });

  test("at the start it becomes the leading command", () => {
    expect(insertCommand("/rev", { start: 0, query: "rev" }, "/review").text).toBe("/review ");
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
