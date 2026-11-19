// Sending from the composer: submitting, forking, quoting and side chats, steering a running turn,
// and the queue of messages that wait for it. Split out of state/store.ts; the state these write
// still lives in the app store.

import { type JsonObject, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { inheritedModeOf, permissionModeOf, setPermissionMode } from "./approvals";
import {
  type ComposeExtras,
  carriesExtras,
  composedText,
  emptyExtras,
  hasExtras,
  promptBlocks,
  sentAttachments,
} from "./compose";
import { selectOption } from "./harness";
import { setNewThreadChoice } from "./harnessActions";
import { answerStatusLocally, linkSteered, queuedLinkDropped } from "./host";
import { newThreadAccountOn } from "./logins";
import { parseDelivered } from "./orchestration";
import { effectiveMode } from "./permissions";
import { type PreparedPullBranch, parsePullBranch } from "./pullThread";
import {
  nextQueued,
  QUEUE_REASON,
  type QueueReason,
  type SendDecision,
  type SteeringCapabilities,
  sendDecision,
  type TurnState,
} from "./queue";
import { allowAgentRename, parseRename, RENAME_PROMPT } from "./renameCommand";
import {
  type AppState,
  advertisedCommands,
  cancel,
  commandsFor,
  connectionForThread,
  createSession,
  createThread,
  engine,
  errorText,
  harnessesOn,
  newThreadChoice,
  peekThread,
  projectThreadChoice,
  refreshSessions,
  runPrompt,
  selectThread,
  setDraft,
  setExtras,
  setSwitcher,
  startNewThread,
  toast,
  updateThread,
  useApp,
  wakeOnWrite,
} from "./store";
import {
  appendSteer,
  departQueued,
  dropDeparted,
  isRunning,
  localId,
  messagesAfter,
  type QueuedPrompt,
  type ThreadState,
} from "./thread";
import { askConfirm, cancelEdit, renameThread, rewindTo } from "./threadActions";
import { LOCAL_HOST, splitThreadKey } from "./threadKeys";
import { sessionById, unsettleThreads } from "./threadStatus";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

/** The harness a thread runs on, or a new thread would start on. */
export function threadHarness(thread?: ThreadState) {
  const s = get();
  // A thread's harness is the one its own machine lists; a new thread's is the one the computer
  // **Run on** names (docs/desktop.md › Several machines).
  const snapshot = harnessesOn(s, thread ? splitThreadKey(thread.id).host : s.newThreadHost);
  const id = thread
    ? (thread.harness?.id ?? "rustcode")
    : (s.newThread?.harness ?? snapshot?.default.harness);
  return snapshot?.harnesses.find((h) => h.id === id);
}

/** Whether the thread's harness takes image blocks. */
export function acceptsImages(thread?: ThreadState): boolean {
  const caps = thread?.harness?.capabilities ?? threadHarness(thread)?.capabilities;
  if (caps && !(threadHarness(thread)?.builtin ?? false)) return caps.images;
  return get().agent?.promptImages ?? true;
}

/** ⏎: send, or queue behind a running turn. False when no thread could start (the draft stays). */
/**
 * Commands the app answers itself, by the action the engine advertises with
 * each one (`_meta.rustcode.action`): nothing goes to the engine as a prompt.
 * Returns whether the command was one of them.
 */
export function runCommandAction(text: string, from: string | null): boolean {
  const s = get();
  const command = commandsFor(s, from).find((c) => c.name === text);
  if (!command || command.action === "execute") return false;
  const thread = from ? s.threads[from] : undefined;
  switch (command.action) {
    case "new_session": {
      // A fresh thread on the same project, harness, model and effort, selected and empty; in a
      // new thread it keeps the folder and the choice already made there.
      const choice = newThreadChoice(s);
      const model = thread ? selectOption(thread.configOptions, "model")?.current : choice?.model;
      const effort = thread ? selectOption(thread.configOptions, "effort")?.current : choice?.effort;
      const harness =
        thread?.harness?.id ?? (from ? s.sessions.find((x) => x.id === from)?.harness : choice?.harness);
      // `/new` in a thread starts on that thread's computer, in its folder, even before it is
      // open; in a new thread, on the current Run on.
      const listed = from ? s.sessions.find((x) => x.id === from) : undefined;
      startNewThread(
        thread?.cwd ?? listed?.cwd ?? (from ? undefined : s.projectCwd),
        from ? splitThreadKey(from).host : s.newThreadHost,
      );
      if (harness) setNewThreadChoice({ harness, model: model ?? null, effort: effort ?? null });
      return true;
    }
    case "model_picker":
      set({ modelPickerAsk: { toggle: false } });
      return true;
    case "session_picker":
      setSwitcher(true);
      return true;
    case "copy_response": {
      const reply = [...(thread?.items ?? [])].reverse().find((i) => i.kind === "assistant");
      if (reply?.kind === "assistant" && reply.text.trim()) {
        void navigator.clipboard.writeText(reply.text).then(
          () => void toast("Copied the last reply", "success"),
          (error) => void toast(`Could not copy: ${errorText(error)}`, "error"),
        );
      } else toast("There is no reply to copy yet");
      return true;
    }
    default:
      return false;
  }
}

export async function submit(text: string, extras: ComposeExtras = emptyExtras): Promise<boolean> {
  // A worktree still setting up owns the new-thread screen: sending again must not make a second.
  const trimmed = text.trim();
  if (!trimmed && !hasExtras(extras)) return false;
  // A command the app answers itself, typed alone or left as a bare token.
  const bare = !hasExtras(extras)
    ? trimmed
    : !trimmed && extras.command && !extras.links.length && !extras.attachments.length
      ? extras.command.name
      : null;
  if (bare && runCommandAction(bare, get().selectedId)) return true;
  const rename = hasExtras(extras) ? null : parseRename(trimmed);
  if (rename) {
    const open = get().selectedId;
    if (!open) {
      toast("Start the thread first, then /rename it.", "info");
      return false;
    }
    if (rename.title) return renameThread(open, rename.title, { user: true });
    // The agent names it with `update_session`; the turn runs, or waits in the queue, as any message.
    allowAgentRename(open);
    text = RENAME_PROMPT;
  }
  let id = get().selectedId;
  // The thread is made for this message, so the new-thread screen already draws its row: it must not
  // fly in again when the thread takes the screen over (lib/sendFlight.ts).
  const madeHere = !id;
  try {
    if (!id) {
      const creating = createThread(text, extras);
      const setupId = get().pendingSetup?.id;
      const started = await creating;
      // A setup that failed keeps the message and its attachments on the thread's own bubble, with
      // Retry: the field stays clear rather than holding a second copy. One that could not start at
      // all (another is still going) leaves the draft in the composer, as it was.
      if (!started) {
        const s = get();
        return !!(s.pendingSetup?.id === setupId ? s.pendingSetup : s.backgroundSetups[setupId ?? ""])
          ?.failure;
      }
      id = started;
    }
  } catch (error) {
    toast(`Could not start the thread: ${errorText(error)}`, "error");
    return false;
  }
  const thread = get().threads[id];
  if (!thread) return false;
  if (trimmed === "/cancel" && !hasExtras(extras)) {
    await cancel(id);
    return true;
  }
  // A bare "status" in a thread with delegates is answered from what the app knows, with no turn.
  if (!hasExtras(extras) && !get().editing[id] && answerStatusLocally(id, trimmed)) return true;
  const edit = get().editing[id];
  if (edit && !isRunning(thread)) {
    cancelEdit(id);
    const later = messagesAfter(thread, edit.itemId);
    const go = async () => {
      if (await rewindTo(id!, edit.itemId, edit.original)) {
        unsettleThreads([id!], { quiet: true });
        void runPrompt(id!, text, extras);
      }
    };
    if (later > 0) {
      askConfirm({
        title: "Send the edited message?",
        message: `The thread goes back to the message you edited; ${later === 1 ? "the message after it and its reply" : `the ${later} messages after it and their replies`} will be removed.`,
        action: "Send",
        destructive: true,
        run: () => void go(),
      });
    } else void go();
    return true;
  }
  if (trimmed !== "/fork" || hasExtras(extras)) {
    unsettleThreads([id], { quiet: true });
    // Its message may wait behind the running turn: the thread comes back all the same.
    wakeOnWrite(id);
  }
  if (isRunning(thread)) {
    updateThread(id, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        // The text as it was written: a paste's offset is an offset in it, and the row composes the
        // two when it draws and when the turn runs (`composedText`).
        { id: localId("queued"), text, extras: carriesExtras(extras) ? extras : undefined },
      ],
    }));
    return true;
  }
  if (trimmed === "/fork" && !hasExtras(extras)) {
    void forkThread(id);
    return true;
  }
  // Messages a stopped turn held keep their place: a new one goes behind them and Resumes the queue,
  // so nothing the user wrote is skipped or reordered.
  if (thread.queue.length) {
    updateThread(id, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        { id: localId("queued"), text, extras: carriesExtras(extras) ? extras : undefined },
      ],
    }));
    void resumeQueue(id);
    return true;
  }
  void runPrompt(id, text, extras, { alreadyShown: madeHere });
  return true;
}

