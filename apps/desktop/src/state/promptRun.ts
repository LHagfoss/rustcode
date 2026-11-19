import {
  type AcpConnection,
  ConnectionClosedError,
  type Json,
  type JsonObject,
  obj,
  RpcError,
  str,
} from "../acp/jsonrpc";
import { byActivity, type Harness, Method, parseConfigOptions, permissionCancelled } from "../acp/types";
import { afterPaint } from "../lib/frame";
import { titleFrom } from "../lib/title";
import { refreshAgents } from "./agentActions";
import { permissionModeOf } from "./approvals";
import { uploadPromptImages } from "./attachmentTransfer";
import { beginAutoCompactIdle, scheduleAutoCompact } from "./autoCompactRun";
import { refreshCheckpoints } from "./checkpointActions";
import {
  type ComposeExtras,
  carriesExtras,
  composedText,
  emptyExtras,
  promptBlocks,
  sentAttachments,
} from "./compose";
import { clearStopped, dropLimitWait, dropWaits, noteStopped, settleStopWait } from "./continueThreads";
import { contextPercent } from "./details";
import { selectOption, unavailableReason } from "./harness";
import { lastAccountFor, noteChoice, rememberModelEffort, switchThreadHarness } from "./harnessActions";
import {
  linkFinished,
  linkStarted,
  linksFinished,
  linksReopened,
  linksResumed,
  linkWaitEnded,
  requeueUnreadSteers,
  stopLinksFrom,
  takeSteeredLinks,
  threadTurnEnded,
  wakeDelivered,
} from "./host";
import { type FallbackRule, movedNotice, pickFallback, switchText } from "./limitFallback";
import type { LimitWait } from "./limitResume";
import { newThreadAccountOn } from "./logins";
import { lostTurn, reloadThread } from "./lostTurns";
import { notifyThread } from "./notify";
import { MACHINE_LOST_TURN, parseDelivered } from "./orchestration";
import { refreshPlanCache, refreshPlanUsage } from "./planUsage";
import { nextQueued } from "./queue";
import { engineBusy, FORCE_STOP_MS } from "./runState";
import { acceptsImages, removeQueued, threadHarness } from "./sendActions";
import { errorText, isReportCommand, refreshSessions, toast, updateThread, useApp } from "./store";
import {
  appendNotice,
  beginQuietTurn,
  beginTurn,
  finishTurn,
  isContinueMessage,
  isLoaded,
  isRunning,
  isWakeMessage,
  localId,
  requeueSteers,
  settleEveryWait,
  startCompaction,
  withContinueMark,
} from "./thread";
import { splitThreadKey } from "./threadKeys";
import { connectionForThread, machineOfThread, threadIsRemote } from "./threadMachines";
import {
  finishToast,
  isDelegated,
  markFinished,
  markMessaged,
  threadName,
  wakeOnWrite,
} from "./threadNotices";
import { closeThread, selectThread } from "./threadOpen";
import { sessionById } from "./threadStatus";
import { userTypedPrompt } from "./topLevel";
import { readableTurnError } from "./turnError";
import { advertisedCommands, harnessOf } from "./viewActions";

// Sending a prompt and stopping a turn (runPrompt, cancel, stopNow), the session config options,
// and moving a thread to another harness when it hits a limit. Split out of store.ts, which
// re-exports it.

// ---------------------------------------------------------------- running a turn

