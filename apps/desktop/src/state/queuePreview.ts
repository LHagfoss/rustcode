// `?queuePreview` in a dev browser: stages a thread's queue so the mock can show what waits at the
// end of a thread — queued rows, a message another thread sent, a wake line, and a held queue — with
// no engine and no model turn. Like `?updatePreview` (state/updates.ts), it only fills the store.

import type { Attachment } from "./compose";
import type { Link } from "./orchestration";
import { openThread, selectThread, useApp } from "./store";
import { withWakeMark } from "./thread";

const file: Attachment = {
  id: "preview-file",
  kind: "file",
  name: "composer-focus.png",
  path: "/tmp/composer-focus.png",
};

const link: Omit<Link, "target" | "targetTitle"> = {
  id: "preview-link",
  orchestrator: "preview-parent",
  orchestratorTitle: "Sidebar polish",
  message: "The sidebar rows are ready for review.",
  state: "done",
  createdAt: Date.now() - 60_000,
};

/**
 * Fills the open thread's queue: two queued messages, a link message from another thread and a wake;
 * a held queue is the same rows as a stopped turn left them, with Resume under them. False until the
 * thread's history is there.
 */
export function stageQueuePreview(held: boolean): boolean {
  const s = useApp.getState();
  const id = s.selectedId;
  const thread = id ? s.threads[id] : undefined;
  if (!id || !thread?.loaded) return false;
  const at = Date.now();
  useApp.setState((state) => ({
    // A draft from an earlier visit to the preview would sit in the field over the staged queue.
    drafts: { ...state.drafts, [id]: "" },
    threads: {
      ...state.threads,
      [id]: {
        ...thread,
        running: !held,
        queueHeld: held || undefined,
        queue: [
          { id: "preview-1", text: "Then run the checks and tell me what failed." },
          {
            id: "preview-wake",
            text: withWakeMark(
              "Delegate finished: Review the sidebar rows\n\nFinished:\n- link_id preview-link · done · “Review the sidebar rows” · Pi · worked 3m 49s · session mock-child · 4 files changed",
            ),
            wake: true,
          },
          { id: "preview-link", text: link.message, link: link.id },
          {
            id: "preview-2",
            text: "Also check the focus ring on the composer.",
            extras: { links: ["https://docs.example.com/design"], attachments: [file] },
          },
        ],
      },
    },
    links: {
      ...state.links,
      links: [
        ...state.links.links.filter((l) => l.id !== link.id),
        { ...link, target: id, targetTitle: thread.title ?? "This thread", createdAt: at - 60_000 },
      ],
    },
  }));
  return true;
}

/**
 * Opens a thread with no turn of its own and stages the queue there: `?queuePreview` while a turn
 * runs, `?queuePreview=held` as a stopped turn left it.
 */
export function installQueuePreview(): void {
  if (!import.meta.env.DEV) return;
  const mode = new URLSearchParams(location.search).get("queuePreview");
  if (mode === null) return;
  // Screenshots stage the held queue once the app's own resume turn is done (`scripts/` drivers).
  Object.assign(window, { __queuePreview: { stage: stageQueuePreview } });
  // A thread the app resumed after a restart runs a turn of its own: the preview takes one that
  // does not, and the queue is the only thing on screen.
  const quiet = (state: ReturnType<typeof useApp.getState>) => {
    const first = state.sessions.find((x) => !x.id.startsWith("mock-thread-1")) ?? state.sessions[0];
    return first?.id;
  };
  let done = false;
  const run = () => {
    if (done) return;
    const s = useApp.getState();
    const id = quiet(s);
    if (!id) return;
    if (!s.selectedId) {
      selectThread(id);
      return;
    }
    if (!s.threads[id]) {
      void openThread(id);
      return;
    }
    // Set before the store notifies its subscribers, so filling the queue cannot stage it again.
    done = true;
    if (!stageQueuePreview(mode === "held")) {
      done = false;
      return;
    }
    off?.();
  };
  const off = useApp.subscribe(run);
  run();
}
