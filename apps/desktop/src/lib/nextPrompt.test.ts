import { describe, expect, test } from "bun:test";
import { emptyThread, type ThreadState, type TranscriptItem } from "../state/thread";
import {
  completedReply,
  dismissNextPrompt,
  nextPromptDismissed,
  nextPromptFor,
  suggestionKeyAction,
  suggestNextPrompt,
} from "./nextPrompt";

describe("the suggestion a reply offers", () => {
  test("accepts an offer the reply ends on", () => {
    expect(suggestNextPrompt("Done.\n\nWant me to add tests for the parser?")).toBe(
      "Yes, add tests for the parser.",
    );
    expect(suggestNextPrompt("Fixed it. Should I open a pull request?")).toBe("Yes, open a pull request.");
    expect(suggestNextPrompt("Would you like me to go ahead and update your config?")).toBe(
      "Yes, update my config.",
    );
    expect(suggestNextPrompt("Shall I proceed?")).toBe("Yes, proceed.");
    expect(suggestNextPrompt("Let me know if you'd like me to **rename** the `helper`.")).toBe(
      "Yes, rename the helper.",
    );
    expect(suggestNextPrompt("I can also bump the version if you'd like.")).toBe("Yes, bump the version.");
  });

  test("looks at the last two sentences only", () => {
    expect(suggestNextPrompt("Want me to deploy? I left a note in the README.")).toBe("Yes, deploy.");
    expect(suggestNextPrompt("Want me to deploy? Here is why. It is all done now.")).toBeNull();
    expect(suggestNextPrompt("Want me to deploy?\n\nAll tests pass and the build is green.")).toBeNull();
  });

  test("names the next step", () => {
    expect(suggestNextPrompt("The schema changed.\n\nNext step: run the migration")).toBe(
      "Run the migration.",
    );
    expect(suggestNextPrompt("Summary.\n\n**Next steps:**\n1. Review the diff\n2. Merge it")).toBe(
      "Review the diff.",
    );
  });

  test("suggests nothing for a choice, a plain answer or a long offer", () => {
    expect(suggestNextPrompt("Should I keep it or remove it?")).toBeNull();
    expect(suggestNextPrompt("The function returns the sum.")).toBeNull();
    expect(suggestNextPrompt("Does that answer your question?")).toBeNull();
    expect(suggestNextPrompt("")).toBeNull();
    expect(suggestNextPrompt(`Want me to ${"refactor ".repeat(30)}?`)).toBeNull();
  });

  test("ignores code blocks", () => {
    expect(suggestNextPrompt("Want me to run it?\n\n```sh\nShould I delete everything?\n```")).toBe(
      "Yes, run it.",
    );
  });
});

const user = (id: string): TranscriptItem => ({ kind: "user", id, text: "hi" });
const reply = (id: string, text: string): TranscriptItem => ({ kind: "assistant", id, text });
const end = (outcome: "completed" | "cancelled" | "failed"): TranscriptItem => ({
  kind: "turnEnd",
  id: `end-${outcome}`,
  outcome,
});
const thread = (items: TranscriptItem[], extra: Partial<ThreadState> = {}): ThreadState => ({
  ...emptyThread("t1", "/tmp"),
  items,
  ...extra,
});

describe("the reply a completed turn ended on", () => {
  test("is the last assistant message of a completed turn", () => {
    expect(completedReply(thread([user("u"), reply("a", "Want me to fix it?"), end("completed")]))).toEqual({
      id: "a",
      text: "Want me to fix it?",
    });
  });

  test("is nothing while the turn runs, asks, failed or was stopped", () => {
    const items = [user("u"), reply("a", "Want me to fix it?")];
    expect(completedReply(thread([...items, end("cancelled")]))).toBeNull();
    expect(completedReply(thread([...items, end("failed")]))).toBeNull();
    expect(completedReply(thread(items, { running: true }))).toBeNull();
    expect(completedReply(thread([...items, end("completed")], { error: "boom" }))).toBeNull();
    // A message the user just sent: the reply belongs to the turn before it.
    expect(completedReply(thread([...items, end("completed"), user("u2")]))).toBeNull();
  });

  test("takes a replayed turn's timing for its end", () => {
    const items = [user("u"), reply("a", "Want me to fix it?")];
    expect(completedReply(thread(items))).toBeNull();
    expect(completedReply(thread(items, { pendingTurn: { outcome: "end_turn" } }))?.id).toBe("a");
    expect(completedReply(thread(items, { pendingTurn: { outcome: "cancelled" } }))).toBeNull();
  });

  test("keys the suggestion by thread and reply, and a dismissal holds for that key", () => {
    const t = thread([user("u"), reply("a", "Want me to fix it?"), end("completed")]);
    const next = nextPromptFor(t);
    expect(next).toEqual({ key: "t1:a", text: "Yes, fix it." });
    expect(nextPromptDismissed("t1:a")).toBe(false);
    dismissNextPrompt("t1:a");
    expect(nextPromptDismissed("t1:a")).toBe(true);
    expect(nextPromptDismissed("t1:b")).toBe(false);
  });
});

describe("keys on a shown suggestion", () => {
  const key = (
    k: string,
    mods: Partial<{ shiftKey: boolean; altKey: boolean; ctrlKey: boolean; metaKey: boolean }> = {},
  ) => ({
    key: k,
    shiftKey: false,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    ...mods,
  });

  test("Tab accepts and Escape dismisses", () => {
    expect(suggestionKeyAction(key("Tab"), false)).toBe("accept");
    expect(suggestionKeyAction(key("Escape"), false)).toBe("dismiss");
    expect(suggestionKeyAction(key("a"), false)).toBeNull();
    expect(suggestionKeyAction(key("Enter"), false)).toBeNull();
  });

  test("an open completion menu and modified keys keep precedence", () => {
    expect(suggestionKeyAction(key("Tab"), true)).toBeNull();
    expect(suggestionKeyAction(key("Escape"), true)).toBeNull();
    expect(suggestionKeyAction(key("Tab", { shiftKey: true }), false)).toBeNull();
    expect(suggestionKeyAction(key("Tab", { ctrlKey: true }), false)).toBeNull();
  });
});