export async function runPrompt(
  id: string,
  text: string,
  extras: ComposeExtras = emptyExtras,
  options: {
    fromView?: boolean;
    link?: string;
    sender?: string;
    /**
     * The links a restart severed and this resume turn answers (`resumeInterrupted`): started as
     * working when its turn begins and settled when it ends, as a link's own turn leaves its link
     * (host.ts `linksResumed`, `linksFinished`).
     */
    resumeLinks?: string[];
    wake?: boolean;
    /** The links a queued wake row reports: marked as heard when its turn starts (host.ts). */
    wakeLinks?: string[];
    /** The links that row named as waiting on the user: a question, not a finish (host.ts). */
    wakeAsking?: string[];
    automatic?: boolean;
    /** A `/compact` the app sent because the context or the cache said so. */
    autoCompact?: boolean;
    /** What the "Compacting context…" divider says for that `/compact` (`autoCompactingText`). */
    autoCompactText?: string;
    /**
     * The turn that continues a thread the app was waiting for (a usage limit, an overload, the
     * internet): its wait is kept while it runs and settled when it ends.
     */
    autoResume?: boolean;
    /**
     * The turn that carries the thread on after the app compacted it: its prompt reads as a divider
     * in the transcript, never as a message the user typed (`state/thread.ts` `CONTINUE_MARK`).
     */
    continued?: boolean;
    /**
     * The next message of a thread's queue, started because the turn before it ended: work the user
     * already sent, so starting it is not a new send and does not bring a snoozed thread back
     * (`state/store.ts` `wakeOnWrite`). Unlike `continued`, it is the user's turn in every other way.
     */
    queued?: boolean;
    /**
     * The thread was made for this message, and the new-thread screen already shows its row
     * (`state/setup.ts`): it does not fly in when the thread takes the screen over.
     */
    alreadyShown?: boolean;
  } = {},
): Promise<TurnOutcome> {
  // Check again when a queued notice starts: settling may have happened after delivery.
  if (
    (options.automatic ||
      options.autoCompact ||
      options.autoResume ||
      options.continued ||
      options.wake ||
      isContinueMessage(text) ||
      isWakeMessage(text)) &&
    (useApp.getState().statusMarks[id] ?? sessionById(useApp.getState(), id)?.status) === "done"
  )
    return "skipped";
  // The turn runs on the thread's own computer, which may have gone away since it was opened.
  const conn = connectionForThread(id);
  if (!conn) {
    toast(threadIsRemote(id) ? "That computer is not connected." : "The server is not connected.", "error");
    return "skipped";
  }
  const thread = useApp.getState().threads[id];
  // The user starting a turn again settles any stop they made before it.
  if (!options.automatic) clearStopped(id);
  // The prompt reads as a divider both ways: live, and after a reload from the history the engine
  // kept, which carries the text and not this flag (state/thread.ts `CONTINUE_MARK`).
  if (options.continued) text = withContinueMark(text);
  // A prompt no agent asked for is the user's: the thread is theirs now, even
  // when it was started by an agent (state/topLevel.ts). A resume after a
  // restart, a wake or a message from another thread is not a message the user
  // sent, and neither is the turn that continues after a wait.
  if (userTypedPrompt(options, text)) markMessaged(id);
  // Whoever starts a turn now moots the wait, and if this one stops the same way it waits again.
  if (!options.autoResume && !options.autoCompact && !isRunning(thread)) dropWaits(id);
  // Never a second prompt into a running turn: an external harness takes it as the next turn and can
  // interrupt the tools it is still running (a parent's wait_session). It waits in the queue, like a
  // message typed meanwhile, and starts when this turn ends.
  if (isRunning(thread)) {
    // An automatic compaction never waits in the queue: the next turn may run
    // before it, and the guards will offer another one in a later idle period.
    if (options.autoCompact) return "skipped";
    updateThread(id, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        {
          id: localId("queued"),
          text,
          extras: carriesExtras(extras) ? extras : undefined,
          fromView: options.fromView,
          link: options.link,
          resume: options.resumeLinks,
          wake: options.wake,
          wakeLinks: options.wakeLinks,
          wakeAsking: options.wakeAsking,
        },
      ],
    }));
    return "skipped";
  }
  // A user turn starts a new idle period, which may auto-compact once more. The app's own continue is
  // not one: it is the same idle period as the compaction that asked for it, and re-arming the rule
  // that just ran would let a harness whose compaction left the context over the threshold compact
  // and continue for ever.
  if (!options.autoCompact && !options.continued) beginAutoCompactIdle(id);
  // Writing in a thread brings it back from a snooze, as it always has: that is the user asking for
  // it (`submit` and `steerOrSubmit` have already done it for a message they took). A turn the app
  // starts — a delegate's wake, a resume after a restart, the next row of the queue (`queued`), the
  // turn after a usage limit — is not a new send and leaves the snooze alone, so a sleeping thread
  // keeps working in the background without coming back to the list.
  if (userTypedPrompt(options, text) && !options.queued) wakeOnWrite(id);
  const startIndex = thread?.items.length ?? 0;
  if (options.link) linkStarted(options.link);
  // The turn a restart resumes this thread with answers the links the quit severed: they work again
  // from here, so the parent reads this turn's answer and its time, never the time before the quit.
  if (options.resumeLinks?.length) {
    linksResumed(options.resumeLinks);
    // Links a usage limit settled as failed work again: this turn is the one that answers them.
    linksReopened(id, options.resumeLinks);
  }
  // A queued wake reports its links now, as its turn starts: late enough that a restart before this
  // point brings the wake back, early enough that nothing in the turn reports it again.
  if (options.wake && options.wakeLinks?.length) wakeDelivered(options.wakeLinks, options.wakeAsking ?? []);
  // The message the transcript shows: the command, links and typed text, with each long paste back
  // where it was pasted. The prompt's blocks say the same thing (below).
  const shown = composedText(text, extras);
  // A message typed here flies out of the composer on its first sighting; one the new-thread screen
  // already drew does not (lib/sendFlight.ts).
  const sent = userTypedPrompt(options, text) && !options.fromView && !options.alreadyShown;
  // The message is on screen before its blocks are built: a long paste or a 20 MB picture is copied
  // into the prompt's JSON after the bubble lands, not before it — and that update gets a painted
  // frame of its own, so the bubble's row (and the flight it starts) is drawn before the work that
  // follows it, which on a fast harness is the whole rest of the turn's first frame.
  updateThread(id, (t) =>
    options.autoCompact
      ? // The turn writes no rows, so "Compacting context…" is the one sign the thread is busy.
        startCompaction(beginQuietTurn(t), options.autoCompactText, "auto-compact")
      : beginTurn(
          t,
          shown,
          Date.now(),
          sentAttachments(extras),
          options.fromView,
          options.wake,
          sent,
          options.continued,
        ),
  );
  await afterPaint();
  const harness = threadHarness(thread);
  const prompt = promptBlocks(text, extras, {
    images: acceptsImages(thread),
    builtin: harness?.builtin ?? true,
    cwd: thread?.cwd ?? "",
    advertised: advertisedCommands(useApp.getState(), id),
  });
  // A new prompt starts a new report, if it is one.
  if (useApp.getState().report?.threadId === id) useApp.setState({ report: null });
  if (!options.autoCompact) {
    useApp.setState((s) => ({
      sessions: byActivity(
        s.sessions.map((x) =>
          x.id === id
            ? { ...x, updatedAt: Date.now(), title: x.title ?? (titleFrom(shown, 80) || "Image") }
            : x,
        ),
      ),
    }));
  }
  let outcome: "completed" | "cancelled" | "failed" = "completed";
  let failure: string | undefined;
  let unused: string[] = [];
  try {
    const sent = threadIsRemote(id) ? await remotePromptImages(conn, id, prompt) : prompt;
    const request: JsonObject = { sessionId: splitThreadKey(id).id, prompt: sent };
    // The access level rides on every prompt: a harness that sandboxes its own shell (Codex) leaves
    // the sandbox for Full access, on this machine and on a remote one alike.
    request._meta = {
      rustcode: {
        access: permissionModeOf(useApp.getState(), id),
        ...(options.sender && parseDelivered(text)
          ? {
              origin: {
                kind: "agent",
                threads: [
                  {
                    sessionId: options.sender,
                    title: parseDelivered(text)?.title ?? "",
                    ...(options.link ? { linkId: options.link } : {}),
                  },
                ],
              },
            }
          : {}),
        ...(options.autoCompact ? { autoCompact: true } : {}),
      },
    };
    const result = (await conn.request(Method.sessionPrompt, request)) as JsonObject | null;
    if (str(result?.stopReason) === "cancelled") outcome = "cancelled";
    const meta = (result?._meta as JsonObject | undefined)?.rustcode as JsonObject | undefined;
    unused = Array.isArray(meta?.unusedSteers)
      ? meta.unusedSteers.filter((x): x is string => typeof x === "string")
      : [];
  } catch (error) {
    // A turn on another machine outlives the request that asked for it: the request dies with the
    // machine's connection, and the turn there goes on. Nothing is settled from the rows that had
    // arrived — a half-written answer is not an answer — so the links are handed to `lostTurn`: it
    // asks the machine how the thread ended once its channel is back, and gives them that answer
    // then, or their failure after `LOST_MACHINE_MS` (issue 3440). Anything else is the engine's own
    // refusal, and a failure as it always was.
    if (error instanceof ConnectionClosedError && threadIsRemote(id)) {
      // The channel's close usually ended the turn already (`watchMachineConnection`); when a
      // closed channel is what refused the request, this is what ends it.
      if (isRunning(useApp.getState().threads[id]))
        updateThread(id, (t) => finishTurn(t, "failed", Date.now(), MACHINE_LOST_TURN));
      lostTurn(id, [options.link, ...(options.resumeLinks ?? []), ...takeSteeredLinks(id)]);
      return "skipped";
    }
    outcome = "failed";
    failure = errorText(error);
  }
  // A provider's raw answer (`Codex: {"detail":"Bad Request"}`) is said in words, with what to do
  // when the conversation is full; the raw text stays on the thread, behind the error's Details.
  const rawFailure = failure;
  if (failure !== undefined)
    failure = readableTurnError(failure, contextPercent(useApp.getState().threads[id]?.usage));
  updateThread(id, (t) => ({
    ...requeueSteers(finishTurn(t, outcome, Date.now(), failure, options.autoCompact), unused),
    ...(rawFailure !== failure ? { errorDetail: rawFailure } : {}),
  }));
  // Steers the turn never read wait as turns of their own, and carry the links they came from.
  if (!options.autoCompact) requeueUnreadSteers(id, unused);
  // The stop is settled first: a turn it stopped is reported as the stop, not twice.
  // An automatic compaction must leave a pending wait alone, not settle it.
  const waiting = options.autoCompact
    ? (useApp.getState().limitWaits[id] ?? null)
    : // The stop is read from what the harness said, not the words the thread shows.
      settleStopWait(id, outcome, rawFailure, startIndex, [options.link, ...(options.resumeLinks ?? [])]);
  // A thread waiting for the internet is held, not dead: it says so in the composer's row.
  const offline = options.autoCompact ? undefined : useApp.getState().internetWaits[id];
  // An automatic compaction is not a turn the user waits for: it is not "Done"
  // news, and it must not wake the thread's parent as if its work finished.
  if (!options.autoCompact) {
    if (outcome === "completed") markFinished(id);
    else if (outcome === "failed") markFinished(id, Date.now(), true);
  }
  // A report command that answered in the panel leaves no turn behind: its message and footer go.
  const reported = useApp.getState().report?.threadId === id;
  if (reported && outcome === "completed" && isReportCommand(id, text, extras)) {
    updateThread(id, (t) => ({ ...t, items: t.items.slice(0, startIndex) }));
  }
  if (options.link)
    linkFinished(
      options.link,
      id,
      outcome,
      failure,
      useApp.getState().threads[id]?.items.slice(startIndex) ?? [],
    );
  // The links this resume turn answers settle together: one write, one wake tick (host.ts).
  if (options.resumeLinks?.length)
    linksFinished(
      options.resumeLinks,
      id,
      outcome,
      failure,
      useApp.getState().threads[id]?.items.slice(startIndex) ?? [],
      // Stopped on a limit or an overload again: the parent heard it once and waits for the end.
      !!waiting,
    );
  // Every finished turn of a thread matters to a parent waiting on it: a middle thread's real
  // answer is the one after the work it started (state/host.ts threadTurnEnded).
  if (!options.autoCompact) {
    threadTurnEnded(
      id,
      !!options.wake,
      outcome,
      failure,
      useApp.getState().threads[id]?.items.slice(startIndex) ?? [],
    );
  }
  // An automatic compaction must not move the thread in the sidebar: no list
  // refresh that would read the turn as new activity.
  if (!options.autoCompact) void refreshSessions();
  void refreshPlanUsage(id);
  // A window's numbers move with the turn, and so does the account's own row:
  // the thread on screen asks again as it ends.
  if (id === useApp.getState().selectedId) void refreshPlanCache();
  void refreshAgents(id);
  void refreshCheckpoints(id);
  // A delegate stopped on a plan limit may go on elsewhere at once (issue 3392); the move says so.
  const move = waiting && !options.autoCompact ? limitMoveFor(id, waiting) : null;
  const moved = move && waiting ? await switchOnLimit(id, waiting, move) : false;
  if (move && !moved) {
    notifyThread(
      { threadId: id, kind: "limit", key: String(waiting!.until), title: threadName(id) },
      useApp.getState(),
    );
  }
  if (
    !moved &&
    !options.autoCompact &&
    !options.continued &&
    (outcome === "completed" || outcome === "failed")
  ) {
    finishToast(id, outcome, { waiting, offline, link: options.link });
  }
  // A queued message starts at once, so "working" stays up across the hand-off. A turn that was
  // stopped or failed leaves the queue held instead (thread.ts `holdsQueue`): the rows read Held and
  // Resume starts them.
  // Nor does one go out into a wait: it waits for the turn that continues.
  const after = useApp.getState().threads[id];
  const next = after && !waiting && !moved && !offline ? nextQueued(after.queue, after) : undefined;
  if (next) {
    removeQueued(id, next.id);
    await runPrompt(id, next.text, next.extras, {
      fromView: next.fromView,
      link: next.link,
      resumeLinks: next.resume,
      wake: next.wake,
      wakeLinks: next.wakeLinks,
      wakeAsking: next.wakeAsking,
      automatic: next.resume !== undefined,
      // The row was typed before this turn: starting it is not a new send (`runPrompt` above).
      queued: true,
    });
  }
  if (outcome === "completed" && !options.continued) scheduleAutoCompact(id);
  return outcome;
}