/**
 * `/fork` copies the chat into a new thread and leaves this one open (the
 * engine's rule). The reply does not name the new thread, so the list is
 * compared before and after, and a toast offers to open it.
 */
export async function forkThread(id: string): Promise<void> {
  const before = new Set(get().sessions.map((x) => x.id));
  await runPrompt(id, "/fork");
  await refreshSessions();
  const fork = get().sessions.find((x) => !before.has(x.id));
  if (!fork) return;
  toast(`Forked into "${fork.title || "a new thread"}"`, "success", {
    action: { label: "Open", run: () => selectThread(fork.id) },
  });
}

/** The text of a message as a quote: every line behind `> `, ready to be followed by a question. */
export function quoteMessage(text: string): string {
  const lines = text.trim().split("\n");
  return `${lines.map((line) => (line ? `> ${line}` : ">")).join("\n")}\n\n`;
}

/**
 * "Ask in a side chat": a new thread beside `parentId` on the same harness, model, effort and
 * folder (never a new worktree), whose composer starts with `message` quoted. It opens in the
 * panel's Thread tab so the parent stays on screen; closing the panel keeps the thread, and the
 * sidebar and the parent's transcript list it. The engine records the parent (relationship
 * `side_chat`) in the new thread's own metadata.
 */
export async function openSideChat(parentId: string, message: string): Promise<string | undefined> {
  const parent = get().threads[parentId];
  if (!parent || !message.trim()) return undefined;
  // An engine that predates the relationship ignores it and would list the side chat as a delegate.
  if (!get().agent?.lineage) {
    toast("Side chats need a newer RustCode engine. Update RustCode and restart.", "error");
    return undefined;
  }
  const choice = (configId: string) => parent.configOptions.find((o) => o.id === configId)?.current;
  const title = sessionById(get(), parentId)?.title || parent.title;
  try {
    const id = await createSession({
      // The folder the parent runs in, its worktree included: a side chat reads the same files.
      cwd: parent.cwd,
      harness: parent.harness?.id,
      account: parent.harness?.account,
      model: choice("model") ?? null,
      effort: choice("effort") ?? null,
      parent: parentId,
      relationship: "side_chat",
      title: title ? `Side chat: ${title}` : "Side chat",
    });
    // Never broader than the thread it was asked from, as for a delegate (state/permissions.ts).
    setPermissionMode(id, effectiveMode(permissionModeOf(get(), parentId), inheritedModeOf(get(), parentId)));
    setDraft(id, quoteMessage(message));
    peekThread(id);
    return id;
  } catch (error) {
    toast(`Could not open a side chat: ${error instanceof Error ? error.message : String(error)}`, "error");
    return undefined;
  }
}

