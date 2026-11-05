import { describe, expect, test } from "bun:test";
import type { SlashCommand } from "../acp/types";
import type { ComposeExtras } from "./compose";
import {
  isOwnQueued,
  isQueuedMessage,
  nextQueued,
  queuedAnnouncement,
  queueView,
  STEER_REFUSAL,
  type SteerConditions,
  type SteerRefusal,
  steerability,
} from "./queue";
import type { QueuedPrompt } from "./thread";

const item = (id: string, extra: Partial<QueuedPrompt> = {}): QueuedPrompt => ({
  id,
  text: `message ${id}`,
  ...extra,
});

const thread = (queue: QueuedPrompt[], over: { running?: boolean; queueHeld?: boolean } = {}) => ({
  queue,
  running: over.running ?? false,
  queueHeld: over.queueHeld,
});

describe("the end of a thread's queue", () => {
  test("the user's messages are rows, in the order they run", () => {
    const view = queueView(thread([item("a"), item("b")]));
    expect(view.messages.map((q) => q.id)).toEqual(["a", "b"]);
    expect(view.wake).toBeUndefined();
    expect(view.held).toBe(false);
  });

  test("a wake is one quiet line, never a row, and a link message is a row of its own", () => {
    const view = queueView(
      thread([
        item("a"),
        item("w", { text: "2 children finished · 1 needs you\n\nFinished:", wake: true }),
        item("l", { link: "a/b" }),
      ]),
    );
    expect(view.messages.map((q) => q.id)).toEqual(["a", "l"]);
    // Its first line is its summary, which is what the transcript shows.
    expect(view.wake).toBe("2 children finished · 1 needs you");
    expect(isQueuedMessage(item("w", { wake: true }))).toBe(false);
    // Only the user's own message is editable in the composer.
    expect(isOwnQueued(item("a"))).toBe(true);
    expect(isOwnQueued(item("l", { link: "a/b" }))).toBe(false);
    expect(isOwnQueued(item("w", { wake: true }))).toBe(false);
  });

  test("a held queue says so, and offers Resume, only while no turn runs", () => {
    const held = queueView(thread([item("a")], { queueHeld: true }));
    expect(held.held).toBe(true);
    expect(held.resumable).toBe(true);
    // The queue that a held turn left does not stay held once its turn started.
    expect(queueView(thread([item("a")], { queueHeld: true, running: true })).held).toBe(false);
    // Nothing to resume with nothing queued.
    expect(queueView(thread([], { queueHeld: true })).resumable).toBe(false);
  });
});

describe("when the next queued message starts", () => {
  const queue = [item("a"), item("b")];

  test("the head of the queue starts when the thread is idle", () => {
    expect(nextQueued(queue, { running: false })?.id).toBe("a");
    expect(nextQueued([], { running: false })).toBeUndefined();
  });

  test("nothing starts while a turn runs", () => {
    expect(nextQueued(queue, { running: true })).toBeUndefined();
  });

  test("nothing starts while the queue is held, however idle the thread is", () => {
    expect(nextQueued(queue, { running: false, queueHeld: true })).toBeUndefined();
    // Resume lets it go: the same queue starts at its head.
    expect(nextQueued(queue, { running: false, queueHeld: undefined })?.id).toBe("a");
  });
});

describe("what the live region says", () => {
  test("counts what waits, and whether it is held", () => {
    expect(queuedAnnouncement(queueView(thread([item("a")])), false)).toBe("1 message queued");
    expect(queuedAnnouncement(queueView(thread([item("a"), item("b")])), false)).toBe("2 messages queued");
    expect(queuedAnnouncement(queueView(thread([item("a")], { queueHeld: true })), false)).toBe(
      "1 message held",
    );
    expect(queuedAnnouncement(queueView(thread([], { queueHeld: true })), false)).toBe("The queue is empty");
  });

  test("a wake is announced as waiting, and an edit says where it is", () => {
    const view = queueView(thread([item("w", { text: "1 child finished", wake: true })]));
    expect(queuedAnnouncement(view, false)).toBe("a wake-up waiting");
    expect(queuedAnnouncement(queueView(thread([item("a")])), true)).toBe(
      "1 message queued · Editing a queued message in the composer",
    );
  });
});

