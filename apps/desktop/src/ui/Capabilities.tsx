// Settings › Capabilities: skills and MCP servers in one place, through the
// engine process that runs them (`_rustcode/capabilities*`), so a status
// is never a guess. Skills by where they come from, each with its file;
// servers with their state, tools, errors and log lines.

import {
  ChevronRight,
  FilePlus2,
  FolderOpen,
  Plus,
  RefreshCw,
  Search,
  SquarePen,
  Trash2,
} from "lucide-react";
import { useMemo, useState } from "react";
import { arr, bool, type Json, type JsonObject, num, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { createSkill, openPath, revealPath } from "../lib/native";
import { hasShell } from "../lib/shell";
import { askConfirm, request, toast, useApp } from "../state/store";
import { cx, Switch, Tooltip } from "./primitives";
import { Action, fieldClass, Group, Loading, Row, SectionTitle, useEngineData } from "./settingsKit";

interface Skill {
  name: string;
  description: string;
  path: string;
  source: string;
  enabled: boolean;
}

interface McpServer {
  name: string;
  transport: string;
  target: string;
  enabled: boolean;
  scope: string;
  connection: string;
  tools: string[];
  toolCount: number;
  error?: string;
}

interface Capabilities {
  skills: Skill[];
  mcpServers: McpServer[];
  configPath?: string;
}

/** The engine sends `tools` as the tool names; an older mock sent a count. */
const parseCapabilities = (v: Json): Capabilities => {
  const root = obj(v) ?? {};
  return {
    configPath: str(root.configPath),
    skills: arr(root.skills).map((s) => {
      const o = obj(s) ?? {};
      return {
        name: str(o.name) ?? "",
        description: str(o.description) ?? "",
        path: str(o.path) ?? "",
        source: str(o.source) ?? "",
        enabled: bool(o.enabled) ?? false,
      };
    }),
    mcpServers: arr(root.mcpServers).map((s) => {
      const o = obj(s) ?? {};
      const tools = Array.isArray(o.tools)
        ? arr(o.tools)
            .map((t) => str(t) ?? "")
            .filter(Boolean)
        : [];
      return {
        name: str(o.name) ?? "",
        transport: str(o.transport) ?? "",
        target: str(o.target) ?? "",
        enabled: bool(o.enabled) ?? false,
        scope: str(o.scope) ?? "user",
        connection: str(o.connection) ?? "",
        tools,
        toolCount: Array.isArray(o.tools) ? tools.length : (num(o.tools) ?? 0),
        error: str(o.error),
      };
    }),
  };
};

/** Where a skill was found, in the words people use. */
const SOURCE: Record<string, { label: string; order: number }> = {
  workspace: { label: "Project", order: 0 },
  rustcode: { label: "User", order: 1 },
  universal: { label: "Shared (~/.agents)", order: 2 },
  configured: { label: "Configured", order: 3 },
  builtin: { label: "Built-in", order: 4 },
};

const sourceOf = (s: string) => SOURCE[s] ?? { label: s || "Other", order: 9 };

function useCapabilities() {
  const state = useEngineData(Method.capabilities, parseCapabilities);
  const run = async (method: string, params: JsonObject, timeout = 30_000) => {
    try {
      state.setData(parseCapabilities(await request(method, params, timeout)));
      return true;
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
      return false;
    }
  };
  return { ...state, run };
}

export function CapabilitiesPage() {
  const { data, error, run, reload } = useCapabilities();
  const [filter, setFilter] = useState("");
  if (!data) return <Loading error={error} />;
  const q = filter.trim().toLowerCase();
  return (
    <>
      <div className="mb-6 flex items-center gap-2">
        <label className={cx(fieldClass, "flex flex-1 items-center gap-2")}>
          <Search size={14} className="shrink-0 text-fg-faint" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter skills and servers"
            aria-label="Filter skills and servers"
            className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-fg-faint"
          />
        </label>
        <Action onClick={() => void reload()} aria-label="Refresh">
          <RefreshCw size={12} /> Refresh
        </Action>
      </div>
      <McpSection data={data} filter={q} run={run} />
      <SkillsSection data={data} filter={q} run={run} />
    </>
  );
}

// ---------------------------------------------------------------- MCP

function StatusDot({ connection }: { connection: string }) {
  return (
    <span
      className={cx(
        "size-[7px] shrink-0 rounded-full",
        connection === "connected"
          ? "bg-success"
          : connection === "failed"
            ? "bg-danger"
            : connection === "disabled"
              ? "bg-fill-strong"
              : "bg-fg-faint",
      )}
    />
  );
}

const CONNECTION: Record<string, string> = {
  connected: "Connected",
  failed: "Failed",
  disabled: "Off",
  not_connected: "Not connected",
};

function McpSection({
  data,
  filter,
  run,
}: {
  data: Capabilities;
  filter: string;
  run: (method: string, params: JsonObject, timeout?: number) => Promise<boolean>;
}) {
  const [open, setOpen] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const connected = data.mcpServers.filter((s) => s.connection === "connected").length;
  const shown = data.mcpServers.filter((s) => `${s.name} ${s.target}`.toLowerCase().includes(filter));
  return (
    <Group
      title={`MCP servers · ${connected} of ${data.mcpServers.length} connected`}
      action={
        <div className="flex items-center gap-1">
          {data.configPath && hasShell() && (
            <Tooltip label={`Edit ${data.configPath}`}>
              <Action
                quiet
                size="sm"
                aria-label="Edit the config file"
                onClick={() => openPath(data.configPath!)}
              >
                <SquarePen size={12} /> Edit config
              </Action>
            </Tooltip>
          )}
          <Action quiet size="sm" onClick={() => setAdding(!adding)} aria-expanded={adding}>
            <Plus size={12} /> Add server
          </Action>
        </div>
      }
      footer={data.configPath ? `Saved in ${data.configPath}` : undefined}
    >
      {adding && <AddServer run={run} onDone={() => setAdding(false)} />}
      {shown.length === 0 && !adding && (
        <Row
          title={filter ? "No matching servers" : "No MCP servers"}
          subtitle="Add one to give agents its tools."
        />
      )}
      {shown.map((s) => (
        <McpRow
          key={s.name}
          server={s}
          open={open === s.name}
          onToggleOpen={() => setOpen(open === s.name ? null : s.name)}
          run={run}
        />
      ))}
    </Group>
  );
}

function McpRow({
  server: s,
  open,
  onToggleOpen,
  run,
}: {
  server: McpServer;
  open: boolean;
  onToggleOpen: () => void;
  run: (method: string, params: JsonObject, timeout?: number) => Promise<boolean>;
}) {
  // The engine's stderr lines that name this server.
  // Select the log itself and filter here: a selector that builds a new array on every read made
  // React re-render without end (opening a server blanked the app).
  const engineLog = useApp((st) => (open ? st.engineLog : null));
  const log = useMemo(
    () => engineLog?.filter((l) => l.includes(s.name)).slice(-40) ?? null,
    [engineLog, s.name],
  );
  const toggleable = s.scope === "user";
  return (
    <div>
      <div className="flex min-h-11 items-center gap-3 px-3.5 py-2">
        <button
          type="button"
          onClick={onToggleOpen}
          aria-expanded={open}
          className="-mx-1.5 flex min-w-0 flex-1 items-center gap-2.5 rounded-md px-1.5 py-1 text-left hover:bg-fill-subtle"
        >
          <ChevronRight
            size={11}
            strokeWidth={2.5}
            className={cx("shrink-0 text-fg-faint transition-transform duration-150", open && "rotate-90")}
          />
          <StatusDot connection={s.connection} />
          <span className="min-w-0 flex-1">
            <span className="block truncate font-medium text-body">{s.name}</span>
            <span className={cx("block truncate text-caption", s.error ? "text-danger" : "text-fg-muted")}>
              {s.error ??
                `${CONNECTION[s.connection] ?? s.connection} · ${s.transport}${
                  s.connection === "connected" ? ` · ${s.toolCount} tool${s.toolCount === 1 ? "" : "s"}` : ""
                }`}
            </span>
          </span>
        </button>
        <Tooltip label={toggleable ? undefined : `Set in a ${s.scope} config; edit it there`}>
          <span>
            <Switch
              label={`Use ${s.name}`}
              checked={s.enabled}
              disabled={!toggleable}
              onChange={(v) => void run(Method.capabilitiesSet, { kind: "mcp", name: s.name, enabled: v })}
            />
          </span>
        </Tooltip>
      </div>
      {open && (
        <div className="rc-expand flex flex-col gap-3 px-3.5 pb-3.5 pl-10">
          <dl className="grid grid-cols-[88px_1fr] gap-x-3 gap-y-1 text-callout">
            <dt className="text-fg-faint">Command</dt>
            <dd className="selectable m-0 truncate font-mono text-fg-muted">{s.target}</dd>
            <dt className="text-fg-faint">Scope</dt>
            <dd className="m-0 text-fg-muted">{s.scope}</dd>
            {s.tools.length > 0 && (
              <>
                <dt className="text-fg-faint">Tools</dt>
                <dd className="selectable m-0 font-mono text-caption leading-[16px] text-fg-muted">
                  {s.tools.join(", ")}
                </dd>
              </>
            )}
          </dl>
          {s.error && (
            <pre className="selectable m-0 rounded-md bg-code-bg p-2.5 font-mono text-caption whitespace-pre-wrap text-danger">
              {s.error}
            </pre>
          )}
          <div>
            <div className="mb-1 text-caption text-fg-faint">Log</div>
            <pre className="selectable m-0 max-h-[160px] overflow-auto rounded-md bg-code-bg p-2.5 font-mono text-caption leading-[15px] whitespace-pre-wrap text-fg-muted">
              {log?.length ? log.join("\n") : "No lines from this server in the engine's log."}
            </pre>
          </div>
          {s.scope === "user" && (
            <Action
              danger
              className="self-start"
              onClick={() =>
                askConfirm({
                  title: `Remove ${s.name}?`,
                  message: "It stops now and is taken out of your config.",
                  action: "Remove",
                  destructive: true,
                  run: () => void run(Method.mcpRemove, { name: s.name }, 15_000),
                })
              }
            >
              <Trash2 size={12} /> Remove
            </Action>
          )}
        </div>
      )}
    </div>
  );
}

function AddServer({
  run,
  onDone,
}: {
  run: (method: string, params: JsonObject, timeout?: number) => Promise<boolean>;
  onDone: () => void;
}) {
  const [name, setName] = useState("");
  const [target, setTarget] = useState("");
  const [env, setEnv] = useState("");
  const [busy, setBusy] = useState(false);
  const isHttp = /^https?:\/\//.test(target.trim());
  const add = async () => {
    setBusy(true);
    const lines = env
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    const ok = await run(Method.mcpAdd, {
      name: name.trim(),
      target: isHttp ? [target.trim()] : target.trim().split(/\s+/),
      transport: isHttp ? "http" : "stdio",
      ...(isHttp ? { headers: lines } : { env: lines }),
    });
    setBusy(false);
    if (ok) onDone();
  };
  return (
    <div className="rc-expand flex flex-col gap-2 px-4 py-3.5">
      <input
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Name"
        aria-label="Server name"
        className={fieldClass}
      />
      <input
        value={target}
        onChange={(e) => setTarget(e.target.value)}
        placeholder="Command (npx some-server) or https:// URL"
        aria-label="Server command or URL"
        className={cx(fieldClass, "font-mono")}
      />
      <textarea
        value={env}
        onChange={(e) => setEnv(e.target.value)}
        rows={2}
        placeholder={isHttp ? "Headers, one per line (Name: value)" : "Environment, one per line (KEY=value)"}
        aria-label={isHttp ? "Headers" : "Environment"}
        className={cx(fieldClass, "h-auto resize-none py-1.5 font-mono")}
      />
      <div className="flex justify-end gap-2">
        <Action onClick={onDone}>Cancel</Action>
        <Action prominent disabled={!name.trim() || !target.trim() || busy} onClick={() => void add()}>
          Add Server
        </Action>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- skills

function SkillsSection({
  data,
  filter,
  run,
}: {
  data: Capabilities;
  filter: string;
  run: (method: string, params: JsonObject, timeout?: number) => Promise<boolean>;
}) {
  const [creating, setCreating] = useState(false);
  const on = data.skills.filter((s) => s.enabled).length;
  const groups = useMemo(() => {
    const shown = data.skills.filter((s) => `${s.name} ${s.description}`.toLowerCase().includes(filter));
    const by = new Map<string, Skill[]>();
    for (const s of shown) by.set(s.source, [...(by.get(s.source) ?? []), s]);
    return [...by.entries()].sort((a, b) => sourceOf(a[0]).order - sourceOf(b[0]).order);
  }, [data.skills, filter]);
  return (
    <>
      <SectionTitle
        title={`Skills · ${on} of ${data.skills.length} on`}
        action={
          hasShell() && (
            <Action quiet size="sm" onClick={() => setCreating(!creating)} aria-expanded={creating}>
              <FilePlus2 size={12} /> New skill
            </Action>
          )
        }
      />
      {creating && <NewSkill onDone={() => setCreating(false)} />}
      {groups.length === 0 && (
        <Group>
          <Row title={filter ? "No matching skills" : "No skills yet"} />
        </Group>
      )}
      {groups.map(([source, skills]) => (
        <Group
          key={source}
          title={<span className="font-medium text-body text-fg-muted">{sourceOf(source).label}</span>}
        >
          {skills.map((s) => (
            <Row
              key={`${s.source}/${s.name}`}
              title={s.name}
              subtitle={
                <>
                  {s.description}
                  {s.path && <span className="block truncate font-mono text-fg-faint">{s.path}</span>}
                </>
              }
            >
              {s.path && hasShell() && (
                <div className="flex shrink-0 items-center">
                  <Tooltip label="Edit SKILL.md">
                    <button
                      type="button"
                      aria-label={`Edit ${s.name}`}
                      onClick={() => openPath(skillFile(s.path))}
                      className="flex size-7 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
                    >
                      <SquarePen size={13} />
                    </button>
                  </Tooltip>
                  <Tooltip label="Show in Finder">
                    <button
                      type="button"
                      aria-label={`Show ${s.name} in Finder`}
                      onClick={() => revealPath(s.path)}
                      className="flex size-7 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
                    >
                      <FolderOpen size={13} />
                    </button>
                  </Tooltip>
                </div>
              )}
              <Switch
                label={`Use ${s.name}`}
                checked={s.enabled}
                onChange={(v) =>
                  void run(Method.capabilitiesSet, { kind: "skill", name: s.name, enabled: v })
                }
              />
            </Row>
          ))}
        </Group>
      ))}
    </>
  );
}

/** A skill's path is its folder or its SKILL.md. */
function skillFile(path: string): string {
  return /\.md$/i.test(path) ? path : `${path.replace(/\/$/, "")}/SKILL.md`;
}

function NewSkill({ onDone }: { onDone: () => void }) {
  const cwd = useApp((s) => (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd);
  const [name, setName] = useState("");
  const [scope, setScope] = useState<"user" | "project">("user");
  const [busy, setBusy] = useState(false);
  const valid = /^[a-z0-9][a-z0-9-]{0,63}$/.test(name);
  const create = async () => {
    setBusy(true);
    try {
      const path = await createSkill(name, scope, cwd);
      openPath(path);
      toast("Skill created. The engine finds it on the next turn.", "success");
      onDone();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="rc-expand mb-8 flex flex-col gap-2.5 rounded-xl border border-border-strong bg-surface-2/40 p-4">
      <input
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value.toLowerCase())}
        onKeyDown={(e) => e.key === "Enter" && valid && void create()}
        placeholder="skill-name"
        aria-label="Skill name"
        className={cx(fieldClass, "font-mono")}
      />
      <div className="flex items-center gap-2 text-callout text-fg-muted">
        <span>In</span>
        {(["user", "project"] as const).map((v) => (
          <Action
            key={v}
            quiet={scope !== v}
            size="sm"
            aria-pressed={scope === v}
            disabled={v === "project" && !cwd}
            onClick={() => setScope(v)}
            className={scope === v ? "bg-surface-3 text-fg" : undefined}
          >
            {v === "user"
              ? "~/.config/rustcode/skills"
              : `${cwd ? cwd.split("/").pop() : "project"}/.rustcode/skills`}
          </Action>
        ))}
        <span className="flex-1" />
        <Action onClick={onDone}>Cancel</Action>
        <Action prominent disabled={!valid || busy} onClick={() => void create()}>
          Create
        </Action>
      </div>
      <p className="m-0 text-caption text-fg-faint">
        Lowercase letters, digits and dashes. Creates SKILL.md from a template and opens it.
      </p>
    </div>
  );
}