/**
 * "Work on this in a thread" (issue 3727): a worktree thread on pull request `number`'s head branch
 * (`_rustcode/pulls/branch` fetches it, a fork's too), with `message` in its composer to read and
 * edit before sending, and the pull request linked so new checks and comments wake it. Runs on the
 * harness a new thread in `cwd` would. `message` is written from the branch the engine prepared, so
 * it names the branch's real name and what a push does (issue 3743); a worktree that already has the
 * branch is where the thread starts, rather than a second one. Answers the thread's id, or undefined
 * after a toast.
 */
export async function startPullThread(
  cwd: string,
  pull: { number: number; title: string; url?: string },
  message: (branch: PreparedPullBranch) => string,
): Promise<string | undefined> {
  const s = get();
  try {
    const answer = obj(
      await engine().request("_rustcode/pulls/branch", { cwd, number: pull.number }, 180_000),
    );
    const branch = parsePullBranch(answer?.branch);
    if (!branch)
      throw new Error(str(obj(answer?.error)?.message) ?? "The pull request's branch could not be fetched.");
    const choice =
      projectThreadChoice(s, cwd) ??
      newThreadChoice({
        newThread: s.newThread,
        harnesses: s.harnesses,
        remoteHarnesses: s.remoteHarnesses,
        newThreadHost: LOCAL_HOST,
        projectCwd: cwd,
      });
    const harness = harnessesOn(s, LOCAL_HOST)?.harnesses.find((h) => h.id === choice?.harness);
    const id = await createSession({
      cwd: branch.worktree ?? cwd,
      harness: choice?.harness,
      account: newThreadAccountOn(harness, s.newThreadAccount, s.signedOutLogins),
      model: choice?.model,
      effort: choice?.effort,
      ...(branch.worktree ? {} : { worktree: { repo: cwd, branch: branch.branch, title: pull.title } }),
      select: true,
      title: `#${pull.number} ${pull.title}`.slice(0, 80),
    });
    const note = branch.note;
    setDraft(id, message(branch));
    const repo = /github\.com\/([\w.-]+\/[\w.-]+)\/pull\//.exec(pull.url ?? "")?.[1];
    const worktreeCwd = get().threads[id]?.cwd ?? cwd;
    void engine()
      .request(
        "_rustcode/pulls/watch/start",
        {
          sessionId: splitThreadKey(id).id,
          cwd: worktreeCwd,
          number: pull.number,
          ...(repo ? { repo } : {}),
          link: true,
        },
        60_000,
      )
      .catch(() => {});
    if (note) toast(note, "info", { persist: true });
    return id;
  } catch (error) {
    toast(`Could not open #${pull.number} in a thread: ${errorText(error)}`, "error");
    return undefined;
  }
}

