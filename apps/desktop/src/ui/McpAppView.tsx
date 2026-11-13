// An MCP App a tool result embeds, drawn in a bounded card under the call's
// row. The frame and the message boundary follow `lib/mcpApp.ts`: scripts
// only, an opaque origin, no network, and only the handshake, resizing and
// approved tool calls answered. In the app the page comes from Smart UI's own
// `rcui://` server, whose policy allows inline script where the app's CSP
// would not; the mock and a browser use `srcdoc` with the same policy in a
// meta tag.

import { useEffect, useRef, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { isTauri } from "../acp/transport";
import {
  type AppToolCall,
  callReply,
  fromApp,
  handleAppMessage,
  MCP_APP_DEFAULT_HEIGHT,
  MCP_APP_SANDBOX,
  type McpApp,
  type McpAppHost,
  mayHaveApp,
  mcpAppPage,
  themeChanged,
} from "../lib/mcpApp";
import { shellCall } from "../lib/shell";
import { isDark, onAppThemeChange } from "../lib/theme";
import { frameUrl } from "../smartui/host";
import { appRouteOpen, callAppTool, readApp } from "../state/mcpAppRoute";
import type { TranscriptItem } from "../state/thread";

type Frame = { src?: string; srcdoc?: string };

function Unavailable({ reason }: { reason: string }) {
  return (
    <div className="text-caption text-fg-faint" title={reason}>
      Interactive view unavailable · {reason}
    </div>
  );
}

/** Runs a tool the app asked for on its server; it resolves with the server's result. */
export type AppCaller = (tool: string, args: Record<string, Json>) => Promise<Json>;

export function McpAppView({
  app,
  input,
  output,
  call,
}: {
  app: McpApp;
  input?: Json;
  output?: string;
  /** Present only where the app's server can be reached; without it the app is display-only. */
  call?: AppCaller;
}) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [page, setPage] = useState<Frame>();
  const [problem, setProblem] = useState<string>();
  const [height, setHeight] = useState(MCP_APP_DEFAULT_HEIGHT);
  const host = useRef<Omit<McpAppHost, "theme">>({ input, output });
  host.current = { input, output, canCall: call !== undefined };
  const caller = useRef(call);
  caller.current = call;
  const calling = useRef(false);
  const html = app.html;

  useEffect(() => {
    if (html === undefined) return;
    let live = true;
    const doc = mcpAppPage(html);
    if (isTauri()) {
      shellCall<string>("smartui_page", { html: doc })
        .then((id) => live && setPage({ src: frameUrl(id) }))
        .catch((error) => live && setProblem(error instanceof Error ? error.message : String(error)));
    } else {
      setPage({ srcdoc: doc });
    }
    return () => {
      live = false;
    };
  }, [html]);

  useEffect(() => {
    if (!page) return;
    const post = (message: unknown) => frame.current?.contentWindow?.postMessage(message, "*");
    // One call at a time: it waits for the user's approval, so a second one is refused, not queued.
    const runCall = (request: AppToolCall) => {
      const run = caller.current;
      if (!run || calling.current) {
        post(
          callReply(request.id, {
            error: run ? "another tool call is waiting" : "tool calls are unavailable",
          }),
        );
        return;
      }
      calling.current = true;
      run(request.tool, request.arguments)
        .then((result) => post(callReply(request.id, { result })))
        .catch((error) => post(callReply(request.id, { error: callError(error) })))
        .finally(() => {
          calling.current = false;
        });
    };
    const onMessage = (event: MessageEvent) => {
      if (!fromApp(event.source, frame.current?.contentWindow)) return;
      const step = handleAppMessage(event.data, {
        theme: isDark() ? "dark" : "light",
        ...host.current,
      });
      if (step.height !== undefined) setHeight(step.height);
      for (const reply of step.replies) post(reply);
      if (step.call) runCall(step.call);
    };
    window.addEventListener("message", onMessage);
    const stopTheme = onAppThemeChange(() => post(themeChanged(isDark() ? "dark" : "light")));
    return () => {
      window.removeEventListener("message", onMessage);
      stopTheme();
    };
  }, [page]);

  if (html === undefined) {
    return (
      <Unavailable reason={`the view is ${Math.ceil(app.totalBytes / 1024)} KB, over the 512 KB limit`} />
    );
  }
  if (problem) return <Unavailable reason={problem} />;
  return (
    <div
      className="max-h-[480px] overflow-y-auto rounded-md border border-border"
      role="group"
      aria-label="Interactive tool view"
    >
      {page && (
        <iframe
          ref={frame}
          title={app.uri}
          sandbox={MCP_APP_SANDBOX}
          referrerPolicy="no-referrer"
          src={page.src}
          srcDoc={page.srcdoc}
          onError={() => setProblem("the view could not load")}
          className="block w-full border-0 bg-transparent"
          style={{ height }}
        />
      )}
    </div>
  );
}

function callError(error: unknown): string {
  const data = (error as { data?: unknown } | null)?.data;
  if (typeof data === "string") return data;
  return error instanceof Error ? error.message : String(error);
}

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

/**
 * The card under a finished tool call: the app its result embedded or, on the built-in harness, the
 * one its server declares. Nothing for an ordinary result.
 */
export function McpAppSlot({ item, threadId }: { item: ToolItem; threadId?: string }) {
  const running = item.status === "pending" || item.status === "in_progress";
  const [read, setRead] = useState<{ app?: McpApp; problem?: string }>();
  const title = item.title;
  const embedded = item.app;
  useEffect(() => {
    if (embedded || running || !threadId || !mayHaveApp(title)) return;
    let live = true;
    readApp(threadId, title)
      .then((app) => live && setRead({ app }))
      .catch((error) => {
        // A server that is gone says nothing about whether it had an app, so only other failures show.
        const message = callError(error);
        if (live && !message.includes("not connected")) setRead({ problem: message });
      });
    return () => {
      live = false;
    };
  }, [embedded, running, threadId, title]);
  if (running) return null;
  const app = embedded ?? read?.app;
  if (!app && !read?.problem) return null;
  const route = threadId !== undefined && appRouteOpen(threadId);
  return (
    <div className="mt-1.5 mb-1 ml-[26px]">
      {app ? (
        <McpAppView
          app={app}
          input={item.input}
          output={item.output}
          call={route && threadId ? (tool, args) => callAppTool(threadId, title, tool, args) : undefined}
        />
      ) : (
        <Unavailable reason={read?.problem ?? "the view could not be read"} />
      )}
    </div>
  );
}
