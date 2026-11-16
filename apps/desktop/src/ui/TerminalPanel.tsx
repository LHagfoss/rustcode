// The workspace terminal (⌘J): xterm.js on PTYs the shell runs in the
// thread's project folder. Several tabs per folder (state/terminalTabs.ts); hiding
// the panel or opening another thread keeps them running, so scrollback and
// processes are still there next time. Selected output goes to the composer as
// a chip (⌘L, or the button over the selection).

import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { observeSize } from "../lib/resize";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useMemo, useRef, useState } from "react";
import { projectName } from "../acp/types";
import { canOpenPty, openPty, type PtyHandle } from "../lib/pty";
import { cssVar, onAppThemeChange } from "../lib/theme";
import { onTerminalCommand, takeTerminalCommands } from "../state/projectActions";
import { setExtras, setTerminalOpen, shortcutFor, toast, useApp, useShortcutHint } from "../state/store";
import {
  activeTab,
  addTerminalTab,
  closeTerminalTab,
  markTerminalExited,
  openTerminalFolder,
  showTerminalTab,
  stepTerminalTab,
  type TerminalTab,
  terminalContext,
  titleTerminalTab,
  useTerminalTabs,
} from "../state/terminalTabs";
import { localId } from "../state/thread";
import { MessageSquarePlus, Plus, RotateCcw, SquareTerminal, X } from "./icons";
import { Cluster, ClusterButton, cx, Pill } from "./primitives";

/** A solid colour for xterm from a token that may be translucent, over the panel's background. */
function solid(name: string, over: string): string {
  const probe = document.createElement("canvas").getContext("2d");
  if (!probe) return over;
  probe.fillStyle = over;
  probe.fillRect(0, 0, 1, 1);
  probe.fillStyle = cssVar(name) || over;
  probe.fillRect(0, 0, 1, 1);
  const [r, g, b] = probe.getImageData(0, 0, 1, 1).data;
  return `rgb(${r}, ${g}, ${b})`; // design-lint: allow (resolved from a token)
}

function xtermTheme() {
  const background = cssVar("--rc-code-bg") || cssVar("--rc-bg");
  return {
    background,
    foreground: solid("--rc-text", background),
    cursor: solid("--rc-text", background),
    selectionBackground: cssVar("--rc-selection"),
    red: cssVar("--rc-danger"),
    green: cssVar("--rc-success"),
    yellow: cssVar("--rc-warning"),
    blue: cssVar("--rc-accent"),
    brightBlack: solid("--rc-text-muted", background),
  };
}

/**
 * Puts the selected output into the open thread's composer as a chip, and returns focus there.
 * The text is a fenced block naming the tab it came from (`terminalContext`).
 */
function addToMessage(selection: string, title: string): void {
  if (!selection.trim()) return;
  const key = useApp.getState().selectedId ?? "new";
  const draft = useApp.getState().drafts[key] ?? "";
  setExtras(key, (extras) => ({
    ...extras,
    attachments: [
      ...extras.attachments,
      { id: localId("paste"), kind: "text", text: terminalContext(selection, title), at: draft.length },
    ],
  }));
  requestAnimationFrame(() =>
    document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]')?.focus(),
  );
}