/** Add a side-chat answer to its parent's editable draft. */
export function sendSideChatToParent(id: string, answer: string): boolean {
  const parent = sessionById(get(), id)?.parent;
  if (parent?.relationship !== "side_chat" || !parent.id || !answer.trim()) return false;
  const draft = get().drafts[parent.id] ?? "";
  setDraft(parent.id, draft ? `${draft}\n\n${answer}` : answer);
  toast("Added to the main thread draft", "success");
  return true;
}

/**
 * ⌘⏎: hand the running turn a message to read after its current step, else send. What it does is
 * `sendDecision`'s answer for this message, the same one the composer's hint and the queued row's
 * Send now read, so nothing here disagrees with what the app says it will do.
 */
export async function steerOrSubmit(text: string, extras: ComposeExtras = emptyExtras): Promise<boolean> {
  const id = get().selectedId;
  const thread = id ? get().threads[id] : undefined;
  const trimmed = text.trim();
  if (!id || !isRunning(thread) || !get().agent?.steer) return submit(text, extras);
  // Nothing typed and nothing attached: take what waits first, by the row's own rule.
  if (!trimmed && !hasExtras(extras)) {
    const next = thread.queue[0];
    if (next) return sendQueuedNow(id, next.id);
    return submit(text, extras);
  }
  unsettleThreads([id], { quiet: true });
  // A steer or a queued row leaves no turn of its own behind to read the snooze: it is the user
  // writing in the thread, so the thread comes back here (`submit` does the same).
  wakeOnWrite(id);
  const verdict = sendDecisionFor(thread, extras);
  if (verdict.action === "steer" && (await steer(id, text, extras))) return true;
  if (verdict.action === "interruptRestart") return restartWith(id, text, extras);
  // A steer the engine refused (the turn ended or began waiting on the user meanwhile) waits too.
  return queueMessage(id, text, extras, verdict.action === "queue" ? verdict.reason : "refused");
}

