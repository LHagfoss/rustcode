import {
  type FormEvent,
  type KeyboardEvent,
  type PointerEvent,
  useEffect,
  useRef,
  useState,
  type WheelEvent,
} from "react";
import {
  navigateBrowserPreview,
  openBrowserPreview,
  pickBrowserElement,
  refreshBrowserPreviewState,
  scalePointerCoords,
  sendBrowserInput,
  setBrowserInspectMode,
  setBrowserStreamActive,
  setBrowserViewport,
  useBrowserPreview,
  VIEWPORT_PRESETS,
  type ViewportPreset,
} from "../state/browserPreview";
import { formatPortLabel, useListeningPorts } from "../state/ports";
import { useApp } from "../state/store";
import { splitThreadKey } from "../state/threadKeys";
import { ArrowLeft, ArrowRight, Code, Globe, RotateCw } from "./icons";
import { Callout, cx, Pill, Segments, Spinner, Tooltip } from "./primitives";

function modifierMask(e: {
  altKey?: boolean;
  ctrlKey?: boolean;
  metaKey?: boolean;
  shiftKey?: boolean;
}): number {
  return (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);
}

function mouseButtonName(button: number): "left" | "middle" | "right" {
  if (button === 1) return "middle";
  if (button === 2) return "right";
  return "left";
}

/**
 * Embedded browser preview panel backed by the thread's engine Chrome screencast
 * (ADR 0012), with navigation controls, responsive device emulation, input forwarding,
 * and sanitized element picking into the composer.
 */
