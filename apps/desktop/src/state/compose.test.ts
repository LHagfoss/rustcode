import { describe, expect, test } from "bun:test";
import {
  commandSuggestions,
  composedText,
  emptyExtras,
  hashToken,
  hasTokens,
  insertCommand,
  insertMention,
  mentionText,
  mentionToken,
  messageCollapses,
  promptBlocks,
  rankAgents,
  rankThreads,
  sentAttachments,
  sentMessages,
  serializeMentions,
  shiftPasteOffset,
  slashToken,
  splicePastes,
  splitImageMarkers,
  splitSlashCommand,
  splitTokens,
  splitUrls,
  type ThreadChoice,
  trackPastes,
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

describe("the hash token", () => {
  test("opens at the start or after whitespace, at any line", () => {
    expect(hashToken("#", 1)).toEqual({ start: 0, query: "" });
    expect(hashToken("#Ship", 5)).toEqual({ start: 0, query: "Ship" });
    expect(hashToken("look at #Ship", 13)).toEqual({ start: 8, query: "Ship" });
    expect(hashToken("one\ntwo #Sh", 11)).toEqual({ start: 8, query: "Sh" });
  });

  test("a word, a heading, a written token or code is not one", () => {
    // `abc#x` is not a mention: the `#` does not open a word.
    expect(hashToken("abc#x", 5)).toBeNull();
    // A markdown heading: its `#` is followed by a space.
    expect(hashToken("# Title", 7)).toBeNull();
    expect(hashToken("one\n# Title", 10)).toBeNull();
    // A mention already written, and the caret inside it.
    expect(hashToken("#[Title]", 5)).toBeNull();
    expect(hashToken("please #[Title] now", 12)).toBeNull();
    // Code, fenced and inline: a `#` there is text being written.
    expect(hashToken("```\n#Ship", 9)).toBeNull();
    expect(hashToken("a `#Sh` code", 7)).toBeNull();
    // The caret mid-word opens nothing.
    expect(hashToken("#Ship", 2)).toBeNull();
  });

  test("a word after a finished token is a new one", () => {
    expect(hashToken("#Fix the tests and #Sh", 22)).toEqual({ start: 19, query: "Sh" });
  });
});

describe("mention tokens", () => {
  const thread = { kind: "thread" as const, id: "42", title: "Ship the tree" };
  const codex = { kind: "agent" as const, harness: "codex", name: "codex" };

  test("the draft keeps the short form, and the message spells it out", () => {
    expect(mentionToken(thread)).toBe("#Ship the tree");
    expect(mentionText(thread)).toBe("#[Ship the tree](rustcode://thread/42)");
    expect(mentionToken(codex)).toBe("@codex");
    expect(mentionText(codex)).toBe("@codex (harness: codex)");
  });

  test("a title with a bracket is escaped, so the link ends where it should", () => {
    expect(mentionText({ ...thread, title: "fix [bug] in a.ts" })).toBe(
      "#[fix \\[bug\\] in a.ts](rustcode://thread/42)",
    );
  });

  test("inserting replaces the token and leaves a space to keep typing", () => {
    expect(insertMention("look at #Sh now", { start: 8, query: "Sh" }, thread)).toEqual({
      text: "look at #Ship the tree now",
      caret: 23,
    });
    expect(insertMention("#co", { start: 0, query: "co" }, codex).text).toBe("@codex ");
  });
});

describe("serializing mentions", () => {
  const thread = (id: string, title = "Ship the tree") => ({ kind: "thread" as const, id, title });

  test("each mention in the draft is written out in full", () => {
    expect(
      serializeMentions("look at #Ship the tree, then @codex", [
        thread("42"),
        { kind: "agent", harness: "codex", name: "codex" },
      ]),
    ).toBe("look at #[Ship the tree](rustcode://thread/42), then @codex (harness: codex)");
  });

  test("a mention no longer in the draft is dropped, and a bare `#123` is left alone", () => {
    expect(serializeMentions("see #123", [thread("42")])).toBe("see #123");
    expect(serializeMentions("nothing here", [thread("42")])).toBe("nothing here");
  });

  test("the longest title wins, and a token running into a word is another word", () => {
    const mentions = [thread("1", "Fix"), thread("2", "Fix the tests")];
    expect(serializeMentions("#Fix the tests pass", mentions)).toBe(
      "#[Fix the tests](rustcode://thread/2) pass",
    );
    // `#Fixer` is not `#Fix`.
    expect(serializeMentions("#Fixer", [thread("1", "Fix")])).toBe("#Fixer");
  });

  test("two threads with one title take the occurrences in turn", () => {
    const mentions = [thread("1"), thread("2")];
    expect(serializeMentions("#Ship the tree and #Ship the tree", mentions)).toBe(
      "#[Ship the tree](rustcode://thread/1) and #[Ship the tree](rustcode://thread/2)",
    );
  });

  test("the composed message carries the mention's words, not the draft's", () => {
    const extras = { ...emptyExtras, mentions: [thread("42")] };
    expect(composedText("look at #Ship the tree", extras)).toBe(
      "look at #[Ship the tree](rustcode://thread/42)",
    );
    // The command still leads, as the composer shows it.
    expect(composedText("#Ship the tree", { ...extras, command: command })).toBe(
      "/review #[Ship the tree](rustcode://thread/42)",
    );
  });
});

describe("threads and agents in the drawer", () => {
  const thread = (id: string, title: string, extra: Partial<ThreadChoice> = {}): ThreadChoice => ({
    id,
    title,
    state: "open",
    ...extra,
  });

  test("this project's threads come first, then the closest title", () => {
    const threads = [
      thread("a", "Snooze fix", { home: false }),
      thread("b", "Unrelated", { home: true }),
      thread("c", "Snooze again", { home: true }),
    ];
    expect(rankThreads(threads, "snooze").map((t) => t.id)).toEqual(["c", "a"]);
    // Nothing typed: this project's list, then the others.
    expect(rankThreads(threads, "").map((t) => t.id)).toEqual(["b", "c", "a"]);
  });

  test("what asks or works comes before what is settled, when two names match as well", () => {
    const threads = [
      thread("done", "Fix the tests", { state: "settled" }),
      thread("working", "Fix the tests", { state: "working" }),
      thread("asked", "Fix the tests", { state: "needs" }),
    ];
    expect(rankThreads(threads, "fix").map((t) => t.id)).toEqual(["asked", "working", "done"]);
  });

  test("the harnesses an `@` looks for, by name or by id", () => {
    const agents = [
      { harness: "claude-code", name: "Claude Code" },
      { harness: "codex", name: "Codex" },
      { harness: "rustcode", name: "RustCode" },
    ];
    expect(rankAgents("co", agents).map((a) => a.harness)).toEqual(["codex", "rustcode", "claude-code"]);
    expect(rankAgents("zzz", agents)).toEqual([]);
  });
});

describe("a mention in the draft", () => {
  const thread = { kind: "thread" as const, id: "42", title: "Ship the tree" };

  test("is drawn on a pill as the draft's own characters", () => {
    expect(splitTokens("see #Ship the tree now", [thread])).toEqual([
      { text: "see " },
      { mention: "#Ship the tree" },
      { text: " now" },
    ]);
    expect(hasTokens("see #Ship the tree", [thread])).toBe(true);
  });

  test("is the draft's text until it is picked from the drawer", () => {
    expect(splitTokens("see #123 now")).toEqual([{ text: "see #123 now" }]);
  });

  test("a harness mention is the same pill a file reference gets", () => {
    const codex = { kind: "agent" as const, harness: "codex", name: "codex" };
    expect(splitTokens("ask @codex", [codex])).toEqual([{ text: "ask " }, { ref: "codex" }]);
  });
});