/** A message that does not steer waits for the next turn, saying why it waits. */
async function queueMessage(
  id: string,
  text: string,
  extras: ComposeExtras,
  reason: QueueReason,
): Promise<boolean> {
  if (carriesExtras(extras)) {
    // Submitted rather than parked, so the chips and the ids a mention stands for go with it.
    const sent = await submit(text, extras);
    if (sent) toast(QUEUE_REASON[reason]);
    return sent;
  }
  updateThread(id, (t) => ({
    ...t,
    queue: [...t.queue, { id: localId("queued"), text }],
  }));
  toast(QUEUE_REASON[reason]);
  return true;
}

/**
 * Stop the running turn and send this message in its place: what `sendDecision` answers for a
 * harness that takes neither a steer nor the queue. The message goes to the head of the queue for
 * the stop to land, and a turn this stopped runs it on (`thread.ts` `finishTurn` keeps the queue
 * for Resume otherwise).
 */
async function restartWith(id: string, text: string, extras: ComposeExtras): Promise<boolean> {
  updateThread(id, (t) => ({
    ...t,
    queue: [
      {
        id: localId("queued"),
        text,
        extras: carriesExtras(extras) ? extras : undefined,
        restart: true,
      },
      ...t.queue,
    ],
  }));
  await cancel(id);
  return true;
}

/** The steering record of the harness a thread runs on: the snapshot's is the fresher of the two. */
export function steeringOf(thread?: ThreadState): SteeringCapabilities {
  const own = thread?.harness?.capabilities.steering;
  const fresh = threadHarness(thread)?.capabilities.steering;
  if (!own)
    return (
      fresh ?? {
        live: false,
        attachments: false,
        while_awaiting_user: true,
        interrupt_restart: true,
        queue: true,
      }
    );
  if (!fresh) return own;
  // A harness that learned to steer after this thread started is `live` in the snapshot only.
  return {
    ...own,
    live: own.live || fresh.live,
    attachments: own.attachments || fresh.attachments,
  };
}

/** What the thread's turn is doing, for the one send rule. */
export function turnState(thread?: ThreadState): TurnState {
  return {
    running: isRunning(thread),
    awaitingUser: (thread?.permissions.length ?? 0) > 0,
  };
}

/** What sending `extras` to this thread now would do: steer, queue (with the reason), or restart. */
export function sendDecisionFor(
  thread: ThreadState | undefined,
  extras: ComposeExtras | undefined,
): SendDecision {
  return sendDecision(extras, steeringOf(thread), turnState(thread));
}

/**
 * Hands the running turn a message to read after its current step (`_rustcode/steer`). False when the
 * engine will not take one now. Exported for the wake a busy parent gets for a child's question
 * (state/host.ts `sendWake`), which is sent by the same rule as a typed message.
 */