/** How a turn ended, and `skipped` for a prompt that never became one (a queued one, a compaction that found a turn running). */
export type TurnOutcome = "completed" | "cancelled" | "failed" | "skipped";

export async function cancel(id = useApp.getState().selectedId): Promise<void> {
  if (!id) return;
  const conn = connectionForThread(id);
  // A stop that finds nothing to send it on says so, instead of doing nothing.
  if (!conn) {
    toast(threadIsRemote(id) ? "That computer is not connected." : "The server is not connected.", "error");
    return;
  }
  const thread = useApp.getState().threads[id];
  // Stopping a thread stops the work it gave other threads (M1); turns the user started there go on.
  void stopLinksFrom(id);
  // The engine may run a turn this window lost track of (`engineRunning`, the list's status): Stop
  // still stops it.
  const engineRuns =
    !!useApp.getState().engineRunning[id] ||
    (!isLoaded(thread) && engineBusy(sessionById(useApp.getState(), id)?.status));
  if (!isRunning(thread) && !engineRuns) return;
  // Recorded first: a quit inside the harness's cancel grace would otherwise leave the thread
  // listed as running, and the restart would resume a turn the user stopped.
  noteStopped(id);
  try {
    conn.notify(Method.sessionCancel, { sessionId: splitThreadKey(id).id });
    // ACP: after `session/cancel` the client answers outstanding permission requests as cancelled.
    for (const request of thread?.permissions ?? []) conn.respond(request.rpcId, permissionCancelled);
    if (thread) {
      updateThread(id, (t) => settleEveryWait({ ...t, permissions: [] }, Date.now()));
      if (thread.permissions.length) linkWaitEnded(id);
    }
  } catch (error) {
    toast(errorText(error), "error");
    return;
  }
  void forceStopIfStuck(id, thread?.turn?.startedAt);
}