function TerminalView({ tab, visible }: { tab: TerminalTab; visible: boolean }) {
  const { cwd, id } = tab;
  const host = useRef<HTMLDivElement>(null);
  const [exited, setExited] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const [selected, setSelected] = useState(false);
  const fitRef = useRef<FitAddon | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const titleRef = useRef(tab.title);
  titleRef.current = tab.title;

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new generation restarts the shell
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    setExited(false);
    setError(null);
    markTerminalExited(id, false);
    const term = new Terminal({
      fontFamily:
        // macOS resolves by Menlo at the latest; the rest are Windows' and Linux's own monospace faces.
        '"SF Mono", "MesloLGS NF", "Symbols Nerd Font Mono", "JetBrainsMono Nerd Font", ui-monospace, Menlo, "Cascadia Mono", Consolas, "JetBrains Mono", "DejaVu Sans Mono", monospace',
      fontSize: 12,
      lineHeight: 1.2,
      cursorBlink: true,
      allowProposedApi: false,
      macOptionIsMeta: true,
      scrollback: 5000,
      theme: xtermTheme(),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    fit.fit();
    termRef.current = term;
    fitRef.current = fit;
    term.onTitleChange((title) => titleTerminalTab(id, title));
    term.onSelectionChange(() => setSelected(term.hasSelection()));
    // The panel's own keys while a shell has focus: ⌘L sends the selection to the message,
    // ⌘T opens a tab, ⌘⇧[ and ⌘⇧] move between tabs.
    term.attachCustomKeyEventHandler((e) => {
      // App shortcuts reach the window from a focused shell too; F6 leaving the terminal must
      // not also send its escape sequence to the shell, so the terminal skips that key.
      if (e.type === "keydown") {
        const command = shortcutFor(e);
        if (
          command === "focus-next-area" ||
          command === "focus-previous-area" ||
          command === "settle-current-thread"
        )
          return false;
      }
      if (e.type !== "keydown" || !(e.metaKey || e.ctrlKey) || e.altKey) return true;
      const key = e.key.toLowerCase();
      const take = () => {
        e.preventDefault();
        e.stopPropagation();
        return false;
      };
      if (key === "l" && e.metaKey && term.hasSelection()) {
        addToMessage(term.getSelection(), titleRef.current);
        term.clearSelection();
        return take();
      }
      if (!e.metaKey) return true;
      if (key === "t" && !e.shiftKey) {
        addTerminalTab(cwd);
        return take();
      }
      if (e.shiftKey && (e.key === "[" || e.key === "{" || e.key === "]" || e.key === "}")) {
        stepTerminalTab(cwd, e.key === "[" || e.key === "{" ? -1 : 1);
        return take();
      }
      return true;
    });
    let pty: PtyHandle | null = null;
    let live = true;
    void openPty(
      cwd,
      term.cols,
      term.rows,
      (data) => term.write(data),
      () => {
        if (!live) return;
        setExited(true);
        markTerminalExited(id, true);
      },
    ).then(
      (handle) => {
        if (!live) return handle.close();
        pty = handle;
        // A project action (state/projectActions.ts) left for this tab runs once the shell is up.
        for (const command of takeTerminalCommands(id)) handle.write(`${command}\r`);
        term.onData((data) => handle.write(data));
        term.onResize(({ cols, rows }) => handle.resize(cols, rows));
        if (visible) term.focus();
      },
      (e) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    const stopCommands = onTerminalCommand((tabId) => {
      if (tabId !== id || !pty) return;
      for (const command of takeTerminalCommands(id)) pty.write(`${command}\r`);
    });
    const stopObserving = observeSize(el, () => {
      if (el.offsetWidth > 0) fit.fit();
    });
    return () => {
      live = false;
      stopCommands();
      stopObserving();
      pty?.close();
      term.dispose();
    };
  }, [cwd, id, generation]);

  // The app's appearance changed: follow it.
  useEffect(
    () =>
      onAppThemeChange(() => {
        if (termRef.current) termRef.current.options.theme = xtermTheme();
      }),
    [],
  );

  useEffect(() => {
    if (visible) {
      requestAnimationFrame(() => {
        fitRef.current?.fit();
        termRef.current?.focus();
      });
    }
  }, [visible]);

  return (
    <div className="relative h-full" style={{ display: visible ? "block" : "none" }}>
      <div ref={host} className="h-full px-2 pt-1" />
      {selected && (
        <div className="absolute top-2 right-3">
          <Pill
            size="sm"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              const term = termRef.current;
              if (!term) return;
              addToMessage(term.getSelection(), tab.title);
              term.clearSelection();
            }}
          >
            <MessageSquarePlus size={12} /> Add to message <span className="text-fg-faint">⌘L</span>
          </Pill>
        </div>
      )}
      {(exited || error) && (
        <div className="absolute right-3 bottom-2 flex items-center gap-2 rounded-full bg-fill px-3 py-1 text-caption text-fg-muted">
          {error ?? "The shell exited."}
          <button
            type="button"
            onClick={() => setGeneration((g) => g + 1)}
            className="flex items-center gap-1 text-accent hover:underline"
          >
            <RotateCcw size={12} /> Restart
          </button>
        </div>
      )}
    </div>
  );
}

