// Settings › Diagnostics: everything needed to debug the RustCode server
// (`rustcode --acp`, run by src-tauri/src/engine.rs) on a machine you cannot
// sit at. Its status and last exit, its stderr live with a level filter and
// search, the system and web view, each harness's binary, a thread's
// debug.log, and one redacted Markdown report for a GitHub issue
// (lib/diagnostics.ts builds and redacts it).

import { Copy, Download, FileText, FolderOpen, Pause, Play, RefreshCw } from "lucide-react";
import {
  Fragment,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Method } from "../acp/types";
import {
  buildDebugReport,
  describeExit,
  filterLog,
  formatDuration,
  type HarnessDiagnostics,
  LOG_LEVELS,
  type LogLevel,
  logLevel,
  OPERATIONAL_EVENTS_SQL,
  type PageInfo,
  parseOperationalEvents,
  REPORT_LOG_LINES,
  redact,
  reportFileName,
  SERVER_STATE_LABEL,
  type ServerInfo,
  type ServerState,
  type SystemInfo,
  serverState,
} from "../lib/diagnostics";
import {
  openLogsFolder,
  openThreadLog,
  resolveBinaries,
  saveText,
  serverStatus,
  systemInfo,
} from "../lib/native";
import { hasShell } from "../lib/shell";
import { errorText, request, restartEngine, SERVER_LOG_LINES, toast, useApp } from "../state/store";
import { cx, noTextHelpers, rowInset } from "./primitives";
import { Action, fieldClass, Group, Row, Select } from "./settingsKit";

const TRANSPORT_LABEL: Record<string, string> = {
  tauri: "Run by the app",
  websocket: "Through the dev bridge",
  mock: "Mock server (no real turns)",
};

const STATE_DOT: Record<ServerState, string> = {
  running: "bg-success",
  starting: "bg-warning",
  crashed: "bg-danger",
  stopped: "bg-fg-faint",
};

const LINE_TONE: Record<LogLevel, string> = {
  error: "text-danger",
  warn: "text-warning",
  info: "text-fg-muted",
  debug: "text-fg-faint",
  trace: "text-fg-faint",
};

const LEVEL_OPTIONS = [
  { value: "all", label: "All levels" },
  { value: "error", label: "Errors" },
  { value: "warn", label: "Warnings and up" },
  { value: "info", label: "Info and up" },
  { value: "debug", label: "Debug and up" },
];

/** What the page sees of how it is drawn: scale, the GPU WebGL names, `data-platform`. */
function readPageInfo(): PageInfo {
  let gpu: string | null = null;
  try {
    const gl = document.createElement("canvas").getContext("webgl");
    if (gl) {
      const named = gl.getExtension("WEBGL_debug_renderer_info");
      gpu = String(gl.getParameter(named ? named.UNMASKED_RENDERER_WEBGL : gl.RENDERER) ?? "") || null;
      gl.getExtension("WEBGL_lose_context")?.loseContext();
    }
  } catch {
    gpu = null;
  }
  return {
    devicePixelRatio: window.devicePixelRatio,
    platform: document.documentElement.dataset.platform ?? null,
    gpu,
    viewport: `${window.innerWidth}×${window.innerHeight}`,
    userAgent: navigator.userAgent,
  };
}

/** A label and its value, as many as a card needs: the value selectable, a path free to wrap. */
function Facts({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className={cx("m-0 grid grid-cols-[10rem_1fr] gap-x-6 gap-y-1.5", rowInset)}>
      {rows.map(([label, value]) => (
        <Fragment key={label}>
          <dt className="text-callout text-fg-muted">{label}</dt>
          <dd className="selectable m-0 min-w-0 break-all font-mono text-callout text-fg">
            {value === null || value === undefined || value === "" ? (
              <span className="text-fg-faint">—</span>
            ) : (
              value
            )}
          </dd>
        </Fragment>
      ))}
    </dl>
  );
}

function useNow(every: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), every);
    return () => window.clearInterval(timer);
  }, [every]);
  return now;
}

function Uptime({ since }: { since: number }) {
  const now = useNow(1000);
  return <>{formatDuration(now - since)}</>;
}

