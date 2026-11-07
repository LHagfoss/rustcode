import { describe, expect, test } from "bun:test";
import type { SlashCommand } from "../acp/types";
import type { ComposeExtras } from "./compose";
import {
  isOwnQueued,
  isQueuedMessage,
  nextQueued,
  QUEUE_REASON,
  type QueueReason,
  queuedAnnouncement,
  queueView,
  type SendDecision,
  type SteeringCapabilities,
  sendDecision,
  type TurnState,
  wakeDelivery,
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
        item("w", { text: "2 delegates finished · Delegate asks: Review PR\n\nFinished:", wake: true }),
        item("l", { link: "a/b" }),
      ]),
    );
    expect(view.messages.map((q) => q.id)).toEqual(["a", "l"]);
    // Its first line is its summary, which is what the transcript shows.
    expect(view.wake).toBe("2 delegates finished · Delegate asks: Review PR");
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
    const view = queueView(thread([item("w", { text: "Delegate finished: Fix the composer", wake: true })]));
    expect(queuedAnnouncement(view, false)).toBe("a wake-up waiting: Delegate finished: Fix the composer");
    // A wake folded into an earlier one keeps its row but not its line: the new summary is announced.
    const folded = queueView(
      thread([
        item("w", {
          text: "2 delegates finished\n\nFinished:\n- link_id a/b · done",
          wake: true,
          wakeLinks: ["a/b", "a/c"],
        }),
      ]),
    );
    expect(queuedAnnouncement(folded, false)).toBe("a wake-up waiting: 2 delegates finished");
    expect(queuedAnnouncement(queueView(thread([item("a")])), true)).toBe(
      "1 message queued · Editing a queued message in the composer",
    );
  });
});

