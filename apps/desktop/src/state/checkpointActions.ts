// Checkpoints: listing a thread's checkpoints and turn diffs, rewinding to one, and returning
// to an earlier attempt. Split out of state/store.ts; the state it writes still lives in the app store.

import {
  type Checkpoint,
  checkpointsByTurn,
  type PatchFile,
  parseCheckpoints,
  parseRestoreAnswer,
  parseTurnDiff,
  type RestoreAnswer,
  V2Method,
} from "../acp/v2";
import { type Attempt, splitAtTurn, transcriptTurns } from "./attempts";
import {
  type AppState,
  askConfirm,
  canRewind,
  connectionFor,
  engine,
  errorText,
  openThread,
  threadIsRemote,
  toast,
  updateThread,
  useApp,
} from "./store";
import { isRunning, localId, type ThreadState, type TranscriptItem } from "./thread";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

export async function refreshCheckpoints(id: string): Promise<void> {
  // Checkpoints are files on the machine that ran the turn: this computer cannot read another's.
  const connection = connectionFor("");
  if (threadIsRemote(id) || !connection || !get().features.checkpoints) return;
  try {
    const list = parseCheckpoints(
      await connection.request(V2Method.checkpointList, { session_id: id }, 15_000),
    );
    set((s) => ({ checkpoints: { ...s.checkpoints, [id]: list } }));
  } catch {
    // A thread outside a git repository has none.
  }
}

/** What one turn changed: its checkpoint against the next (or the working tree). */
export async function turnDiff(id: string, turn: number): Promise<PatchFile[]> {
  return parseTurnDiff(await engine().request(V2Method.checkpointDiff, { session_id: id, turn }, 30_000));
}

/**
 * Rewind to before turn N: put the files back to that turn's checkpoint, and the conversation with them
 * when the harness can rewind. With `keep`, the state it abandons is kept as a checkpoint first,
 * so the attempt it leaves can be returned to. The restored files and the reloaded conversation
 * are the feedback; the caller says what happened to the transcript.
 */
async function restoreCheckpoint(
  id: string,
  turn: number,
  { rewind, keep }: { rewind: boolean; keep: boolean },
): Promise<RestoreAnswer | undefined> {
  try {
    const result = await engine().request(
      V2Method.checkpointRestore,
      { session_id: id, turn, rewind, keep },
      60_000,
    );
    const answer = parseRestoreAnswer(result);
    if (answer.conversation === "rewound") {
      // The conversation changed under the app: load it again.
      updateThread(id, (t) => ({ ...t, loaded: false }));
      set((s) => {
        const { [id]: _gone, ...threads } = s.threads;
        return { threads };
      });
      await openThread(id);
    }
    void refreshCheckpoints(id);
    return answer;
  } catch (error) {
    toast(`Could not rewind: ${errorText(error)}`, "error");
    return undefined;
  }
}

/** Whether this engine keeps the state a rewind abandons, so there is a way back to it. */
function keepsAbandonedState(): boolean {
  return !!get().agent?.checkpointAttempts;
}

/**
 * Rewind to a turn's checkpoint. The turns it leaves stay in the app as an earlier attempt, faded
 * under a divider below the transcript (`state/attempts.ts`), and the state they ended in is kept,
 * so "go back to this attempt" can put the files there again. A harness that cannot rewind its
 * conversation gets the files only, and says so: the thread stays as it is.
 */
export async function rewindCheckpoint(id: string, checkpoint: Checkpoint): Promise<void> {
  const thread = get().threads[id];
  if (!thread || isRunning(thread)) return;
  const rewind = canRewind(thread);
  const keep = rewind && keepsAbandonedState();
  const go = () => void runRewind(id, checkpoint.turn, { rewind, keep });
  if (!rewind) {
    askConfirm({
      title: `Put the files back to before turn ${checkpoint.turn}?`,
      message:
        "This harness cannot rewind the conversation, so the thread stays as it is and only the files go back. Later messages are kept.",
      action: "Files Only",
      destructive: true,
      run: go,
    });
    return;
  }
  if (!keep) {
    askConfirm({
      title: `Rewind to before turn ${checkpoint.turn}?`,
      message:
        "The files and the conversation go back to before this turn. This server cannot keep the state it leaves, so there is no way back to it.",
      action: "Rewind",
      destructive: true,
      run: go,
    });
    return;
  }
  go();
}

/** The rewind itself: what it abandons becomes an attempt, then the thread reads from the cut. */
async function runRewind(
  id: string,
  turn: number,
  { rewind, keep }: { rewind: boolean; keep: boolean },
): Promise<void> {
  const thread = get().threads[id];
  if (!thread) return;
  const tail = abandonsAt(thread, turn);
  const answer = await restoreCheckpoint(id, turn, { rewind, keep });
  if (!answer) return;
  if (answer.conversation !== "rewound") {
    toast(
      answer.rewindError
        ? `The files went back to before turn ${turn}. The conversation stayed: ${answer.rewindError}`
        : `The files went back to before turn ${turn}. The conversation stayed.`,
      answer.rewindError ? "error" : "success",
    );
    return;
  }
  if (!tail) {
    toast(`The files went back to before turn ${turn}.`, "success");
    return;
  }
  const attempt: Attempt = {
    id: localId("attempt"),
    at: Date.now(),
    turn,
    head: answer.kept,
    items: tail,
  };
  set((s) => ({ attempts: { ...s.attempts, [id]: [attempt, ...(s.attempts[id] ?? [])] } }));
}

/** The turns a rewind to `turn` abandons: from the message that started it to the transcript's end. */
function abandonsAt(thread: ThreadState, turn: number): TranscriptItem[] | undefined {
  const byEnd = checkpointsByTurn(transcriptTurns(thread.items), get().checkpoints[thread.id] ?? []);
  const end = Object.entries(byEnd).find(([, checkpoint]) => checkpoint.turn === turn)?.[0];
  if (!end) return undefined;
  return splitAtTurn(thread.items, end)?.tail;
}

/**
 * Go back to an earlier attempt: the files return to where it ended. The conversation cannot
 * follow (its messages are out of the thread's context for good), so the transcript stays as it
 * reads, and the confirm says so rather than pretending the two go together.
 */
export function returnToAttempt(threadId: string, attemptId: string): void {
  const attempt = get().attempts[threadId]?.find((a) => a.id === attemptId);
  const thread = get().threads[threadId];
  if (!attempt || attempt.head === undefined || !thread || isRunning(thread)) return;
  const head = attempt.head;
  askConfirm({
    title: "Go back to this attempt?",
    message:
      "The files go back to where this attempt ended. Its messages cannot: they are no longer in the thread's context, so the thread keeps reading as it does now.",
    action: "Restore the Files",
    run: () => {
      void restoreCheckpoint(threadId, head, { rewind: false, keep: false }).then((answer) => {
        if (answer) toast("Files restored to the earlier attempt", "success");
      });
    },
  });
}