/**
 * Stops a thread's turn and waits for it to end, forcing it after a short wait: `cancel` for a
 * turn that obeys, closing the thread in the engine for one that does not. True once it runs nothing.
 */
export async function stopNow(id: string, waitMs = 3_000): Promise<boolean> {
  await cancel(id);
  const until = Date.now() + waitMs;
  while (isRunning(useApp.getState().threads[id]) && Date.now() < until)
    await new Promise((r) => setTimeout(r, 100));
  if (isRunning(useApp.getState().threads[id])) {
    await closeThread(id);
    updateThread(id, (t) => (isRunning(t) ? finishTurn(t, "cancelled", Date.now()) : t));
  }
  return !isRunning(useApp.getState().threads[id]);
}

/** The stops waiting to be forced: one per thread, however many times Stop is pressed. */
export const forcingStops = new Set<string>();

/**
 * `session/cancel` is a notification: nothing says it landed. A harness that ignores it, or a turn
 * wedged on a tool, would leave Stop doing nothing for good. When the turn is still the same one
 * after `FORCE_STOP_MS`, the thread is closed in the engine (which ends its harness) and the window
 * settles it as stopped and reads it again, so the thread can be used and closed (issue 3652).
 */
async function forceStopIfStuck(id: string, turnStartedAt: number | undefined): Promise<void> {
  if (forcingStops.has(id)) return;
  forcingStops.add(id);
  try {
    await new Promise((resolve) => setTimeout(resolve, FORCE_STOP_MS));
    const now = useApp.getState();
    const thread = now.threads[id];
    // Ended, or a new turn began since: nothing is stuck.
    const sameTurn = isRunning(thread) && thread.turn?.startedAt === turnStartedAt;
    const engineStill = !isRunning(thread) && !!now.engineRunning[id];
    if (!sameTurn && !engineStill) return;
    toast("The turn did not stop, so it was ended.", "info");
    await closeThread(id);
    updateThread(id, (t) => (isRunning(t) ? finishTurn(t, "cancelled", Date.now()) : t));
    useApp.setState((s) => {
      const { [id]: _gone, ...rest } = s.engineRunning;
      return _gone ? { engineRunning: rest } : {};
    });
    if (useApp.getState().agent?.closeSession) void reloadThread(id);
  } finally {
    forcingStops.delete(id);
  }
}

