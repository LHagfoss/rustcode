// Settings, full window, after SettingsView.swift: a section list on the
// left and grouped rows on the right. Engine-backed pages (Harnesses,
// Connections, Skills, MCP) read and write through `_rustcode/*` requests.

import {
  Blocks,
  ChevronLeft,
  Settings as Gear,
  Info,
  KeyRound,
  Layers,
  Monitor,
  Moon,
  Palette,
  Sparkles,
  Sun,
  TerminalSquare,
} from "lucide-react";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { arr, bool, type Json, type JsonObject, num, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { pickFolder } from "../lib/native";
import { validateDefault } from "../state/harness";
import {
  APP_VERSION,
  type NewThreadChoice,
  openSettings,
  request,
  restartEngine,
  type SettingsSection,
  setDefaultHarness,
  setOnboarding,
  setProjectCwd,
  setTheme,
  toast,
  useApp,
} from "../state/store";
import { DefaultsForm, HarnessList } from "./Harnesses";
import { ChromeButton, cx, Kbd, Pill, Spinner, Switch } from "./primitives";

const SECTIONS: { id: SettingsSection; label: string; icon: typeof Gear }[] = [
  { id: "general", label: "General", icon: Gear },
  { id: "harnesses", label: "Harnesses", icon: Layers },
  { id: "connections", label: "Connections", icon: KeyRound },
  { id: "skills", label: "Skills", icon: Sparkles },
  { id: "mcp", label: "MCP Servers", icon: Blocks },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "engine", label: "Engine", icon: TerminalSquare },
];

function Group({ title, children, footer }: { title?: string; children: ReactNode; footer?: ReactNode }) {
  return (
    <section className="mb-[18px]">
      {title && <h3 className="mb-1.5 px-1 font-semibold text-callout text-fg-2">{title}</h3>}
      <div className="divide-y divide-hairline overflow-hidden rounded-xl bg-fg/[0.045]">{children}</div>
      {footer && <p className="mt-1.5 px-1 text-caption text-fg-2">{footer}</p>}
    </section>
  );
}

function Row({
  title,
  subtitle,
  children,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="flex min-h-11 items-center gap-4 px-3.5 py-[11px]">
      <div className="min-w-0 flex-1">
        <div className="text-body">{title}</div>
        {subtitle && <div className="line-clamp-3 text-caption text-fg-2">{subtitle}</div>}
      </div>
      {children}
    </div>
  );
}

function useEngineData<T>(method: string, parse: (v: Json) => T) {
  const status = useApp((s) => s.status);
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    try {
      setData(parse(await request(method)));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [method, parse]);
  useEffect(() => {
    if (status === "ready") void load();
  }, [status, load]);
  return { data, error, setData, reload: load };
}

function Loading({ error }: { error: string | null }) {
  return error ? (
    <p className="text-attention text-callout">{error}</p>
  ) : (
    <div className="flex items-center gap-2 text-callout text-fg-2">
      <Spinner /> Loading…
    </div>
  );
}

// ---------------------------------------------------------------- pages

function General() {
  const projectCwd = useApp((s) => s.projectCwd);
  const shortcuts: [string, string][] = [
    ["New thread", "⌘N"],
    ["Switch thread", "⌘K"],
    ["Settings", "⌘,"],
    ["Toggle sidebar", "⌘B"],
    ["Stop the turn", "Esc / ⌘."],
    ["Send · queue while running", "⏎"],
    ["Steer the running turn", "⌘⏎"],
    ["New line", "⇧⏎"],
    ["Answer an approval", "1–9"],
  ];
  return (
    <>
      <Group title="New threads">
        <Row title="Default project folder" subtitle={projectCwd || "None chosen"}>
          <Pill onClick={() => void pickFolder(projectCwd).then((p) => p && setProjectCwd(p))}>Choose…</Pill>
        </Row>
        <Row title="Harness, model and effort" subtitle="Which agent runs a new thread by default.">
          <Pill onClick={() => openSettings("harnesses")}>Harnesses…</Pill>
        </Row>
      </Group>
      <Group title="Keyboard">
        {shortcuts.map(([label, keys]) => (
          <Row key={label} title={label}>
            <Kbd>{keys}</Kbd>
          </Row>
        ))}
      </Group>
    </>
  );
}

