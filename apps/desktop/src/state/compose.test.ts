import { describe, expect, test } from "bun:test";
import {
  composedText,
  emptyExtras,
  hasTokens,
  messageCollapses,
  promptBlocks,
  sentAttachments,
  sentMessages,
  shiftPasteOffset,
  splicePastes,
  splitImageMarkers,
  splitSlashCommand,
  splitTokens,
  splitUrls,
  trackPastes,
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

  test("a long paste is in the text where it was pasted, as plain text", () => {
    const extras = {
      ...emptyExtras,
      attachments: [{ id: "1", kind: "text" as const, text: "error: it broke\n  at main.rs:1", at: 19 }],
    };
    expect(promptBlocks("why does this fail?", extras, { images: true, builtin: true, cwd: "" })).toEqual([
      { type: "text", text: "why does this fail?\nerror: it broke\n  at main.rs:1" },
    ]);
  });

  test("a paste is never fenced, even one holding a fence", () => {
    const extras = {
      ...emptyExtras,
      attachments: [{ id: "1", kind: "text" as const, text: "```\ncode\n```", at: 0 }],
    };
    expect(promptBlocks("", extras, { images: true, builtin: true, cwd: "" })).toEqual([
      { type: "text", text: "```\ncode\n```" },
    ]);
  });

  test("a sent paste is in the text, so it keeps no chip of its own", () => {
    const text = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");
    const extras = { ...emptyExtras, attachments: [{ id: "1", kind: "text" as const, text, at: 4 }] };
    expect(sentAttachments(extras)).toEqual([]);
    expect(composedText("why?", extras)).toBe(`why?\n${text}`);
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

describe("a long paste's offset in the draft", () => {
  test("an edit before it moves it, an edit after it leaves it alone", () => {
    // "hello| world", the paste after "hello".
    expect(shiftPasteOffset(5, "hello world", "well hello world")).toBe(10);
    expect(shiftPasteOffset(5, "hello world", "hello world again")).toBe(5);
    expect(shiftPasteOffset(5, "hello world", "hellXo world")).toBe(6);
  });

  test("it stays inside the draft when the text it sat in goes", () => {
    expect(shiftPasteOffset(9, "hello world", "hello wor")).toBe(7);
    expect(shiftPasteOffset(5, "hello world", "hello")).toBe(0);
    expect(shiftPasteOffset(0, "hello", "")).toBe(0);
    expect(shiftPasteOffset(3, "hello", "hello")).toBe(3);
  });

  test("every paste in the attachments moves together", () => {
    const file = { id: "f", kind: "file" as const, name: "a.ts", path: "/p/a.ts" };
    const first = { id: "1", kind: "text" as const, text: "A", at: 2 };
    const second = { id: "2", kind: "text" as const, text: "B", at: 8 };
    expect(trackPastes([file, first, second], "one two three", "oXne two three")).toEqual([
      file,
      { ...first, at: 3 },
      { ...second, at: 9 },
    ]);
    // Nothing moved: the same list comes back, so the composer does not write on every keystroke.
    const untouched = [first, second];
    expect(trackPastes(untouched, "same", "same")).toBe(untouched);
  });
});

describe("splicing long pastes into the message", () => {
  const paste = (text: string, at: number) => ({ id: text, kind: "text" as const, text, at });

  test("an empty draft is the paste itself", () => {
    expect(splicePastes("", [paste("LOG", 0)])).toBe("LOG");
  });

  test("at the start, the middle and the end of the typed text", () => {
    expect(splicePastes("explain this", [paste("LOG", 0)])).toBe("LOG\nexplain this");
    expect(splicePastes("before after", [paste("LOG", 7)])).toBe("before LOG\nafter");
    expect(splicePastes("beforeafter", [paste("LOG", 6)])).toBe("before\nLOG\nafter");
    expect(splicePastes("explain this", [paste("LOG", 12)])).toBe("explain this\nLOG");
  });

  test("two pastes go in by offset, and in the order they were made at the same one", () => {
    expect(splicePastes("xy", [paste("A", 0), paste("B", 2)])).toBe("A\nxy\nB");
    expect(splicePastes("", [paste("A", 0), paste("B", 0)])).toBe("A\nB");
    expect(splicePastes("q", [paste("B", 1), paste("A", 0)])).toBe("A\nq\nB");
  });

  test("text typed before and after the paste keeps it in place", () => {
    // "| world", then "well " typed in front of it.
    const at = shiftPasteOffset(0, " world", "well  world");
    expect(splicePastes("well  world", [paste("LOG", at)])).toBe("well LOG world");
    // A word typed past the end does not move a paste in the middle.
    expect(shiftPasteOffset(6, "hello world", "hello world again")).toBe(6);
  });

  test("a paste that already ends in a newline is not given a second one", () => {
    expect(splicePastes("before after", [paste("LOG\n", 7)])).toBe("before LOG\nafter");
    expect(splicePastes("beforeafter", [paste("\nLOG", 6)])).toBe("before\nLOG\nafter");
  });

  test("the typed text keeps its own trim, and a paste-only message is just the paste", () => {
    expect(splicePastes("  why?  ", [paste("LOG", 7)])).toBe("why?\nLOG");
    expect(splicePastes("  \n", [paste("LOG", 3)])).toBe("LOG");
    expect(splicePastes("\n\nwhy?\n", [paste("LOG", 0)])).toBe("LOG\nwhy?");
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
