// An MCP App a tool result embeds, drawn in a bounded card under the call's
// row. The frame and the message boundary follow `lib/mcpApp.ts`: scripts
// only, an opaque origin, no network, and only the handshake and resizing
// answered. In the app the page comes from Smart UI's own `rcui://` server,
// whose policy allows inline script where the app's CSP would not; the mock
// and a browser use `srcdoc` with the same policy in a meta tag.

import { useEffect, useRef, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { isTauri } from "../acp/transport";
import {
  fromApp,
  handleAppMessage,
  MCP_APP_DEFAULT_HEIGHT,
  MCP_APP_SANDBOX,
  type McpApp,
  mcpAppPage,
  themeChanged,
} from "../lib/mcpApp";
import { shellCall } from "../lib/shell";
import { isDark, onAppThemeChange } from "../lib/theme";
import { frameUrl } from "../smartui/host";

type Frame = { src?: string; srcdoc?: string };

function Unavailable({ reason }: { reason: string }) {
  return (
    <div className="text-caption text-fg-faint" title={reason}>
      Interactive view unavailable · {reason}
    </div>
  );
}

export function McpAppView({ app, input, output }: { app: McpApp; input?: Json; output?: string }) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [page, setPage] = useState<Frame>();
  const [problem, setProblem] = useState<string>();
  const [height, setHeight] = useState(MCP_APP_DEFAULT_HEIGHT);
  const host = useRef({ input, output });
  host.current = { input, output };
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
    const onMessage = (event: MessageEvent) => {
      if (!fromApp(event.source, frame.current?.contentWindow)) return;
      const step = handleAppMessage(event.data, {
        theme: isDark() ? "dark" : "light",
        ...host.current,
      });
      if (step.height !== undefined) setHeight(step.height);
      for (const reply of step.replies) post(reply);
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