export function TerminalPanel() {
  const open = useApp((s) => s.terminalOpen);
  const terminalKey = useShortcutHint("toggle-terminal");
  const cwd = useApp((s) => (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd);
  const tabs = useTerminalTabs((s) => s.tabs);
  const active = useTerminalTabs((s) => (cwd ? activeTab(s, cwd) : undefined));
  const [height, setHeight] = useState(260);

  // A folder shown in the open panel gets its first shell. Every tab in the state stays mounted;
  // the folder cap closes the least recently used folder's tabs there (`withOpenedFolder`), once,
  // and only when the panel opens, so a hidden panel never closes a shell.
  useEffect(() => {
    if (!open || !cwd) return;
    const evicted = openTerminalFolder(cwd);
    if (evicted?.busy)
      toast(`Closed the terminals in ${projectName(evicted.cwd)}, which had a project action running.`);
  }, [open, cwd]);

  const here = useMemo(() => (cwd ? tabs.filter((t) => t.cwd === cwd) : []), [tabs, cwd]);

  const onResize = (e: React.PointerEvent) => {
    const startY = e.clientY;
    const start = height;
    const move = (ev: PointerEvent) =>
      setHeight(Math.min(window.innerHeight - 200, Math.max(120, start - (ev.clientY - startY))));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  if (!tabs.length) return null;
  return (
    <section
      aria-label="Terminal"
      data-focus-area="terminal"
      className="relative flex shrink-0 flex-col border-border border-t bg-bg"
      style={{ height, display: open ? "flex" : "none" }}
    >
      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize terminal"
        onPointerDown={onResize}
        className="absolute -top-1 right-0 left-0 z-10 h-2 cursor-row-resize"
      />
      <div className="flex h-[30px] shrink-0 items-center gap-2 px-3">
        <SquareTerminal size={13} className="shrink-0 text-fg-muted" />
        <span className="shrink-0 truncate text-caption text-fg-muted">{cwd ? projectName(cwd) : ""}</span>
        <div
          role="tablist"
          aria-label="Terminals"
          className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto"
        >
          {here.map((tab) => (
            <div
              key={tab.id}
              className={cx(
                "group flex h-6 max-w-[180px] shrink-0 items-center gap-1 rounded-full pr-1 pl-3 text-callout",
                tab.id === active?.id ? "bg-fill text-fg" : "text-fg-muted hover:bg-fill-subtle",
              )}
            >
              <button
                type="button"
                role="tab"
                aria-selected={tab.id === active?.id}
                className="min-w-0 truncate"
                onClick={() => cwd && showTerminalTab(cwd, tab.id)}
              >
                {tab.title}
              </button>
              <button
                type="button"
                aria-label={`Close ${tab.title}`}
                onClick={() => closeTerminalTab(tab.id)}
                className="shrink-0 text-fg-faint opacity-0 group-focus-within:opacity-100 group-hover:opacity-100 hover:text-fg focus-visible:opacity-100"
              >
                <X size={12} />
              </button>
            </div>
          ))}
          {cwd && canOpenPty() && (
            <ClusterButton
              label="New terminal"
              tooltip="New terminal (⌘T)"
              onClick={() => addTerminalTab(cwd)}
            >
              <Plus size={13} />
            </ClusterButton>
          )}
        </div>
        <Cluster>
          <ClusterButton
            label="Hide terminal"
            tooltip={`Hide terminal (${terminalKey})`}
            onClick={() => setTerminalOpen(false)}
          >
            <X size={13} />
          </ClusterButton>
        </Cluster>
      </div>
      <div className="min-h-0 flex-1">
        {!canOpenPty() ? (
          <p className="p-3 text-callout text-fg-muted">
            The terminal runs in the desktop app or `bun run dev:web`.
          </p>
        ) : here.length === 0 ? (
          <div className="flex h-full items-center justify-center">
            <Pill onClick={() => cwd && addTerminalTab(cwd)}>
              <Plus size={12} /> New terminal
            </Pill>
          </div>
        ) : null}
        {canOpenPty() &&
          tabs.map((tab) => <TerminalView key={tab.id} tab={tab} visible={open && tab.id === active?.id} />)}
      </div>
    </section>
  );
}
