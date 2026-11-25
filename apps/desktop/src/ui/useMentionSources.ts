import { useMemo } from "react";
import { useShallow } from "zustand/react/shallow";
import { projectName } from "../acp/types";
import type { ThreadChoice } from "../state/compose";
import { agentStateOf, type DelegateNode, delegateTree } from "../state/delegates";
import { newThreadChoice, useApp } from "../state/store";
import { isRunning } from "../state/thread";
import { agentChoices, mentionStateOf, NO_PLANS, projectKeyOf } from "./composerMentions";

/** What the composer's `@` and `#` drawer can point at, besides the project's files and machines. */
export interface MentionSources {
  /** The open thread's project, or the folder a new thread will start in. */
  home: string | undefined;
  /** Every other named thread, marked when it shares the open thread's project. */
  threadChoices: ThreadChoice[];
  /** The threads the open one started and still runs, at any depth: the details panel's tree. */
  delegates: ThreadChoice[];
  /** Who a delegate could run on, on which model, at which effort. */
  agents: ReturnType<typeof agentChoices>;
}

/**
 * The threads, delegates and agents the composer's mention drawer lists. Running and asking threads
 * are read as id lists, so a streamed chunk in another thread does not redraw the composer. A
 * machine's harness rows carry no plan: `planCache` is this computer's `_rustcode/plans`. A
 * delegate started from the open thread inherits its harness and login, and a new thread's
 * delegate the harness and login the new thread will start on.
 */
export function useMentionSources({
  openId,
  host,
  harnesses,
  cwdForFiles,
}: {
  openId: string | undefined;
  host: string | undefined;
  harnesses: Parameters<typeof agentChoices>[0] | undefined;
  cwdForFiles: string | undefined;
}): MentionSources {
  const sessions = useApp((s) => s.sessions);
  const links = useApp((s) => s.links);
  const planCache = useApp((s) => s.planCache);
  const newHarness = useApp((s) => newThreadChoice(s)?.harness);
  const newAccount = useApp((s) => s.newThreadAccount);
  const signedOut = useApp((s) => s.signedOutLogins);
  const home = useMemo(() => {
    const open = openId ? sessions.find((s) => s.id === openId) : undefined;
    return open ? projectKeyOf(open) : cwdForFiles;
  }, [sessions, openId, cwdForFiles]);
  const working = useApp(
    useShallow((s) => Object.values(s.threads).flatMap((t) => (isRunning(t) ? [t.id] : []))),
  );
  const asking = useApp(
    useShallow((s) => Object.values(s.threads).flatMap((t) => (t.permissions.length ? [t.id] : []))),
  );
  const threadChoices = useMemo(() => {
    const workingIds = new Set(working);
    const askingIds = new Set(asking);
    return sessions.flatMap((session): ThreadChoice[] => {
      const title = session.title?.trim();
      if (!title || session.id === openId) return [];
      const project = projectKeyOf(session);
      return [
        {
          id: session.id,
          title,
          harness: session.harness,
          project: project ? projectName(project) : undefined,
          home: project === home || session.cwd === home,
          state: mentionStateOf(session, workingIds.has(session.id), askingIds.has(session.id)),
          updatedAt: session.updatedAt,
        },
      ];
    });
  }, [sessions, openId, home, working, asking]);
  const delegates = useMemo(() => {
    if (!openId) return [];
    const workingIds = new Set(working);
    const askingIds = new Set(asking);
    const found: ThreadChoice[] = [];
    const walk = (nodes: readonly DelegateNode[]) => {
      for (const node of nodes) {
        const session = sessions.find((s) => s.id === node.id);
        const title = session?.title?.trim();
        const activity = askingIds.has(node.id)
          ? "needs_input"
          : workingIds.has(node.id)
            ? "running"
            : session?.status;
        const state = agentStateOf(node.link, activity);
        if (session && title && (state === "working" || state === "needs")) {
          found.push({
            id: node.id,
            title,
            harness: session.harness,
            state,
            delegate: true,
            home: true,
            updatedAt: session.updatedAt,
          });
        }
        walk(node.children);
      }
    };
    walk(delegateTree(openId, links.links));
    return found;
  }, [openId, sessions, links, working, asking]);
  const parent = useMemo(
    () => ({
      harness: openId ? sessions.find((x) => x.id === openId)?.harness : newHarness,
      account: openId ? (sessions.find((x) => x.id === openId)?.account ?? null) : newAccount,
    }),
    [openId, sessions, newHarness, newAccount],
  );
  const plans = host ? NO_PLANS : planCache;
  const agents = useMemo(
    () => agentChoices(harnesses ?? [], plans, signedOut, parent),
    [harnesses, plans, signedOut, parent],
  );
  return { home, threadChoices, delegates, agents };
}