/** Below this a prompt fits one frame of a paired computer's connection (1 MiB), with room to spare. */
const INLINE_PROMPT_CHARS = 900_000;

/**
 * A remote thread's images go ahead of its prompt, in chunks under the connection's 1 MiB frame
 * (state/attachmentTransfer.ts), and the prompt names each by the file the machine kept. A machine
 * on a RustCode from before the upload takes them inline, only when the prompt fits a frame.
 */
async function remotePromptImages(conn: AcpConnection, id: string, prompt: Json[]): Promise<Json[]> {
  if (!prompt.some((block) => obj(block)?.type === "image")) return prompt;
  try {
    return await uploadPromptImages(
      (method, params, timeout) => conn.request(method, params, timeout),
      prompt,
    );
  } catch (error) {
    if (!(error instanceof RpcError && error.isMethodNotFound)) throw error;
    if (JSON.stringify(prompt).length < INLINE_PROMPT_CHARS) return prompt;
    throw new Error(
      `${machineOfThread(id) ?? "That computer"} runs an older RustCode that cannot take an image this large from another computer. Update RustCode there, or send a smaller image.`,
    );
  }
}

/**
 * Sets a config option on a thread. `remember` (the default) mirrors a model or effort into the
 * harness's next new-thread choice and the model's own effort memory: the user picked it here.
 * A turn's own `configure_session`, and the apply after `session/new` (whose choice the picker
 * already noted), write their options with `remember: false`, so an agent never rewrites the
 * user's memory.
 */
