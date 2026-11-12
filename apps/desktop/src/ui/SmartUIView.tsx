// A Smart UI block in the transcript: the agent's HTML in a sandboxed frame
// with no chrome, so it reads like the rest of the reply. Copy, Show source
// and Stop/Run are small icons that appear on hover or focus. See
// docs/smart-ui.md for the format, limits and security model.

import { Check, Code, Copy, Play, Square } from "lucide-react";
import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { isTauri } from "../acp/transport";
import { KEYBOARD_NAV } from "../lib/focusModality";
import { openExternal } from "../lib/native";
import { shellCall } from "../lib/shell";
import { cssVar, isDark, onAppThemeChange } from "../lib/theme";
import { chartSource } from "../smartui/chart";
import {
  AutoRun,
  claimLive,
  clearHalt,
  frameToken,
  HoverSize,
  haltOf,
  haltView,
  releaseLive,
  rememberedHeight,
  rememberHeight,
  SendGate,
  StreamPump,
} from "../smartui/host";
import { buildPage, buildStreamPage, pageTheme, previewable, usesChart, varsCss } from "../smartui/page";
import {
  blockProblem,
  blockTitle,
  clampHeight,
  closeOpenTags,
  MAX_ELEMENTS,
  MIN_HEIGHT,
  viewMessage,
} from "../smartui/parse";
import { sendFromView, setSmartUi, toast, useApp } from "../state/store";
import { CodeBlock } from "./Markdown";
import { copyText, cx, Pill, Tooltip } from "./primitives";

const LOAD_TIMEOUT = 5000;
const BUSY_TIMEOUT = 3000;
const HIDDEN_TIMEOUT = 30_000;
/** A view starts this far before it scrolls into sight, so it is drawn when it arrives. */
const SEEN_MARGIN = "200px";

function currentTheme() {
  // The app's fonts on every platform, macOS included: a theme that sets one is
  // the theme the view's text follows (issue 3304).
  return pageTheme(isDark() ? "dark" : "light", cssVar, {
    sans: cssVar("--font-sans"),
    mono: cssVar("--font-mono"),
  });
}

/** Where a page is served from in the app: its own scheme, with its own CSP (src-tauri/src/smartui.rs). */
function frameUrl(id: string): string {
  const windows = navigator.userAgent.includes("Windows");
  return windows ? `http://rcui.localhost/${id}` : `rcui://localhost/${id}`;
}

