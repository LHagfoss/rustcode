// The workspace terminal (⌘J): xterm.js on a PTY the shell runs in the
// thread's project folder. One terminal per folder; hiding the panel keeps
// it running, so its scrollback and processes are still there next time.

import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { RotateCcw, SquareTerminal, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { projectName } from "../acp/types";
import { canOpenPty, openPty, type PtyHandle } from "../lib/pty";
import { cssVar, onAppThemeChange } from "../lib/theme";
import { setTerminalOpen, useApp } from "../state/store";
import { Cluster, ClusterButton } from "./primitives";

/** A solid colour for xterm from a token that may be translucent, over the panel's background. */
function solid(name: string, over: string): string {
  const probe = document.createElement("canvas").getContext("2d");
  if (!probe) return over;
  probe.fillStyle = over;
  probe.fillRect(0, 0, 1, 1);
  probe.fillStyle = cssVar(name) || over;
  probe.fillRect(0, 0, 1, 1);
  const [r, g, b] = probe.getImageData(0, 0, 1, 1).data;
  return `rgb(${r}, ${g}, ${b})`;
}

function xtermTheme() {
  const background = cssVar("--rc-chat") || "#1e1e20";
  return {
    background,
    foreground: solid("--rc-fg", background),
    cursor: solid("--rc-fg", background),
    selectionBackground: "rgba(10, 132, 255, 0.35)",
    red: cssVar("--rc-red"),
    green: cssVar("--rc-green"),
    yellow: cssVar("--rc-orange"),
    blue: cssVar("--rc-blue"),
    brightBlack: solid("--rc-fg-2", background),
  };
}

function TerminalView({ cwd, visible }: { cwd: string; visible: boolean }) {
  const host = useRef<HTMLDivElement>(null);
  const [exited, setExited] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const fitRef = useRef<FitAddon | null>(null);
  const termRef = useRef<Terminal | null>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new generation restarts the shell
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    setExited(false);
    setError(null);
    const term = new Terminal({
      fontFamily:
        '"SF Mono", "MesloLGS NF", "Symbols Nerd Font Mono", "JetBrainsMono Nerd Font", ui-monospace, Menlo, monospace',
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
    let pty: PtyHandle | null = null;
    let live = true;
    void openPty(
      cwd,
      term.cols,
      term.rows,
      (data) => term.write(data),
      () => live && setExited(true),
    ).then(
      (handle) => {
        if (!live) return handle.close();
        pty = handle;
        term.onData((data) => handle.write(data));
        term.onResize(({ cols, rows }) => handle.resize(cols, rows));
        term.focus();
      },
      (e) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    const observer = new ResizeObserver(() => {
      if (el.offsetWidth > 0) fit.fit();
    });
    observer.observe(el);
    return () => {
      live = false;
      observer.disconnect();
      pty?.close();
      term.dispose();
    };
  }, [cwd, generation]);

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
      {(exited || error) && (
        <div className="absolute right-3 bottom-2 flex items-center gap-2 rounded-full bg-fill-2 px-3 py-1 text-caption text-fg-2">
          {error ?? "The shell exited."}
          <button
            type="button"
            onClick={() => setGeneration((g) => g + 1)}
            className="flex items-center gap-1 text-brand hover:underline"
          >
            <RotateCcw size={11} /> Restart
          </button>
        </div>
      )}
    </div>
  );
}

export function TerminalPanel() {
  const open = useApp((s) => s.terminalOpen);
  const cwd = useApp((s) => (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd);
  const [opened, setOpened] = useState<string[]>([]);
  const [height, setHeight] = useState(260);

  useEffect(() => {
    if (open && cwd && !opened.includes(cwd)) setOpened((o) => [...o, cwd].slice(-4));
  }, [open, cwd, opened]);

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

  if (!opened.length) return null;
  return (
    <section
      aria-label="Terminal"
      className="relative flex shrink-0 flex-col border-hairline border-t bg-chat"
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
        <SquareTerminal size={13} className="text-fg-2" />
        <span className="font-semibold text-callout">Terminal</span>
        <span className="truncate text-caption text-fg-2">{cwd ? projectName(cwd) : ""}</span>
        <span className="flex-1" />
        <Cluster>
          <ClusterButton
            label="Hide terminal"
            tooltip="Hide terminal (⌘J)"
            onClick={() => setTerminalOpen(false)}
          >
            <X size={13} />
          </ClusterButton>
        </Cluster>
      </div>
      <div className="min-h-0 flex-1">
        {!canOpenPty() ? (
          <p className="p-3 text-callout text-fg-2">
            The terminal runs in the desktop app or `bun run dev:web`.
          </p>
        ) : (
          opened.map((dir) => <TerminalView key={dir} cwd={dir} visible={open && dir === cwd} />)
        )}
      </div>
    </section>
  );
}