export async function setConfigOption(
  id: string,
  configId: string,
  value: string | boolean,
  options: { remember?: boolean } = {},
): Promise<void> {
  // A remote thread's options are its machine's to list and set.
  const conn = connectionForThread(id);
  if (!conn) {
    toast("That computer is not connected.", "error");
    return;
  }
  const params: JsonObject =
    typeof value === "boolean"
      ? { sessionId: splitThreadKey(id).id, configId, type: "boolean", value }
      : { sessionId: splitThreadKey(id).id, configId, value };
  try {
    const result = (await conn.request(Method.sessionSetConfigOption, params, 15_000)) as JsonObject | null;
    if (result?.configOptions) {
      updateThread(id, (t) => ({ ...t, configOptions: parseConfigOptions(result.configOptions) }));
    }
    // A model or effort picked in a thread is also its harness's next new-thread choice; the
    // engine keeps it (docs/harnesses.md), the app only mirrors it, and the model keeps the
    // effort it was left on for the next time it is picked.
    const thread = useApp.getState().threads[id];
    if (thread && options.remember !== false && (configId === "model" || configId === "effort")) {
      const current = (option: string) => thread.configOptions.find((o) => o.id === option)?.current ?? null;
      noteChoice(thread.harness?.id ?? "rustcode", { model: current("model"), effort: current("effort") });
      rememberModelEffort(thread.harness?.id ?? "rustcode", current("model"), current("effort"));
    }
  } catch (error) {
    toast(`Could not change ${configId}: ${errorText(error)}`, "error");
  }
}