function Harnesses() {
  const snapshot = useApp((s) => s.harnesses);
  const [choice, setChoice] = useState<NewThreadChoice | null>(null);
  if (!snapshot) return <Loading error={null} />;
  const value = choice ?? snapshot.default;
  const problem = validateDefault(snapshot, value);
  return (
    <>
      {snapshot.unsupported && (
        <p className="mb-4 rounded-xl bg-attention/10 px-3.5 py-2.5 text-callout">
          This engine does not run other harnesses yet (no{" "}
          <code className="font-mono">_rustcode/harnesses</code>). Threads use RustCode's built-in harness.
          Update RustCode to add Codex, Claude Code, Gemini and others.
        </p>
      )}
      <Group
        title="Harnesses"
        footer="Each harness signs in on its own; RustCode never stores its credentials."
      >
        <HarnessList snapshot={snapshot} />
      </Group>
      {!snapshot.unsupported && (
        <section className="mb-[18px]">
          <h3 className="mb-1.5 px-1 font-semibold text-callout text-fg-2">Default for new threads</h3>
          <DefaultsForm snapshot={snapshot} value={value} onChange={setChoice} />
          <div className="mt-2.5 flex gap-2">
            <Pill
              prominent
              disabled={!choice || !!problem}
              onClick={() =>
                void setDefaultHarness(value).then((saved) => {
                  if (!saved) return;
                  setChoice(null);
                  toast("Default saved", "success");
                })
              }
            >
              Save default
            </Pill>
            <Pill onClick={() => setOnboarding(true)}>Set up again…</Pill>
          </div>
        </section>
      )}
    </>
  );
}

interface Connections {
  github?: {
    cliInstalled: boolean;
    accounts: { host: string; login: string; active: boolean; valid: boolean }[];
    error?: string;
  };
  accounts: { provider: string; display: string; method: string; active: boolean; needsSignIn: boolean }[];
}

const parseConnections = (v: Json): Connections => {
  const root = obj(v) ?? {};
  const gh = obj(root.github);
  return {
    github: gh
      ? {
          cliInstalled: bool(gh.cliInstalled) ?? false,
          accounts: arr(gh.accounts).map((a) => {
            const o = obj(a) ?? {};
            return {
              host: str(o.host) ?? "",
              login: str(o.login) ?? "",
              active: bool(o.active) ?? false,
              valid: bool(o.valid) ?? false,
            };
          }),
          error: str(gh.error),
        }
      : undefined,
    accounts: arr(root.accounts).map((a) => {
      const o = obj(a) ?? {};
      return {
        provider: str(o.provider) ?? "",
        display: str(o.display) ?? "",
        method: str(o.method) ?? "",
        active: bool(o.active) ?? false,
        needsSignIn: bool(o.needsSignIn) ?? false,
      };
    }),
  };
};

function ConnectionsPage() {
  const { data, error } = useEngineData(Method.connections, parseConnections);
  if (!data) return <Loading error={error} />;
  return (
    <>
      <Group title="GitHub" footer="RustCode uses the GitHub CLI's sign-in for pull requests.">
        {!data.github?.cliInstalled ? (
          <Row title="GitHub CLI not installed" subtitle="Install it with: brew install gh" />
        ) : data.github.accounts.length ? (
          data.github.accounts.map((a) => (
            <Row
              key={`${a.host}/${a.login}`}
              title={a.login}
              subtitle={`${a.host}${a.active ? " · active" : ""}${a.valid ? "" : " · token invalid"}`}
            />
          ))
        ) : (
          <Row title="Not signed in" subtitle="Run: gh auth login" />
        )}
      </Group>
      <Group title="Model accounts" footer="Sign in or switch accounts with `rustcode auth` in a terminal.">
        {data.accounts.length ? (
          data.accounts.map((a, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: accounts can repeat a provider
            <Row
              key={i}
              title={a.display || a.provider}
              subtitle={`${a.provider} · ${a.method}${a.needsSignIn ? " · sign in again" : ""}`}
            >
              {a.active && <span className="text-caption text-success">Active</span>}
            </Row>
          ))
        ) : (
          <Row title="No accounts" />
        )}
      </Group>
    </>
  );
}

interface Capabilities {
  skills: { name: string; description: string; source: string; enabled: boolean }[];
  mcpServers: {
    name: string;
    transport: string;
    target: string;
    enabled: boolean;
    connection: string;
    tools: number;
    error?: string;
  }[];
  configPath?: string;
}

const parseCapabilities = (v: Json): Capabilities => {
  const root = obj(v) ?? {};
  return {
    configPath: str(root.configPath),
    skills: arr(root.skills).map((s) => {
      const o = obj(s) ?? {};
      return {
        name: str(o.name) ?? "",
        description: str(o.description) ?? "",
        source: str(o.source) ?? "",
        enabled: bool(o.enabled) ?? false,
      };
    }),
    mcpServers: arr(root.mcpServers).map((s) => {
      const o = obj(s) ?? {};
      return {
        name: str(o.name) ?? "",
        transport: str(o.transport) ?? "",
        target: str(o.target) ?? "",
        enabled: bool(o.enabled) ?? false,
        connection: str(o.connection) ?? "",
        tools: num(o.tools) ?? 0,
        error: str(o.error),
      };
    }),
  };
};