describe("the one send rule", () => {
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
  const steering = (over: Partial<SteeringCapabilities> = {}): SteeringCapabilities => ({
    live: true,
    attachments: true,
    while_awaiting_user: true,
    interrupt_restart: true,
    queue: true,
    ...over,
  });
  const turn = (over: Partial<TurnState> = {}): TurnState => ({
    running: true,
    awaitingUser: false,
    ...over,
  });
  const queued = (reason: QueueReason): SendDecision => ({ action: "queue", reason });

  test("plain text steers wherever the turn takes a message at all", () => {
    expect(sendDecision(undefined, steering(), turn())).toEqual({ action: "steer" });
    expect(sendDecision(extras(), steering(), turn())).toEqual({ action: "steer" });
    expect(sendDecision(extras(), steering({ live: false }), turn())).toEqual(queued("harness"));
    // Nothing running: nothing to steer into.
    expect(sendDecision(extras(), steering(), turn({ running: false }))).toEqual(queued("idle"));
  });

  test("pictures steer only where the harness takes them mid-turn", () => {
    const pictures = extras({ attachments: [image] });
    expect(sendDecision(pictures, steering(), turn())).toEqual({ action: "steer" });
    // The built-in turn reads a steer as text, so its pictures wait.
    expect(sendDecision(pictures, steering({ attachments: false }), turn())).toEqual(queued("attachments"));
    expect(sendDecision(pictures, steering({ live: false }), turn())).toEqual(queued("harness"));
  });

  test("an approval or a question stops a turn that reads steers between steps only", () => {
    // The built-in turn: it queues, and says the turn is what waits.
    const between = steering({ while_awaiting_user: false });
    expect(sendDecision(extras(), between, turn({ awaitingUser: true }))).toEqual(queued("awaiting"));
    // Even a message that would otherwise wait for its pictures.
    expect(
      sendDecision(
        extras({ attachments: [image] }),
        { ...between, attachments: false },
        turn({ awaitingUser: true }),
      ),
    ).toEqual(queued("awaiting"));
    // Codex and a steering ACP agent take a message while the card is up.
    expect(sendDecision(extras(), steering(), turn({ awaitingUser: true }))).toEqual({ action: "steer" });
    // And nothing steers into a turn that is not running, whoever the harness is.
    expect(sendDecision(extras(), steering(), turn({ running: false, awaitingUser: true }))).toEqual(
      queued("idle"),
    );
  });

  test("a file, a command or a link waits for the next turn, pictures or not", () => {
    expect(sendDecision(extras({ attachments: [file] }), steering(), turn())).toEqual(queued("files"));
    expect(sendDecision(extras({ command }), steering(), turn())).toEqual(queued("commands"));
    expect(sendDecision(extras({ links: ["https://example.com"] }), steering(), turn())).toEqual(
      queued("links"),
    );
    expect(sendDecision(extras({ attachments: [image, file] }), steering(), turn())).toEqual(queued("files"));
    expect(
      sendDecision(extras({ attachments: [image], links: ["https://example.com"] }), steering(), turn()),
    ).toEqual(queued("links"));
    // A command is the headline when both wait: it is what the message runs.
    expect(sendDecision(extras({ command, links: ["https://example.com"] }), steering(), turn())).toEqual(
      queued("commands"),
    );
    // A file outranks the harness: it would wait for the next turn there too.
    expect(sendDecision(extras({ attachments: [file] }), steering({ live: false }), turn())).toEqual(
      queued("files"),
    );
  });

  test("a harness that cannot queue stops the turn instead, and only then", () => {
    const restart: SendDecision = { action: "interruptRestart" };
    expect(sendDecision(extras(), steering({ live: false, queue: false }), turn())).toEqual(restart);
    expect(
      sendDecision(extras({ attachments: [file] }), steering({ live: false, queue: false }), turn()),
    ).toEqual(queued("files"));
    // The last resort: a harness that says it can do none of the three still waits.
    expect(
      sendDecision(extras(), steering({ live: false, queue: false, interrupt_restart: false }), turn()),
    ).toEqual(queued("harness"));
  });

  test("every combination of content, harness and turn has one answer", () => {
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
          for (const takes of [true, false])
            for (const queue of [true, false])
              for (const awaiting of [true, false])
                for (const awaitingUser of [true, false]) {
                  const decision = sendDecision(
                    extras({ attachments, command: cmd, links }),
                    steering({
                      live,
                      attachments: takes,
                      while_awaiting_user: awaiting,
                      queue,
                    }),
                    turn({ awaitingUser }),
                  );
                  // Every wait names a reason the user is shown, never a blank arrow.
                  if (decision.action === "queue") expect(QUEUE_REASON[decision.reason]).toBeTruthy();
                  seen.push(
                    `${attachments.map((a) => a.kind).join("+") || "text"}|${cmd ? "cmd" : ""}${links.length ? "link" : ""}|${live}${takes}${queue}${awaiting}${awaitingUser}|${
                      decision.action === "queue" ? decision.reason : decision.action
                    }`,
                  );
                }
    // 4 contents × 4 command/link shapes × 32 harness/turn shapes, each answered once.
    expect(seen).toHaveLength(512);
    expect(seen).toContain("text||truetruetruetruefalse|steer");
    expect(seen).toContain("image||truetruetruetruefalse|steer");
    expect(seen).toContain("image||truefalsetruetruefalse|attachments");
    expect(seen).toContain("text||truetruetruefalsetrue|awaiting");
    expect(seen).toContain("text||falsefalsetruetruefalse|harness");
    expect(seen).toContain("text||falsefalsefalsetruefalse|interruptRestart");
    expect(seen).toContain("file||truetruetruetruefalse|files");
    expect(seen).toContain("image+file|cmd|truetruetruetruefalse|files");
    expect(seen).toContain("text|cmd|truetruetruetruefalse|commands");
    expect(seen).toContain("image|link|truetruetruetruefalse|links");
    expect(seen).toContain("text|cmdlink|truetruetruetruefalse|commands");
  });

  test("every harness the engine ships has one answer, from its own capabilities", () => {
    const caps = {
      rustcode: steering({ attachments: false }),
      codex: steering(),
      "claude-code": steering(),
      gemini: steering({ live: false }),
      opencode: steering({ live: false }),
      pi: steering({ live: false }),
    };
    const answers = Object.fromEntries(
      Object.entries(caps).map(([id, harness]) => [
        id,
        sendDecision(extras(), harness, turn()).action === "steer" ? "steer" : "queue",
      ]),
    );
    expect(answers).toEqual({
      rustcode: "steer",
      codex: "steer",
      "claude-code": "steer",
      gemini: "queue",
      opencode: "queue",
      pi: "queue",
    });
    // And a picture: the built-in turn takes text only, the others take it with the message.
    const picture = extras({ attachments: [image] });
    expect(sendDecision(picture, caps.rustcode, turn())).toEqual(queued("attachments"));
    expect(sendDecision(picture, caps.codex, turn())).toEqual({ action: "steer" });
    expect(sendDecision(picture, caps["claude-code"], turn())).toEqual({ action: "steer" });
    // A harness that steers only its text takes a picture with the next turn, not as a stop.
    expect(sendDecision(picture, steering({ attachments: false }), turn())).toEqual(queued("attachments"));
  });

  test("the words a dimmed arrow shows name every reason", () => {
    expect(QUEUE_REASON).toEqual({
      idle: "No turn is running: this starts one",
      files: "Files go with the next turn",
      commands: "Commands go with the next turn",
      links: "Links go with the next turn",
      attachments: "This harness takes text mid-turn only: the pictures go with the next turn",
      awaiting: "The turn is waiting on you: this goes with the next one",
      refused: "The turn can't take it now: it goes with the next one",
      harness: "This harness can't take a message mid-turn: it waits for the next one",
    });
  });
});