// ---------------------------------------------------------------- switching at a limit

// The harnesses a thread left on a limit, by thread: a row never sends it back to one of them, so two
// providers at their limits do not hand it to each other. Memory only: a restart starts afresh.
const leftOnLimit = new Map<string, Set<string>>();

interface LimitMove {
  rule: FallbackRule;
  fromName: string;
  to: Harness;
}

/**
 * Where a delegate that just stopped on a plan limit would go on instead of waiting (issue 3392), or
 * null: the setting is off, the stop is not a limit, the thread is not a delegate of this machine, or
 * no row of the table names an available harness for what it ran on.
 */
export function limitMoveFor(id: string, wait: LimitWait): LimitMove | null {
  const s = useApp.getState();
  if (!s.limitFallback || (wait.reason ?? "limit") !== "limit") return null;
  if (threadIsRemote(id) || !isDelegated(id)) return null;
  const ran = harnessOf(s, id);
  const model = selectOption(s.threads[id]?.configOptions ?? [], "model")?.current;
  const rule = pickFallback(s.limitFallbackMap, { harness: ran, model }, leftOnLimit.get(id));
  if (!rule) return null;
  const listed = s.harnesses?.harnesses ?? [];
  const to = listed.find((h) => h.id === rule.to.harness);
  if (!to || unavailableReason(to)) return null;
  return { rule, fromName: listed.find((h) => h.id === ran)?.name ?? ran ?? "The harness", to };
}

/**
 * Move a limited delegate to the table's model and carry on there with the turn it was in, in the
 * same way `continueDueThreads` carries on after a reset: the links the stop settled are reopened and
 * answered by it. The thread says which model it moved to and why, and the engine records the move
 * in its activity. False when the move did not happen; the thread then waits for the reset as before.
 */
async function switchOnLimit(id: string, wait: LimitWait, move: LimitMove): Promise<boolean> {
  const { rule, fromName, to } = move;
  const from = harnessOf(useApp.getState(), id);
  const account =
    rule.to.account ?? newThreadAccountOn(to, lastAccountFor(to.id), useApp.getState().signedOutLogins);
  const moved = await switchThreadHarness(id, to.id, account, {
    model: rule.to.model,
    effort: rule.to.effort,
    reason: "limit",
    quiet: true,
  });
  if (!moved) return false;
  if (from) leftOnLimit.set(id, new Set([...(leftOnLimit.get(id) ?? []), from]));
  dropLimitWait(id);
  const options = useApp.getState().threads[id]?.configOptions ?? [];
  const modelOption = selectOption(options, "model");
  const model =
    modelOption?.choices.find((c) => c.value === modelOption.current)?.name ?? modelOption?.current;
  const target = model ? `${to.name} · ${model}` : to.name;
  updateThread(id, (t) => appendNotice(t, movedNotice(fromName, target), "info"));
  toast(`“${threadName(id)}” hit its limit and moved to ${target}`, "info", {
    action: { label: "Open", run: () => selectThread(id) },
  });
  void runPrompt(id, switchText(fromName, target), undefined, {
    automatic: true,
    autoResume: true,
    resumeLinks: wait.links,
  });
  return true;
}
