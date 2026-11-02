// A Smart UI block in the transcript: the agent's HTML in a sandboxed frame
// with no chrome, so it reads like the rest of the reply. Copy, Show source
// and Stop/Run are small icons that appear on hover or focus. See
// docs/smart-ui.md for the format, limits and security model.

import { Check, Code, Copy, Play, Square } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "../acp/transport";
import { shellCall } from "../lib/shell";
import { cssVar, isDark, onAppThemeChange } from "../lib/theme";
import { chartSource } from "../smartui/chart";
import { claimLive, frameToken, releaseLive, SendGate } from "../smartui/host";
import { buildPage, pageTheme, usesChart, varsCss } from "../smartui/page";
import { blockProblem, blockTitle, clampHeight, MAX_ELEMENTS, viewMessage } from "../smartui/parse";
import { sendFromView, setSmartUi, toast, useApp } from "../state/store";
import { CodeBlock } from "./Markdown";
import { copyText, cx, Tooltip } from "./primitives";

const LOAD_TIMEOUT = 5000;
const BUSY_TIMEOUT = 3000;
const HIDDEN_TIMEOUT = 30_000;

function currentTheme() {
  return pageTheme(isDark() ? "dark" : "light", cssVar);
}

/** Where a page is served from in the app: its own scheme, with its own CSP (src-tauri/src/smartui.rs). */
function frameUrl(id: string): string {
  const windows = navigator.userAgent.includes("Windows");
  return windows ? `http://rcui.localhost/${id}` : `rcui://localhost/${id}`;
}

type State = { kind: "idle" } | { kind: "running" } | { kind: "failed"; reason: string };

function IconButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Tooltip label={label}>
      <button
        type="button"
        aria-label={label}
        onClick={onClick}
        className="flex size-[22px] items-center justify-center rounded-md bg-bg/80 text-fg-muted backdrop-blur hover:bg-fill hover:text-fg"
      >
        {children}
      </button>
    </Tooltip>
  );
}