function useCapabilities() {
  const state = useEngineData(Method.capabilities, parseCapabilities);
  const set = async (kind: "skill" | "mcp", name: string, enabled: boolean) => {
    try {
      state.setData(
        parseCapabilities(await request(Method.capabilitiesSet, { kind, name, enabled }, 30_000)),
      );
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "warning");
    }
  };
  return { ...state, set };
}

function SkillsPage() {
  const { data, error, set } = useCapabilities();
  const [filter, setFilter] = useState("");
  if (!data) return <Loading error={error} />;
  const on = data.skills.filter((s) => s.enabled).length;
  const shown = data.skills.filter((s) =>
    `${s.name} ${s.description}`.toLowerCase().includes(filter.toLowerCase()),
  );
  return (
    <>
      <input
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        placeholder="Filter skills"
        aria-label="Filter skills"
        className="mb-3 h-[30px] w-full rounded-full bg-fill-3 px-3.5 text-callout outline-none placeholder:text-fg-3"
      />
      <Group title={`${on} of ${data.skills.length} on`}>
        {shown.map((s) => (
          <Row key={`${s.source}/${s.name}`} title={s.name} subtitle={`${s.source} · ${s.description}`}>
            <Switch
              label={`Use ${s.name}`}
              checked={s.enabled}
              onChange={(v) => void set("skill", s.name, v)}
            />
          </Row>
        ))}
      </Group>
    </>
  );
}

function McpPage() {
  const { data, error, set, setData } = useCapabilities();
  const [name, setName] = useState("");
  const [target, setTarget] = useState("");
  const [adding, setAdding] = useState(false);
  if (!data) return <Loading error={error} />;
  const connected = data.mcpServers.filter((s) => s.connection === "connected").length;
  const add = async () => {
    setAdding(true);
    try {
      const isHttp = /^https?:\/\//.test(target.trim());
      const params: JsonObject = {
        name: name.trim(),
        target: isHttp ? [target.trim()] : target.trim().split(/\s+/),
        transport: isHttp ? "http" : "stdio",
      };
      setData(parseCapabilities(await request(Method.mcpAdd, params, 30_000)));
      setName("");
      setTarget("");
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "warning");
    } finally {
      setAdding(false);
    }
  };
  const remove = async (server: string) => {
    try {
      setData(parseCapabilities(await request(Method.mcpRemove, { name: server }, 15_000)));
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "warning");
    }
  };
  return (
    <>
      <Group
        title={`${connected} of ${data.mcpServers.length} connected`}
        footer={data.configPath ? `Saved in ${data.configPath}` : undefined}
      >
        {data.mcpServers.map((s) => (
          <Row
            key={s.name}
            title={
              <span className="flex items-center gap-2">
                <span
                  className={cx(
                    "size-[7px] rounded-full",
                    s.connection === "connected"
                      ? "bg-success"
                      : s.connection === "failed"
                        ? "bg-danger"
                        : "bg-fg-4",
                  )}
                />
                {s.name}
              </span>
            }
            subtitle={s.error ?? `${s.transport} · ${s.target}${s.tools ? ` · ${s.tools} tools` : ""}`}
          >
            <button
              type="button"
              onClick={() => void remove(s.name)}
              className="text-caption text-fg-2 hover:text-danger"
            >
              Remove
            </button>
            <Switch
              label={`Use ${s.name}`}
              checked={s.enabled}
              onChange={(v) => void set("mcp", s.name, v)}
            />
          </Row>
        ))}
      </Group>
      <Group title="Add a server">
        <div className="flex flex-col gap-2 p-3.5">
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Name"
            aria-label="Server name"
            className="h-[30px] rounded-lg bg-field px-3 text-callout outline-none ring-1 ring-hairline"
          />
          <input
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            placeholder="Command (npx some-server) or https:// URL"
            aria-label="Server command or URL"
            className="h-[30px] rounded-lg bg-field px-3 font-mono text-callout outline-none ring-1 ring-hairline"
          />
          <Pill
            prominent
            className="self-end"
            disabled={!name.trim() || !target.trim() || adding}
            onClick={() => void add()}
          >
            Add Server
          </Pill>
        </div>
      </Group>
    </>
  );
}