export function BrowserPreviewPanel() {
  const threadId = useApp((s) => s.selectedId);
  const thread = useApp((s) => (s.selectedId ? s.threads[s.selectedId] : undefined));
  const summary = useApp((s) => (s.selectedId ? s.sessions.find((x) => x.id === s.selectedId) : undefined));
  const cwd = thread?.cwd ?? summary?.cwd;
  const isRemote = threadId ? Boolean(splitThreadKey(threadId).host) : false;
  const ports = useListeningPorts(isRemote ? undefined : cwd);

  const preview = useBrowserPreview((s) => (threadId ? s.byThread[threadId] : undefined));
  const [urlDraft, setUrlDraft] = useState(preview?.url ?? "");
  const imgRef = useRef<HTMLImageElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const lastMoveAt = useRef(0);

  useEffect(() => {
    setUrlDraft(preview?.url ?? "");
  }, [preview?.url]);

  useEffect(() => {
    if (!threadId) return;
    void refreshBrowserPreviewState(threadId);
    return () => {
      void setBrowserStreamActive(threadId, false);
    };
  }, [threadId]);

  if (!threadId) {
    return (
      <p className="px-3 py-6 text-center text-callout text-fg-muted">
        Open a thread to preview its browser.
      </p>
    );
  }

  const state = preview ?? {
    url: "",
    title: "",
    canGoBack: false,
    canGoForward: false,
    viewport: "desktop" as ViewportPreset,
    width: VIEWPORT_PRESETS.desktop.width,
    height: VIEWPORT_PRESETS.desktop.height,
    inspectMode: false,
    screencastActive: false,
    frame: null,
    loading: false,
    picking: false,
    error: null,
  };

  const frameWidth = state.frame?.width ?? state.width;
  const frameHeight = state.frame?.height ?? state.height;

  const submitUrl = (e: FormEvent) => {
    e.preventDefault();
    if (!urlDraft.trim()) return;
    if (state.url) void navigateBrowserPreview(threadId, "goto", urlDraft);
    else void openBrowserPreview(threadId, urlDraft);
  };

  const coordsOf = (e: { clientX: number; clientY: number }) => {
    const rect = imgRef.current?.getBoundingClientRect();
    if (!rect) return null;
    return scalePointerCoords(e.clientX, e.clientY, rect, frameWidth, frameHeight);
  };

  const onPointerMove = (e: PointerEvent<HTMLImageElement>) => {
    const now = performance.now();
    if (now - lastMoveAt.current < 32) return;
    lastMoveAt.current = now;
    const coords = coordsOf(e);
    if (!coords) return;
    sendBrowserInput(threadId, {
      kind: "mouse",
      type: "mouseMoved",
      x: coords.x,
      y: coords.y,
      modifiers: modifierMask(e),
    });
  };

  const onPointerDown = (e: PointerEvent<HTMLImageElement>) => {
    stageRef.current?.focus();
    const coords = coordsOf(e);
    if (!coords) return;
    if (state.inspectMode) {
      e.preventDefault();
      void pickBrowserElement(threadId, coords.x, coords.y);
      return;
    }
    sendBrowserInput(threadId, {
      kind: "mouse",
      type: "mousePressed",
      x: coords.x,
      y: coords.y,
      button: mouseButtonName(e.button),
      clickCount: e.detail || 1,
      modifiers: modifierMask(e),
    });
  };

  const onPointerUp = (e: PointerEvent<HTMLImageElement>) => {
    if (state.inspectMode) return;
    const coords = coordsOf(e);
    if (!coords) return;
    sendBrowserInput(threadId, {
      kind: "mouse",
      type: "mouseReleased",
      x: coords.x,
      y: coords.y,
      button: mouseButtonName(e.button),
      clickCount: e.detail || 1,
      modifiers: modifierMask(e),
    });
  };

  const onWheel = (e: WheelEvent<HTMLImageElement>) => {
    const coords = coordsOf(e);
    if (!coords) return;
    sendBrowserInput(threadId, {
      kind: "mouse",
      type: "mouseWheel",
      x: coords.x,
      y: coords.y,
      deltaX: e.deltaX,
      deltaY: e.deltaY,
      modifiers: modifierMask(e),
    });
  };

  const onStageKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape" && state.inspectMode) {
      e.preventDefault();
      e.stopPropagation();
      void setBrowserInspectMode(threadId, false);
      return;
    }
    if (state.inspectMode) return;
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      sendBrowserInput(threadId, { kind: "text", text: e.key });
      return;
    }
    e.preventDefault();
    sendBrowserInput(threadId, {
      kind: "key",
      type: "keyDown",
      key: e.key,
      code: e.code,
      windowsVirtualKeyCode: e.keyCode,
      modifiers: modifierMask(e),
    });
  };

  const onStageKeyUp = (e: KeyboardEvent<HTMLDivElement>) => {
    if (state.inspectMode) return;
    sendBrowserInput(threadId, {
      kind: "key",
      type: "keyUp",
      key: e.key,
      code: e.code,
      windowsVirtualKeyCode: e.keyCode,
      modifiers: modifierMask(e),
    });
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2 px-2 pb-2">
      <form onSubmit={submitUrl} className="flex items-center gap-1">
        <Tooltip label="Back">
          <button
            type="button"
            aria-label="Back"
            disabled={!state.canGoBack || state.loading}
            onClick={() => void navigateBrowserPreview(threadId, "back")}
            className="flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg disabled:opacity-40"
          >
            <ArrowLeft size={14} />
          </button>
        </Tooltip>
        <Tooltip label="Forward">
          <button
            type="button"
            aria-label="Forward"
            disabled={!state.canGoForward || state.loading}
            onClick={() => void navigateBrowserPreview(threadId, "forward")}
            className="flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg disabled:opacity-40"
          >
            <ArrowRight size={14} />
          </button>
        </Tooltip>
        <Tooltip label="Reload page">
          <button
            type="button"
            aria-label="Reload page"
            disabled={!state.url || state.loading}
            onClick={() => void navigateBrowserPreview(threadId, "reload")}
            className="flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg disabled:opacity-40"
          >
            {state.loading ? <Spinner size={12} /> : <RotateCw size={13} />}
          </button>
        </Tooltip>
        <input
          type="text"
          value={urlDraft}
          onChange={(e) => setUrlDraft(e.target.value)}
          placeholder="http://localhost:5173"
          aria-label="Preview URL"
          spellCheck={false}
          className="h-7 min-w-0 flex-1 rounded-md border border-border bg-fill-subtle px-2 font-mono text-caption text-fg outline-none focus:border-focus"
        />
        <Pill size="sm" type="submit" disabled={!urlDraft.trim() || state.loading}>
          Go
        </Pill>
      </form>

      <div className="flex flex-wrap items-center justify-between gap-2">
        <Segments
          label="Viewport size"
          value={state.viewport}
          onChange={(preset) => void setBrowserViewport(threadId, preset)}
          options={[
            { value: "desktop" as const, label: "Desktop" },
            { value: "tablet" as const, label: "Tablet" },
            { value: "mobile" as const, label: "Mobile" },
          ]}
        />
        <Tooltip label="Click an element in the preview to attach its context and screenshot to the composer">
          <Pill
            size="sm"
            prominent={state.inspectMode}
            disabled={!state.frame || state.picking}
            onClick={() => void setBrowserInspectMode(threadId, !state.inspectMode)}
          >
            {state.picking ? <Spinner size={12} /> : <Code size={12} />}
            <span>{state.inspectMode ? "Picking…" : "Pick element"}</span>
          </Pill>
        </Tooltip>
      </div>

      {state.inspectMode && (
        <div className="rounded-md bg-fill-subtle px-2 py-1.5 text-caption text-fg-muted">
          Click any element in the preview to attach it to the composer. Press Esc to cancel.
        </div>
      )}

      {state.error && <Callout tone="danger">{state.error}</Callout>}

      {state.frame ? (
        <div
          ref={stageRef}
          tabIndex={-1}
          role="application"
          aria-label="Browser preview viewport"
          onKeyDown={onStageKeyDown}
          onKeyUp={onStageKeyUp}
          className="relative flex min-h-0 flex-1 items-center justify-center overflow-auto rounded-lg border border-border bg-fill-subtle p-2 outline-none focus-visible:border-focus"
        >
          <img
            ref={imgRef}
            src={`data:image/jpeg;base64,${state.frame.data}`}
            alt={state.title || state.url || "Browser preview"}
            draggable={false}
            onPointerMove={onPointerMove}
            onPointerDown={onPointerDown}
            onPointerUp={onPointerUp}
            onWheel={onWheel}
            className={cx(
              "max-h-full max-w-full rounded border border-border object-contain select-none",
              state.inspectMode ? "cursor-crosshair" : "cursor-default",
            )}
          />
          <span className="pointer-events-none absolute right-2 bottom-2 rounded bg-bg/85 px-1.5 py-0.5 font-mono text-caption text-fg-muted tabular">
            {frameWidth}×{frameHeight}
          </span>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 rounded-lg border border-border bg-fill-subtle p-4 text-center">
          <Globe size={18} className="text-fg-muted" />
          <div className="max-w-xs space-y-1">
            <p className="font-medium text-callout text-fg">Browser preview</p>
            <p className="text-caption text-fg-muted">
              Open a local dev server or allowed URL to stream this thread&apos;s shared Chrome session and
              pick UI elements into the composer.
            </p>
          </div>
          {ports.length > 0 && (
            <div className="flex flex-wrap justify-center gap-1.5">
              {ports.map((port) => (
                <Pill
                  key={`${port.pid}:${port.port}`}
                  size="sm"
                  onClick={() => void openBrowserPreview(threadId, port.url)}
                >
                  <span aria-hidden className="size-1.5 rounded-full bg-success" />
                  <span className="font-mono">{formatPortLabel(port)}</span>
                </Pill>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