describe("how a wake reaches a parent that is mid-turn", () => {
  const steering = (over: Partial<SteeringCapabilities> = {}): SteeringCapabilities => ({
    live: true,
    attachments: true,
    while_awaiting_user: true,
    interrupt_restart: true,
    queue: true,
    ...over,
  });
  const turn = (over: Partial<TurnState> = {}): TurnState => ({
    running: true,
    awaitingUser: false,
    ...over,
  });

  test("a child's question is steered into a running turn the harness reads steers in", () => {
    expect(wakeDelivery(steering(), turn(), 1)).toBe("steer");
    // A parent that steers with the card up: the question goes in anyway.
    expect(wakeDelivery(steering(), turn({ awaitingUser: true }), 1)).toBe("steer");
  });

  test("a harness that takes no live steer keeps the wake queued", () => {
    expect(wakeDelivery(steering({ live: false }), turn(), 1)).toBe("queue");
    expect(wakeDelivery(steering({ live: false }), turn({ awaitingUser: true }), 1)).toBe("queue");
  });

  test("a wake never stops a parent's turn, whatever the harness can do", () => {
    // A harness that would have to restart the turn to take a message: its tool calls come first.
    expect(wakeDelivery(steering({ live: false, queue: false }), turn(), 1)).toBe("queue");
    expect(wakeDelivery(steering({ live: false, queue: false, interrupt_restart: true }), turn(), 1)).toBe(
      "queue",
    );
    // One that cannot take a message at all waits beside it.
    expect(wakeDelivery(steering({ live: false, queue: false, interrupt_restart: false }), turn(), 1)).toBe(
      "queue",
    );
  });

  test("a turn that reads no steer while an approval waits takes the question after it", () => {
    // The built-in turn reads a steer between steps only: the card is the user's to answer first.
    expect(wakeDelivery(steering({ while_awaiting_user: false }), turn({ awaitingUser: true }), 1)).toBe(
      "queue",
    );
    // With no card waiting, the same harness steers.
    expect(wakeDelivery(steering({ while_awaiting_user: false }), turn(), 1)).toBe("steer");
  });

  test("a completion waits its turn, and so does anything for a parent with no turn running", () => {
    expect(wakeDelivery(steering(), turn(), 0)).toBe("queue");
    expect(wakeDelivery(steering(), turn({ running: false }), 1)).toBe("queue");
    expect(wakeDelivery(steering({ live: false }), turn({ running: false }), 0)).toBe("queue");
  });
});