export async function steer(
  id: string,
  text: string,
  extras?: ComposeExtras,
  sender?: string,
  link?: string,
): Promise<boolean> {
  const thread = get().threads[id];
  // What the turn reads: the typed text with its command, links and long pastes in place.
  const full = composedText(text, extras ?? emptyExtras);
  const conditions = {
    images: acceptsImages(thread),
    builtin: threadHarness(thread)?.builtin ?? true,
  };
  const prompt =
    extras && (extras.attachments.length || extras.command?.action === "skill")
      ? promptBlocks(text, extras, {
          images: conditions.images,
          builtin: conditions.builtin,
          cwd: thread?.cwd ?? "",
          advertised: advertisedCommands(get(), id),
        })
      : undefined;
  try {
    const conn = connectionForThread(id);
    if (!conn) return false;
    const sessionId = splitThreadKey(id).id;
    const result = (await conn.request(
      Method.steer,
      {
        sessionId,
        text: full,
        ...(prompt ? { prompt } : {}),
        ...(sender && parseDelivered(text)
          ? {
              _meta: {
                rustcode: {
                  origin: {
                    kind: "agent",
                    threads: [
                      {
                        sessionId: sender,
                        title: parseDelivered(text)?.title ?? "",
                        ...(link ? { linkId: link } : {}),
                      },
                    ],
                  },
                },
              },
            }
          : {}),
      },
      10_000,
    )) as JsonObject | null;
    if (result?.accepted !== true) return false;
    updateThread(id, (t) => appendSteer(t, full, Date.now(), extras && sentAttachments(extras)));
    // The steered row in the transcript says it was read after the current step.
    return true;
  } catch {
    return false;
  }
}

/**
 * Send now: steer a queued message into the running turn, by the same rule ⌘⏎ follows. One the rule
 * refuses stays queued and says why; one the engine refuses stays queued too.
 */
export async function sendQueuedNow(id: string, queuedId: string): Promise<boolean> {
  const thread = get().threads[id];
  const item = thread?.queue.find((q) => q.id === queuedId);
  if (!item || !thread) return false;
  const verdict = sendDecisionFor(thread, item.extras);
  if (verdict.action === "queue") {
    toast(QUEUE_REASON[verdict.reason]);
    return false;
  }
  unsettleThreads([id], { quiet: true });
  if (verdict.action === "interruptRestart") {
    removeQueued(id, queuedId);
    return restartWith(id, item.text, item.extras ?? emptyExtras);
  }
  if (await steer(id, item.text, item.extras, item.from, item.link)) {
    removeQueued(id, queuedId);
    glideAway(id, item);
    // A delegate's message that was waiting: the turn it went into answers its link.
    linkSteered(id, item.link, item.text);
    return true;
  }
  toast(QUEUE_REASON.refused);
  return false;
}

/** How long a queued row takes to leave the stack, gliding up toward the transcript (index.css). */
const STEER_GLIDE_MS = 240;

/**
 * A queued row a steer took keeps the stack's place while it glides out, so the row itself leaves
 * instead of vanishing from under the reader; it is dropped once the glide is over.
 */
function glideAway(id: string, item: QueuedPrompt) {
  updateThread(id, (t) => departQueued(t, item));
  setTimeout(() => updateThread(id, (t) => dropDeparted(t, item.id)), STEER_GLIDE_MS);
}

/**
 * Resume: the first message a stopped or failed turn held starts a turn of its own. The rest follow
 * it, each when the turn before completes; a new message sent meanwhile waits behind them.
 */
export async function resumeQueue(id: string): Promise<boolean> {
  const thread = get().threads[id];
  if (!thread || isRunning(thread)) return false;
  const next = nextQueued(thread.queue, {});
  updateThread(id, (t) => ({ ...t, queueHeld: undefined }));
  if (!next) return false;
  removeQueued(id, next.id);
  await runPrompt(id, next.text, next.extras, {
    fromView: next.fromView,
    link: next.link,
    sender: next.from,
    resumeLinks: next.resume,
    wake: next.wake,
    wakeLinks: next.wakeLinks,
    wakeAsking: next.wakeAsking,
    // A resumed row is the app's, not the user's: the thread stays where it was in the sidebar.
    automatic: next.resume !== undefined,
  });
  return true;
}