export function SmartUIView({
  source,
  complete,
  streaming,
  threadId,
}: {
  source: string;
  complete: boolean;
  /** The reply is streaming in front of the reader now: a block that finishes runs on its own. */
  streaming: boolean;
  threadId?: string;
}) {
  const enabled = useApp((s) => s.smartUi);
  const [state, setState] = useState<State>({ kind: "idle" });
  const [height, setHeight] = useState(48);
  const [showSource, setShowSource] = useState(false);
  const [copied, setCopied] = useState(false);
  const [page, setPage] = useState<{ token: string; src?: string; srcdoc?: string } | null>(null);
  const frame = useRef<HTMLIFrameElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const sawStreaming = useRef(streaming && !complete);
  const instance = useRef(frameToken());
  const problem = complete ? blockProblem(source) : null;
  const title = blockTitle(source);

  if (streaming && !complete) sawStreaming.current = true;

  const stop = useCallback((reason?: string) => {
    releaseLive(instance.current);
    setPage(null);
    setState(reason ? { kind: "failed", reason } : { kind: "idle" });
  }, []);

  const run = useCallback(async () => {
    if (!threadId) return;
    const token = frameToken();
    const chart = usesChart(source) ? await chartSource() : undefined;
    const html = buildPage(source, token, currentTheme(), chart);
    claimLive(instance.current, () => stop());
    setHeight(48);
    setState({ kind: "running" });
    if (isTauri()) {
      try {
        const id = await shellCall<string>("smartui_page", { html });
        setPage({ token, src: frameUrl(id) });
      } catch (error) {
        stop(`The view could not start: ${error instanceof Error ? error.message : String(error)}`);
      }
    } else {
      setPage({ token, srcdoc: html });
    }
  }, [source, threadId, stop]);

  // A block that finished streaming in front of the reader runs on its own; others wait for Run.
  useEffect(() => {
    if (enabled && complete && !problem && sawStreaming.current && state.kind === "idle" && !page) {
      sawStreaming.current = false;
      void run();
    }
  }, [enabled, complete, problem, state.kind, page, run]);

  useEffect(() => () => releaseLive(instance.current), []);

  // Messages from the page: only from this frame, with this frame's token.
  useEffect(() => {
    if (!page) return;
    const gate = new SendGate();
    let lastSign = Date.now();
    let loaded = false;
    const onMessage = (event: MessageEvent) => {
      const data = event.data as { rc?: string; type?: string; [k: string]: unknown } | null;
      if (!frame.current || event.source !== frame.current.contentWindow || data?.rc !== page.token) return;
      loaded = true;
      lastSign = Date.now();
      switch (data.type) {
        case "size": {
          const elements = Number(data.elements);
          if (elements > MAX_ELEMENTS)
            stop(`The view built more than ${MAX_ELEMENTS.toLocaleString()} elements.`);
          else setHeight(clampHeight(Number(data.height) || 48));
          break;
        }
        case "send": {
          // A message only from a real click or key press: the page checked its own activation;
          // here the app checks the gesture reached it too (activation propagates to the app's
          // frame) and that focus is in this view.
          const gesture =
            (navigator.userActivation ? navigator.userActivation.isActive : true) &&
            document.activeElement === frame.current;
          const text = viewMessage(data.text);
          if (!gesture || !text || !threadId) return;
          if (!gate.allow()) {
            toast("The view can send one message every 2 seconds", "info");
            return;
          }
          sendFromView(threadId, text);
          break;
        }
        case "copy":
          if (typeof data.text === "string" && navigator.userActivation?.isActive !== false) {
            void copyText(data.text);
          }
          break;
      }
    };
    window.addEventListener("message", onMessage);
    // Watchdogs: a page that never starts, or stops answering (a busy loop), is stopped.
    const watch = setInterval(() => {
      const quiet = Date.now() - lastSign;
      if (!loaded && quiet > LOAD_TIMEOUT) stop("The view did not load in 5 seconds.");
      else if (loaded && quiet > BUSY_TIMEOUT)
        stop("The view stopped answering for 3 seconds (a busy loop?).");
    }, 500);
    // The app's appearance changed: the page's variables follow.
    const unTheme = onAppThemeChange(() => {
      const theme = currentTheme();
      frame.current?.contentWindow?.postMessage(
        { rc: page.token, type: "theme", css: varsCss(theme), mode: theme.mode },
        "*",
      );
    });
    return () => {
      window.removeEventListener("message", onMessage);
      clearInterval(watch);
      unTheme();
    };
  }, [page, threadId, stop]);

  // Out of sight for 30 s: torn down; its Run button brings it back.
  useEffect(() => {
    const el = box.current;
    if (!el || !page) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const observer = new IntersectionObserver(([entry]) => {
      if (entry?.isIntersecting) clearTimeout(timer);
      else timer = setTimeout(() => stop(), HIDDEN_TIMEOUT);
    });
    observer.observe(el);
    return () => {
      observer.disconnect();
      clearTimeout(timer);
    };
  }, [page, stop]);

  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);

  if (!complete) {
    return streaming ? (
      <div className="flex h-[48px] items-center text-callout">
        <span className="rc-shimmer">Building an interactive view…</span>
      </div>
    ) : (
      <Fallback source={source} reason="The view is unfinished." />
    );
  }
  if (problem) return <Fallback source={source} reason={problem} />;
  if (!enabled) {
    return (
      <div className="flex h-[36px] items-center gap-2 text-callout text-fg-muted">
        <Code size={13} />
        <span className="truncate">{title}</span>
        <span className="text-fg-faint">· Smart UI is off</span>
        <button type="button" onClick={() => setSmartUi(true)} className="text-accent hover:underline">
          Turn on Smart UI
        </button>
      </div>
    );
  }

  const running = state.kind === "running" && page;
  return (
    <div ref={box} className="group/ui relative" aria-label={title} role="group">
      {running ? (
        <iframe
          ref={frame}
          title={title}
          sandbox="allow-scripts"
          referrerPolicy="no-referrer"
          src={page.src}
          srcDoc={page.srcdoc}
          className="block w-full border-0 bg-transparent"
          style={{ height, colorScheme: "normal" }}
        />
      ) : (
        <div className="flex min-h-[44px] items-center gap-2 text-callout">
          <button
            type="button"
            disabled={!threadId}
            onClick={() => void run()}
            className="flex h-[28px] items-center gap-1.5 rounded-full bg-fill px-3 font-medium hover:bg-fill-strong"
          >
            <Play size={11} fill="currentColor" /> Run interactive view
          </button>
          <span className="truncate text-fg-muted">{title}</span>
        </div>
      )}
      {state.kind === "failed" && <p className="mt-1 text-caption text-danger">{state.reason}</p>}
      <div
        className={cx(
          "absolute top-0 right-0 flex gap-1 opacity-0 transition-opacity duration-150 group-focus-within/ui:opacity-100 group-hover/ui:opacity-100",
        )}
      >
        <IconButton
          label={copied ? "Copied" : "Copy source"}
          onClick={() => void copyText(source).then(() => setCopied(true))}
        >
          {copied ? <Check size={12} className="text-success" /> : <Copy size={12} />}
        </IconButton>
        <IconButton
          label={showSource ? "Hide source" : "Show source"}
          onClick={() => setShowSource(!showSource)}
        >
          <Code size={12} />
        </IconButton>
        {running ? (
          <IconButton label="Stop" onClick={() => stop()}>
            <Square size={10} />
          </IconButton>
        ) : (
          <IconButton label="Run" onClick={() => void run()}>
            <Play size={11} />
          </IconButton>
        )}
      </div>
      {(showSource || state.kind === "failed") && (
        <div className="mt-2">
          <CodeBlock code={source} lang="html" />
        </div>
      )}
    </div>
  );
}

function Fallback({ source, reason }: { source: string; reason: string }) {
  return (
    <div>
      <p className="mb-1 text-caption text-fg-muted">{reason} Its source:</p>
      <CodeBlock code={source} lang="html" />
    </div>
  );
}