function Appearance() {
  const theme = useApp((s) => s.theme);
  const tiles: { id: typeof theme; label: string; icon: typeof Sun }[] = [
    { id: "system", label: "System", icon: Monitor },
    { id: "light", label: "Light", icon: Sun },
    { id: "dark", label: "Dark", icon: Moon },
  ];
  return (
    <Group title="Appearance">
      <div className="flex gap-3 p-3.5" role="radiogroup" aria-label="Appearance">
        {tiles.map((t) => (
          <button
            key={t.id}
            type="button"
            role="radio"
            aria-checked={theme === t.id}
            onClick={() => setTheme(t.id)}
            className="flex flex-col items-center gap-1.5"
          >
            <span
              className={cx(
                "flex h-[62px] w-24 items-center justify-center rounded-lg ring-2",
                t.id === "dark"
                  ? "bg-[#1e1e20] text-white/80"
                  : t.id === "light"
                    ? "bg-white text-black/70"
                    : "bg-gradient-to-r from-white from-50% to-[#1e1e20] to-50% text-fg-2",
                theme === t.id ? "ring-brand" : "ring-hairline",
              )}
            >
              <t.icon size={18} />
            </span>
            <span className={cx("text-callout", theme === t.id ? "text-fg" : "text-fg-2")}>{t.label}</span>
          </button>
        ))}
      </div>
    </Group>
  );
}

function Engine() {
  const status = useApp((s) => s.status);
  const detail = useApp((s) => s.statusDetail);
  const transport = useApp((s) => s.transport);
  const log = useApp((s) => s.engineLog);
  return (
    <>
      <Group title="Engine">
        <Row
          title={status === "ready" ? "Running" : status === "connecting" ? "Starting…" : "Stopped"}
          subtitle={
            detail ??
            (transport === "tauri"
              ? "rustcode --acp, run by the app"
              : transport === "websocket"
                ? "rustcode --acp through the dev bridge"
                : "Mock engine (no real turns)")
          }
        >
          <Pill onClick={() => void restartEngine()}>Restart</Pill>
        </Row>
        <Row title="App version" subtitle={`RustCode desktop ${APP_VERSION}`}>
          <Info size={14} className="text-fg-3" />
        </Row>
      </Group>
      <Group title="Engine log" footer="The engine's stderr; turns log to each thread's logs/debug.log.">
        <pre className="selectable m-0 max-h-[320px] overflow-auto p-3 font-mono text-caption leading-[15px] whitespace-pre-wrap text-fg-2">
          {log.length ? log.slice(-200).join("\n") : "Nothing yet."}
        </pre>
      </Group>
    </>
  );
}

export function Settings() {
  const section = useApp((s) => s.settings) ?? "general";
  const current = SECTIONS.find((s) => s.id === section) ?? SECTIONS[0]!;
  return (
    <div className="fixed inset-0 z-30 flex bg-chat" role="dialog" aria-label="Settings">
      <nav className="flex w-[240px] shrink-0 flex-col bg-sidebar" aria-label="Settings sections">
        <div data-tauri-drag-region className="flex h-[34px] items-end pr-2.5 pl-20">
          <ChromeButton label="Back to chat" tooltip="Back (Esc)" onClick={() => openSettings(null)}>
            <ChevronLeft size={16} />
          </ChromeButton>
        </div>
        <div className="px-5 pt-4 pb-1 font-semibold text-caption text-fg-2">Settings</div>
        <div className="flex flex-col gap-0.5 px-2.5">
          {SECTIONS.map((s) => (
            <button
              key={s.id}
              type="button"
              aria-current={s.id === section ? "page" : undefined}
              onClick={() => openSettings(s.id)}
              className={cx(
                "flex items-center gap-2 rounded-[9px] px-2 py-1.5 text-left text-body",
                s.id === section ? "bg-fg/10" : "hover:bg-fg/[0.035]",
              )}
            >
              <span className="flex w-[22px] justify-center">
                <s.icon size={14} className={s.id === section ? "text-brand" : "text-fg-2"} />
              </span>
              {s.label}
            </button>
          ))}
        </div>
      </nav>
      <div className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-[720px] px-[clamp(24px,5%,64px)] pb-16">
          <h2 className="pt-10 pb-3 font-semibold text-title2">{current.label}</h2>
          {section === "general" && <General />}
          {section === "harnesses" && <Harnesses />}
          {section === "connections" && <ConnectionsPage />}
          {section === "skills" && <SkillsPage />}
          {section === "mcp" && <McpPage />}
          {section === "appearance" && <Appearance />}
          {section === "engine" && <Engine />}
        </div>
      </div>
    </div>
  );
}