/** The harnesses the Harnesses page knows, with where this app's PATH finds each one's binary. */
function useHarnessDiagnostics(): HarnessDiagnostics[] {
  const harnesses = useApp((s) => s.harnesses?.harnesses);
  const [paths, setPaths] = useState<(string | null)[]>([]);
  const commands = useMemo(() => (harnesses ?? []).map((h) => h.command[0] ?? ""), [harnesses]);
  useEffect(() => {
    let live = true;
    void resolveBinaries(commands).then((found) => {
      if (live) setPaths(found);
    });
    return () => {
      live = false;
    };
  }, [commands]);
  return useMemo(
    () =>
      (harnesses ?? [])
        .filter((h) => !h.builtin)
        .map((h) => ({
          id: h.id,
          name: h.name,
          enabled: h.enabled,
          installed: h.installed,
          command: h.command,
          path: paths[(harnesses ?? []).indexOf(h)] ?? null,
          version: h.version ?? null,
          auth: h.auth,
          error: h.error ?? null,
        })),
    [harnesses, paths],
  );
}

export function DiagnosticsPage() {
  const status = useApp((s) => s.status);
  const detail = useApp((s) => s.statusDetail);
  const transport = useApp((s) => s.transport);
  const [server, setServer] = useState<ServerInfo | null>(null);
  const [system, setSystem] = useState<SystemInfo | null>(null);
  const [page] = useState(readPageInfo);
  const harnesses = useHarnessDiagnostics();
  const shell = hasShell();

  // The process changes under the page (a crash, a restart): read it again while the page is open.
  useEffect(() => {
    let live = true;
    const read = () =>
      void serverStatus().then((info) => {
        if (live) setServer(info);
      });
    read();
    const timer = window.setInterval(read, 2000);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, []);
  // biome-ignore lint/correctness/useExhaustiveDependencies: a status change is when the process is new
  useEffect(() => {
    void serverStatus().then(setServer);
  }, [status]);
  useEffect(() => {
    void systemInfo().then(setSystem);
  }, []);

  const state = serverState(status, server);

  const report = useCallback(async () => {
    const events = await request(Method.dataQuery, { sql: OPERATIONAL_EVENTS_SQL, limit: 50 }, 10_000).then(
      (value) => ({ events: parseOperationalEvents(value), error: null }),
      (error) => ({ events: [], error: errorText(error) }),
    );
    const [freshServer, freshSystem] = await Promise.all([serverStatus(), systemInfo()]);
    const app = useApp.getState();
    return buildDebugReport({
      generatedAt: new Date(),
      state: serverState(app.status, freshServer ?? server),
      stateDetail: app.statusDetail ?? null,
      transport: app.transport ?? null,
      server: freshServer ?? server,
      system: freshSystem ?? system,
      page: readPageInfo(),
      harnesses,
      log: app.engineLog,
      events: events.events,
      eventsError: events.error,
    });
  }, [harnesses, server, system]);

  const copyReport = async () => {
    try {
      await navigator.clipboard.writeText(await report());
      toast("Debug report copied. Paste it into a GitHub issue.", "success");
    } catch (error) {
      toast(`Could not copy the report: ${errorText(error)}`, "error");
    }
  };
  const saveReport = async () => {
    try {
      const saved = await saveText(reportFileName(new Date()), await report());
      if (saved) toast("Debug report saved", "success");
    } catch (error) {
      toast(`Could not save the report: ${errorText(error)}`, "error");
    }
  };

  const lastExit = server?.lastExit;
  return (
    <>
      <Group
        title="Debug report"
        footer={`The server, the system, the harnesses, the last ${REPORT_LOG_LINES} server log lines and recent events as Markdown. Tokens, API keys, email addresses and your home folder are taken out; read it before you post it.`}
      >
        <Row
          title="Copy debug report"
          subtitle="Everything on this page in one block, to paste into a GitHub issue."
        >
          <Action onClick={() => void saveReport()}>
            <Download size={13} />
            Save…
          </Action>
          <Action prominent onClick={() => void copyReport()}>
            <Copy size={13} />
            Copy debug report
          </Action>
        </Row>
      </Group>

      <Group title="Server">
        <Row
          title={
            <>
              <span aria-hidden className={cx("size-2 shrink-0 rounded-full", STATE_DOT[state])} />
              {SERVER_STATE_LABEL[state]}
            </>
          }
          subtitle="The RustCode server (rustcode --acp) runs every thread. The app starts it again when it crashes."
          status={detail ?? TRANSPORT_LABEL[transport ?? ""]}
        >
          {shell && (
            <Action
              onClick={() =>
                void openLogsFolder().catch((error) =>
                  toast(`Could not open the logs folder: ${errorText(error)}`, "error"),
                )
              }
            >
              <FolderOpen size={13} />
              Open logs folder
            </Action>
          )}
          <Action onClick={() => void restartEngine()}>
            <RefreshCw size={13} />
            Restart server
          </Action>
        </Row>
        {server ? (
          <Facts
            rows={[
              ["Version", server.version],
              ["Process ID", server.pid],
              ["Uptime", server.startedAtMs ? <Uptime key="uptime" since={server.startedAtMs} /> : null],
              ["Binary", server.path],
              ["Config folder", server.configDir],
              ["Log file", server.logFile],
              ["Restarts after a crash", String(server.crashRestarts)],
              [
                "Last exit",
                lastExit
                  ? `${describeExit(lastExit)} at ${new Date(lastExit.atMs).toLocaleString()}: ${lastExit.reason}`
                  : "None since the app started",
              ],
            ]}
          />
        ) : (
          <Row
            title="Only the app sees the server's process"
            subtitle="Its process ID, binary and exits are reported by the desktop app, not in the browser."
          />
        )}
      </Group>

      <ServerLog />

      <Group title="System">
        <Facts
          rows={[
            [
              "OS",
              system
                ? [system.os, system.osVersion, system.kernel && `(kernel ${system.kernel})`]
                    .filter(Boolean)
                    .join(" ")
                : null,
            ],
            ["Architecture", system?.arch],
            ["App version", system?.appVersion],
            ["Tauri", system?.tauriVersion],
            [
              "Web view",
              system ? [system.webview, system.webviewVersion].filter(Boolean).join(" ") : page.userAgent,
            ],
            ["GPU", page.gpu],
            ["Display scale", `${page.devicePixelRatio}x`],
            ["Window", page.viewport],
            ["data-platform", page.platform],
            ["Graphics environment", system?.graphicsEnv.length ? system.graphicsEnv.join("\n") : null],
          ]}
        />
      </Group>

      <Group
        title="Harnesses"
        footer="What Settings › Harnesses last detected. The binary is where this app's PATH finds the command; the server may see a fuller PATH."
      >
        {harnesses.length ? (
          harnesses.map((h) => (
            <div key={h.id}>
              <div className={cx("flex items-center gap-2 pb-0 font-medium text-body text-fg", rowInset)}>
                {h.name}
                <span className="font-normal text-caption text-fg-faint">
                  {[!h.enabled && "off", !h.installed && "not installed"].filter(Boolean).join(", ")}
                </span>
              </div>
              <Facts
                rows={[
                  ["Binary", h.path ?? h.command[0]],
                  ["Version", h.version],
                  [
                    "Sign-in",
                    h.auth === "ok" ? "Signed in" : h.auth === "needs_login" ? "Needs sign-in" : "Unknown",
                  ],
                  [
                    "Last error",
                    h.error ? (
                      <span key="error" className="text-danger">
                        {h.error}
                      </span>
                    ) : null,
                  ],
                ]}
              />
            </div>
          ))
        ) : (
          <Row title="No harnesses listed yet" subtitle="They show once the server has answered." />
        )}
      </Group>

      <ThreadLogGroup />
    </>
  );
}

/** The server's stderr, live: a level filter, search, pause and follow, Copy and Save. */
function ServerLog() {
  const live = useApp((s) => s.engineLog);
  const [level, setLevel] = useState<LogLevel | "all">("all");
  const [query, setQuery] = useState("");
  const [paused, setPaused] = useState<string[] | null>(null);
  const lines = paused ?? live;
  const shown = useMemo(() => filterLog(lines, level, query), [lines, level, query]);
  const box = useRef<HTMLPreElement>(null);
  // Following keeps the newest line in view; a scroll up stops that until the end is reached again.
  const follow = useRef(true);
  // biome-ignore lint/correctness/useExhaustiveDependencies: new lines shown are what scroll it
  useLayoutEffect(() => {
    const el = box.current;
    if (el && follow.current && !paused) el.scrollTop = el.scrollHeight;
  }, [shown, paused]);
  const waiting = paused ? Math.max(0, live.length - paused.length) : 0;
  const home = useRef<string | null>(null);
  useEffect(() => {
    void systemInfo().then((info) => {
      home.current = info?.home ?? null;
    });
  }, []);
  const text = () => shown.join("\n");

  return (
    <Group
      title="Server log"
      footer={`The server's stderr since the app started (the last ${SERVER_LOG_LINES} lines). Each thread also logs its turns to its own logs/debug.log. Copy and Save keep only the lines shown, with secrets and your home folder taken out.`}
    >
      <div className={cx("flex flex-wrap items-center gap-2", rowInset)}>
        <Select
          label="Log level"
          value={level}
          options={LEVEL_OPTIONS}
          onChange={(value) =>
            setLevel((LOG_LEVELS as readonly string[]).includes(value) ? (value as LogLevel) : "all")
          }
        />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search the log"
          aria-label="Search the server log"
          {...noTextHelpers}
          className={cx(fieldClass, "w-44 min-w-0 flex-1 outline-none")}
        />
        <Action
          aria-pressed={paused !== null}
          onClick={() => {
            follow.current = true;
            setPaused((p) => (p ? null : [...live]));
          }}
        >
          {paused ? <Play size={13} /> : <Pause size={13} />}
          {paused ? (waiting ? `Follow (${waiting} new)` : "Follow") : "Pause"}
        </Action>
        <Action
          aria-label="Copy the log"
          onClick={() =>
            void navigator.clipboard.writeText(redact(text(), home.current)).then(
              () => toast("Server log copied", "success"),
              (error) => toast(`Could not copy: ${errorText(error)}`, "error"),
            )
          }
        >
          <Copy size={13} />
          Copy
        </Action>
        <Action
          aria-label="Save the log"
          onClick={() =>
            void saveText(reportFileName(new Date(), "server-log"), redact(text(), home.current)).then(
              (saved) => saved && toast("Server log saved", "success"),
              (error) => toast(`Could not save: ${errorText(error)}`, "error"),
            )
          }
        >
          <Download size={13} />
          Save…
        </Action>
      </div>
      <pre
        ref={box}
        onScroll={(e) => {
          const el = e.currentTarget;
          follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
        className="selectable m-0 h-[360px] overflow-auto p-3 font-mono text-caption leading-[15px] whitespace-pre-wrap break-all"
      >
        {shown.length ? (
          shown.map((line, i) => (
            <div key={i} className={LINE_TONE[logLevel(line)]}>
              {line || " "}
            </div>
          ))
        ) : (
          <span className="text-fg-faint">{lines.length ? "No line matches." : "Nothing yet."}</span>
        )}
      </pre>
    </Group>
  );
}

/** The open thread's logs/debug.log (also in each thread's menu in the sidebar). */
function ThreadLogGroup() {
  const selected = useApp((s) => s.selectedId);
  const title = useApp((s) => s.sessions.find((t) => t.id === s.selectedId)?.title);
  return (
    <Group title="Thread log">
      <Row
        title="Open thread log"
        subtitle="The open thread's logs/debug.log: each turn's rounds, tool calls, retries and errors. Every thread's menu in the sidebar has it too."
        status={selected ? `${title || "New thread"} · ${selected}` : "Open a thread first."}
      >
        <Action
          disabled={!selected || !hasShell()}
          onClick={() =>
            selected &&
            void openThreadLog(selected).catch((error) =>
              toast(`Could not open the thread log: ${errorText(error)}`, "error"),
            )
          }
        >
          <FileText size={13} />
          Open thread log
        </Action>
      </Row>
    </Group>
  );
}