/** The scroller around a view: an observer's margin only applies to its own root. */
function scrollParent(el: Element): Element | null {
  for (let parent = el.parentElement; parent; parent = parent.parentElement) {
    const overflow = getComputedStyle(parent).overflowY;
    if (overflow === "auto" || overflow === "scroll") return parent;
  }
  return null;
}

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
        className="flex size-[22px] items-center justify-center rounded-md bg-bg/80 text-fg-muted hover:bg-fill hover:text-fg"
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
  /** The reply is streaming in front of the reader now. */
  streaming: boolean;
  threadId?: string;
}) {
  const enabled = useApp((s) => s.smartUi);
  const [height, setHeight] = useState(() => rememberedHeight(source) ?? MIN_HEIGHT);
  const [showSource, setShowSource] = useState(false);
  const [copied, setCopied] = useState(false);
  const [page, setPage] = useState<{
    token: string;
    src?: string;
    srcdoc?: string;
    /** The fragment is still arriving: this page takes `stream` messages and runs no script. */
    stream?: boolean;
  } | null>(null);
  const [inView, setInView] = useState(false);
  // The frame's own colour scheme, so its native pieces (a scrollbar, a control) and
  // a canvas fall back to the mode the app is in, not the system's.
  const [mode, setMode] = useState<"light" | "dark">(() => (isDark() ? "dark" : "light"));
  const [, haltChanged] = useReducer((n: number) => n + 1, 0);
  const frame = useRef<HTMLIFrameElement>(null);
  const box = useRef<HTMLDivElement>(null);
  // The height the frame is drawn at, with the hover hold (`HoverSize`).
  const sizes = useRef<HoverSize | null>(null);
  const sizeGate = useCallback(
    () => (sizes.current ??= new HoverSize(rememberedHeight(source) ?? MIN_HEIGHT)),
    [source],
  );

  const syncFrameFocusModality = useCallback(() => {
    if (!page) return;
    frame.current?.contentWindow?.postMessage(
      {
        rc: page.token,
        type: "focus-modality",
        keyboard: document.documentElement.hasAttribute(KEYBOARD_NAV),
      },
      "*",
    );
  }, [page]);

  useEffect(() => {
    if (!page) return;
    // The iframe is a separate document: mirror parent modality changes across the boundary.
    const observer = new MutationObserver(syncFrameFocusModality);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: [KEYBOARD_NAV],
    });
    syncFrameFocusModality();
    return () => observer.disconnect();
  }, [page, syncFrameFocusModality]);
  const autoRun = useRef(new AutoRun());
  // Each start or stop outdates a start still waiting on Chart.js or the shell.
  const attempt = useRef(0);
  const starting = useRef(false);
  const instance = useRef(frameToken());
  const problem = complete ? blockProblem(source) : null;
  const title = blockTitle(source);
  const haltKey = `${threadId ?? ""}\n${source}`;
  const halt = complete ? haltOf(haltKey) : undefined;
  const runnable = enabled && complete && !problem;
  // A view whose fence is still open, being read now: it is drawn as it arrives.
  // One that names the chart library is not: a preview runs no script, so its
  // canvases would be empty boxes, and the chart is the content (see `previewable`).
  const streamable = enabled && !complete && streaming && source.trim().length > 0 && previewable(source);
  const latest = useRef(source);
  const pump = useRef<StreamPump | null>(null);
  // A preview that could not start waits for the finished view instead of retrying on every byte.
  const streamFailed = useRef(false);

  const tearDown = useCallback(() => {
    attempt.current++;
    starting.current = false;
    pump.current = null;
    releaseLive(instance.current);
    setPage(null);
  }, []);

  /** Stop and failure stay after the row unmounts: only pressing Run brings the view back. */
  const haltWith = useCallback(
    (reason?: string) => {
      haltView(haltKey, reason ? { kind: "failed", reason } : { kind: "stopped" });
      haltChanged();
      tearDown();
    },
    [haltKey, tearDown],
  );

  const run = useCallback(async () => {
    if (!threadId) return;
    const mine = ++attempt.current;
    starting.current = true;
    const show = (next: NonNullable<typeof page>) => {
      starting.current = false;
      setPage(next);
    };
    const token = frameToken();
    let chart: string | undefined;
    if (usesChart(source)) {
      try {
        chart = await chartSource();
      } catch (error) {
        // Without the library every canvas stays blank with no reason on screen.
        if (mine === attempt.current)
          haltWith(
            `The chart library could not be loaded: ${error instanceof Error ? error.message : String(error)}`,
          );
        return;
      }
    }
    if (mine !== attempt.current) return;
    const html = buildPage(source, token, currentTheme(), chart);
    claimLive(
      instance.current,
      () => {
        autoRun.current.evicted();
        tearDown();
      },
      () => autoRun.current.inView,
    );
    const remembered = rememberedHeight(source);
    if (remembered !== undefined) {
      sizeGate().set(remembered);
      setHeight(remembered);
    }
    if (isTauri()) {
      try {
        const id = await shellCall<string>("smartui_page", { html });
        if (mine === attempt.current) show({ token, src: frameUrl(id) });
      } catch (error) {
        if (mine === attempt.current)
          haltWith(`The view could not start: ${error instanceof Error ? error.message : String(error)}`);
      }
    } else {
      show({ token, srcdoc: html });
    }
  }, [source, threadId, tearDown, haltWith, sizeGate]);

  const pressRun = () => {
    clearHalt(haltKey);
    haltChanged();
    void run();
  };

  /** The page a still-streaming block is drawn in; the fragment arrives over `stream` messages. */
  const startStream = useCallback(async () => {
    if (!threadId || streamFailed.current) return;
    const mine = ++attempt.current;
    starting.current = true;
    const token = frameToken();
    const html = buildStreamPage(token, currentTheme());
    const start = (next: NonNullable<typeof page>) => {
      starting.current = false;
      pump.current = new StreamPump((fragment) => {
        frame.current?.contentWindow?.postMessage({ rc: token, type: "stream", html: fragment }, "*");
      });
      setPage(next);
    };
    claimLive(
      instance.current,
      () => {
        autoRun.current.evicted();
        tearDown();
      },
      () => autoRun.current.inView,
    );
    try {
      if (isTauri()) {
        const id = await shellCall<string>("smartui_page", { html });
        if (mine === attempt.current) start({ token, src: frameUrl(id), stream: true });
      } else {
        start({ token, srcdoc: html, stream: true });
      }
    } catch {
      // The finished view still lands normally; drawing nothing until then beats halting.
      streamFailed.current = true;
      starting.current = false;
      releaseLive(instance.current);
    }
  }, [threadId, tearDown]);

  // What is in sight, for the whole life of the block, running or not.
  useEffect(() => {
    const el = box.current;
    if (!(runnable || streamable) || !el) return;
    const gate = autoRun.current;
    const observer = new IntersectionObserver(
      (entries) => {
        const seen = entries.at(-1)?.isIntersecting ?? false;
        gate.see(seen);
        setInView(seen);
      },
      { root: scrollParent(el), rootMargin: SEEN_MARGIN },
    );
    observer.observe(el);
    return () => {
      observer.disconnect();
      gate.see(false);
      setInView(false);
    };
  }, [runnable, streamable]);

  // In sight it runs by itself, unless stopped, failed or just evicted (`AutoRun`).
  useEffect(() => {
    if (runnable && inView && !halt && !page && !starting.current && autoRun.current.ready) void run();
  }, [runnable, inView, halt, page, run]);

  // A block still streaming in front of the reader is drawn as it arrives.
  useEffect(() => {
    if (streamable && inView && !page && !starting.current && autoRun.current.ready) void startStream();
  }, [streamable, inView, page, startStream]);

  // Each new fragment goes to the streaming frame, the last one winning (throttled by `StreamPump`).
  useEffect(() => {
    latest.current = source;
    if (page?.stream) pump.current?.push(closeOpenTags(source));
  }, [source, page]);

  // The fence closed: the finished page replaces the preview, and its script runs once.
  useEffect(() => {
    if (page?.stream && (complete || !streaming)) {
      latest.current = source;
      tearDown();
    }
  }, [complete, streaming, page, source, tearDown]);

  // Smart UI turned off: nothing draws the frame, so its watchdog must not call it a failure.
  useEffect(() => {
    if (!enabled && page) tearDown();
  }, [enabled, page, tearDown]);

  useEffect(
    () => () => {
      attempt.current++;
      releaseLive(instance.current);
    },
    [],
  );

  // Messages from the page: only from this frame, with this frame's token.
  useEffect(() => {
    if (!page) return;
    const gate = new SendGate();
    const links = new SendGate(1000);
    let lastSign = Date.now();
    let loaded = false;
    const onMessage = (event: MessageEvent) => {
      const data = event.data as { rc?: string; type?: string; [k: string]: unknown } | null;
      if (!frame.current || event.source !== frame.current.contentWindow || data?.rc !== page.token) return;
      loaded = true;
      lastSign = Date.now();
      switch (data.type) {
        case "stream-ready":
          // The preview frame can take the fragment now, including the part it was built after.
          pump.current?.push(closeOpenTags(latest.current));
          break;
        case "hover": {
          // Track whether measurements should be ignored while the pointer is inside.
          const next = sizeGate().hover(data.over === true);
          if (next !== null) {
            rememberHeight(source, next);
            setHeight(next);
          }
          break;
        }
        case "size": {
          const elements = Number(data.elements);
          const measured = clampHeight(Number(data.height) || MIN_HEIGHT);
          if (page.stream) {
            // Never shrink while streaming, so the chat does not jump as the fragment grows.
            const next = Math.max(sizeGate().value, measured);
            sizeGate().set(next);
            setHeight(next);
          } else if (elements > MAX_ELEMENTS)
            haltWith(`The view built more than ${MAX_ELEMENTS.toLocaleString()} elements.`);
          else {
            // A hover overlay may have moved the content; a click or key may open a panel.
            // The page says when it was pressed: an arrow key on a tab grants the app no activation.
            const interacted = data.interacted === true || (navigator.userActivation?.isActive ?? false);
            const next = sizeGate().measure(measured, interacted);
            if (next !== null) {
              rememberHeight(source, next);
              setHeight(next);
            }
          }
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
        case "open": {
          // A link in the view (a source, a reference) opens in the browser: only a web or
          // mail address, only from a real click on this view, at most one a second.
          const gesture =
            (navigator.userActivation ? navigator.userActivation.isActive : true) &&
            document.activeElement === frame.current;
          const url = typeof data.url === "string" ? data.url.trim() : "";
          if (!gesture || !/^(https?:\/\/|mailto:)/i.test(url) || !links.allow()) return;
          openExternal(url);
          break;
        }
        case "copy":
          if (typeof data.text === "string" && navigator.userActivation?.isActive !== false) {
            void copyText(data.text);
          }
          break;
        case "focus-modality":
          document.documentElement.toggleAttribute(KEYBOARD_NAV, data.keyboard === true);
          break;
        case "error": {
          // A page that threw used to be left on screen, blank where its content should be.
          const reason =
            typeof data.message === "string" && data.message ? data.message : "its script failed";
          haltWith(`The view's script failed: ${reason}`);
          break;
        }
      }
    };
    window.addEventListener("message", onMessage);
    // Watchdogs: a page that never starts, or stops answering (a busy loop), is stopped.
    const watch = setInterval(() => {
      // A hidden window throttles the page's heartbeat; that is not a failure, and a failure sticks.
      if (document.hidden) lastSign = Date.now();
      const quiet = Date.now() - lastSign;
      if (!loaded && quiet > LOAD_TIMEOUT) haltWith("The view did not load in 5 seconds.");
      else if (loaded && quiet > BUSY_TIMEOUT)
        haltWith("The view stopped answering for 3 seconds (a busy loop?).");
    }, 500);
    // The app's appearance changed: the page's variables follow.
    const unTheme = onAppThemeChange(() => {
      const theme = currentTheme();
      setMode(theme.mode);
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
  }, [page, threadId, source, haltWith, sizeGate]);

  // Out of sight for 30 s: torn down; it runs again when it comes back.
  useEffect(() => {
    if (!page || inView) return;
    const timer = setTimeout(tearDown, HIDDEN_TIMEOUT);
    return () => clearTimeout(timer);
  }, [page, inView, tearDown]);

  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);

  if (!complete) {
    if (!streaming) return <Fallback source={source} reason="The view is unfinished." />;
    // The fragment is drawn as it arrives; when the fence closes the finished page replaces it.
    return (
      <div ref={box} className="group/ui relative" aria-label={title} role="group">
        {page?.stream ? (
          <iframe
            ref={frame}
            title={title}
            sandbox="allow-scripts"
            referrerPolicy="no-referrer"
            onFocus={syncFrameFocusModality}
            onLoad={syncFrameFocusModality}
            src={page.src}
            srcDoc={page.srcdoc}
            className="block w-full border-0 bg-transparent"
            style={{ height, colorScheme: mode }}
          />
        ) : (
          <div className="flex h-[48px] items-center text-callout">
            <span className="rc-shimmer">Building an interactive view…</span>
          </div>
        )}
      </div>
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

  return (
    <div ref={box} className="group/ui relative" aria-label={title} role="group">
      {page ? (
        <iframe
          ref={frame}
          title={title}
          sandbox="allow-scripts"
          referrerPolicy="no-referrer"
          onFocus={syncFrameFocusModality}
          onLoad={syncFrameFocusModality}
          src={page.src}
          srcDoc={page.srcdoc}
          className="block w-full border-0 bg-transparent"
          style={{ height, colorScheme: mode }}
        />
      ) : halt ? (
        <div className="flex min-h-[44px] items-center gap-2 text-callout">
          <Pill disabled={!threadId} onClick={pressRun}>
            <Play size={11} fill="currentColor" /> Run interactive view
          </Pill>
          <span className="truncate text-fg-muted">{title}</span>
        </div>
      ) : (
        <div className="truncate text-callout text-fg-muted" style={{ height }}>
          {title}
        </div>
      )}
      {halt?.kind === "failed" && <p className="mt-1 text-caption text-danger">{halt.reason}</p>}
      <div
        className={cx(
          "absolute top-0 right-0 flex gap-1 opacity-0 group-focus-within/ui:opacity-100 group-hover/ui:opacity-100",
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
        {page ? (
          <IconButton label="Stop" onClick={() => haltWith()}>
            <Square size={10} />
          </IconButton>
        ) : (
          <IconButton label="Run" onClick={pressRun}>
            <Play size={11} />
          </IconButton>
        )}
      </div>
      {(showSource || halt?.kind === "failed") && (
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