/**
 * Open a queued message in the composer (its pencil, or ↑ at the start of an empty one). The item
 * stays where it waits, marked as being edited, and whatever the composer held comes back when the
 * edit ends. A message another thread sent cannot be edited: sending it again would lose its link.
 */
export function startQueuedEdit(id: string, queuedId: string): boolean {
  const item = get().threads[id]?.queue.find((q) => q.id === queuedId);
  if (!item || item.link || item.wake) return false;
  const before = { text: get().drafts[id] ?? "", extras: get().extras[id] ?? emptyExtras };
  set({ queuedEdit: { threadId: id, queuedId, ...before } });
  setDraft(id, item.text);
  setExtras(id, () => item.extras ?? emptyExtras);
  return true;
}

/** ⏎ or the check button on a queued message being edited: saved where it waits, never sent. */
export function saveQueuedEdit(): boolean {
  const edit = get().queuedEdit;
  if (!edit) return false;
  updateQueued(edit.threadId, edit.queuedId, get().drafts[edit.threadId] ?? "", get().extras[edit.threadId]);
  restoreComposer(edit);
  return true;
}

/** Esc or Cancel: the queued message stays as it was, and the composer gets its own message back. */
export function cancelQueuedEdit(): boolean {
  const edit = get().queuedEdit;
  if (!edit) return false;
  restoreComposer(edit);
  return true;
}

/**
 * The item left the queue while it was being edited (it started, or was removed): the edit ends. What
 * was typed stays in the composer when it was empty before the edit, and is dropped when it was not.
 */
export function endQueuedEdit(): void {
  const edit = get().queuedEdit;
  if (!edit) return;
  if (!edit.text.trim() && !hasExtras(edit.extras)) {
    set({ queuedEdit: null });
    return;
  }
  restoreComposer(edit);
}

function restoreComposer(edit: NonNullable<AppState["queuedEdit"]>): void {
  set({ queuedEdit: null });
  setDraft(edit.threadId, edit.text);
  setExtras(edit.threadId, () => edit.extras);
}

export function removeQueued(id: string, queuedId: string) {
  const link = get().threads[id]?.queue.find((q) => q.id === queuedId)?.link;
  updateThread(id, (t) => ({ ...t, queue: t.queue.filter((q) => q.id !== queuedId) }));
  if (link) queuedLinkDropped(link);
  // Editing a message that just left the queue (it started, or it was removed): the edit ends.
  if (get().queuedEdit?.queuedId === queuedId) endQueuedEdit();
}

/** A queued message edited where it waits; its attachments are replaced with the composer's. */
export function updateQueued(id: string, queuedId: string, text: string, extras?: ComposeExtras) {
  const trimmed = text.trim();
  updateThread(id, (t) => {
    const item = t.queue.find((q) => q.id === queuedId);
    if (!item) return t;
    const next = extras ?? item.extras;
    // Its attachments stay; one with attachments survives an emptied text. The text is kept as it
    // was written, since a paste is spliced at its offset when the message is composed.
    if (!trimmed && !hasExtras(next)) return { ...t, queue: t.queue.filter((q) => q.id !== queuedId) };
    return {
      ...t,
      queue: t.queue.map((q) => (q.id === queuedId ? { ...q, text, extras: next } : q)),
    };
  });
}

/** Move a queued message one place earlier (-1) or later (1). */
export function moveQueued(id: string, queuedId: string, by: -1 | 1) {
  updateThread(id, (t) => {
    const at = t.queue.findIndex((q) => q.id === queuedId);
    const to = at + by;
    if (at < 0 || to < 0 || to >= t.queue.length) return t;
    const queue = t.queue.slice();
    [queue[at], queue[to]] = [queue[to]!, queue[at]!];
    return { ...t, queue };
  });
}
