import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "../acp/transport";
import { parseDeepLink, promptLink, threadLink } from "../lib/deepLinks";
import { setAddEnvironmentOpen, useMachines } from "./machines";
import { projectsOn, remoteRepository } from "./projects";
import {
  harnessesOn,
  newThreadChoice,
  openSettings,
  selectThread,
  setDraft,
  setNewThreadChoice,
  startNewThread,
  toast,
  useApp,
} from "./store";

export function openDeepLink(raw: string): void {
  const link = parseDeepLink(raw);
  if (!link) return;
  const s = useApp.getState();
  if (link.kind === "connect") {
    useMachines.setState({ incomingConnectCode: link.code });
    openSettings("machines");
    setAddEnvironmentOpen(true);
  } else if (link.kind === "thread") {
    if (s.sessions.some((session) => session.id === link.id) || s.threads[link.id]) {
      openSettings(null);
      selectThread(link.id);
    } else toast("Thread not found on connected environments", "error");
  } else {
    const host = s.newThreadHost;
    const project = link.repo
      ? projectsOn(s.sessions, host).find(
          (p) => remoteRepository(p.remote)?.split("/").slice(-2).join("/") === link.repo?.toLowerCase(),
        )
      : undefined;
    // A link must not reset a permission override, branch or worktree choice.
    const permissions = s.newThreadPermissionMode;
    const worktree = s.newWorktree;
    startNewThread(project?.path, host);
    useApp.setState({ newThreadPermissionMode: permissions, newWorktree: worktree });
    const snapshot = harnessesOn(s, host);
    const choice = newThreadChoice(useApp.getState());
    const available = snapshot?.harnesses.filter((h) => h.enabled && h.installed) ?? [];
    const harness =
      available.find((h) => h.id === link.harness) ?? available.find((h) => h.id === choice?.harness);
    if (harness) {
      const model = harness.models.find((m) => m.id === link.model);
      setNewThreadChoice({
        harness: harness.id,
        model: model?.id ?? (harness.id === choice?.harness ? choice.model : null),
        effort: harness.id === choice?.harness ? choice.effort : null,
      });
    }
    setDraft("new", link.prompt);
  }
}

export function copyPromptLink(): void {
  const s = useApp.getState();
  copyLink(promptLink(s.drafts[s.selectedId ?? "new"] ?? ""));
}
export function copyThreadLink(id: string): void {
  copyLink(threadLink(id));
}
function copyLink(link: string): void {
  void navigator.clipboard.writeText(link).then(
    () => toast("Link copied"),
    () => toast("Could not copy link", "error"),
  );
}

/** Listen before reading startup URLs. Hold links until the initial thread/catalog load finishes. */
export function startDeepLinks(): () => void {
  if (!isTauri()) return () => {};
  let live = true;
  let stop: (() => void) | undefined;
  const pending: string[] = [];
  const received = new Set<string>();
  let startup = true;
  const receive = (urls: string[]) => {
    for (const url of urls)
      if (!startup || !received.has(url)) {
        if (startup) received.add(url);
        pending.push(url);
      }
    drain();
  };
  const drain = () => {
    const s = useApp.getState();
    if (!live || s.status !== "ready" || !s.sessionsLoaded || !s.harnesses) return;
    for (const raw of pending.splice(0)) openDeepLink(raw);
  };
  const unsubscribe = useApp.subscribe(drain);
  void (async () => {
    stop = await listen<string[]>("deep-link://new-url", (event) => {
      receive(event.payload);
    });
    if (!live) {
      stop();
      return;
    }
    const urls = await invoke<string[] | null>("plugin:deep-link|get_current");
    receive(urls ?? []);
    startup = false;
    received.clear();
  })().catch(() => toast("Could not receive app links", "error"));
  return () => {
    live = false;
    stop?.();
    unsubscribe();
  };
}