describe("the steer rule", () => {
  const image = { id: "i", kind: "image" as const, name: "shot.png", mimeType: "image/png", data: "AAAA" };
  const file = { id: "f", kind: "file" as const, name: "notes.txt", path: "/tmp/notes.txt" };
  const extras = (over: Partial<ComposeExtras> = {}): ComposeExtras => ({
    links: [],
    attachments: [],
    ...over,
  });
  const command: SlashCommand = {
    name: "/review",
    description: "Review",
    takesArguments: false,
    report: false,
    action: "execute",
  };
  const harness = (over: Partial<SteerConditions>): SteerConditions => ({
    live: true,
    images: true,
    builtin: false,
    ...over,
  });
  const no = (reason: SteerRefusal) => ({ ok: false, reason });

  test("plain text steers wherever the turn takes a message at all", () => {
    expect(steerability(undefined, harness({}))).toEqual({ ok: true });
    expect(steerability(extras(), harness({}))).toEqual({ ok: true });
    expect(steerability(extras(), harness({ live: false }))).toEqual(no("harness"));
  });

  test("pictures steer only where the harness takes them mid-turn, and not on the built-in one", () => {
    const pictures = extras({ attachments: [image] });
    expect(steerability(pictures, harness({}))).toEqual({ ok: true });
    expect(steerability(pictures, harness({ images: false }))).toEqual(no("harness"));
    // The engine answers `accepted: false` to a steer with an image block on the built-in harness.
    expect(steerability(pictures, harness({ builtin: true }))).toEqual(no("harness"));
    expect(steerability(pictures, harness({ live: false }))).toEqual(no("harness"));
  });

  test("a file, a command or a link waits for the next turn, pictures or not", () => {
    expect(steerability(extras({ attachments: [file] }), harness({}))).toEqual(no("files"));
    expect(steerability(extras({ command }), harness({}))).toEqual(no("commands"));
    expect(steerability(extras({ links: ["https://example.com"] }), harness({}))).toEqual(no("links"));
    expect(steerability(extras({ attachments: [image, file] }), harness({}))).toEqual(no("files"));
    expect(
      steerability(extras({ attachments: [image], links: ["https://example.com"] }), harness({})),
    ).toEqual(no("links"));
    // A command is the headline when both wait: it is what the message runs.
    expect(steerability(extras({ command, links: ["https://example.com"] }), harness({}))).toEqual(
      no("commands"),
    );
    // A file outranks the harness: it would wait for the next turn there too.
    expect(steerability(extras({ attachments: [file] }), harness({ live: false }))).toEqual(no("files"));
  });

  test("every combination of text, pictures, files, commands, links and harness has one answer", () => {
    const lists = [[], [image], [file], [image, file]];
    const kinds = [
      { cmd: undefined, links: [] as string[] },
      { cmd: command, links: [] as string[] },
      { cmd: undefined, links: ["https://example.com"] },
      { cmd: command, links: ["https://example.com"] },
    ];
    const seen: string[] = [];
    for (const attachments of lists)
      for (const { cmd, links } of kinds)
        for (const live of [true, false])
          for (const images of [true, false])
            for (const builtin of [true, false]) {
              const verdict = steerability(extras({ attachments, command: cmd, links }), {
                live,
                images,
                builtin,
              });
              // The refusal names the reason the user is shown, never a blank arrow.
              if (!verdict.ok) expect(STEER_REFUSAL[verdict.reason]).toBeTruthy();
              seen.push(
                `${attachments.map((a) => a.kind).join("+") || "text"}|${cmd ? "cmd" : ""}${links.length ? "link" : ""}|${live}${images}${builtin}|${
                  verdict.ok ? "yes" : verdict.reason
                }`,
              );
            }
    // 4 contents × 4 command/link shapes × 8 harness shapes, each answered once.
    expect(seen).toHaveLength(128);
    expect(seen).toContain("text||truetruefalse|yes");
    expect(seen).toContain("image||truetruefalse|yes");
    expect(seen).toContain("image||truetruetrue|harness");
    expect(seen).toContain("image||truefalsetrue|harness");
    expect(seen).toContain("text||falsetruefalse|harness");
    expect(seen).toContain("file||truetruefalse|files");
    expect(seen).toContain("image+file|cmd|truetruefalse|files");
    expect(seen).toContain("text|cmd|truetruefalse|commands");
    expect(seen).toContain("image|link|truetruefalse|links");
    expect(seen).toContain("text|cmdlink|truetruefalse|commands");
  });

  test("the words a dimmed arrow shows are the four reasons", () => {
    expect(STEER_REFUSAL).toEqual({
      files: "Files go with the next turn",
      commands: "Commands go with the next turn",
      links: "Links go with the next turn",
      harness: "This harness can't take a message mid-turn: it waits for the next one",
    });
  });
});
