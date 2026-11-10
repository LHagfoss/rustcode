import { fileManagerName } from "../lib/platform";
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
import { useCallback, useEffect, useMemo, useState } from "react";
import { arr, bool, type Json, type JsonObject, num, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { createSkill, openPath, revealPath } from "../lib/native";
import { hasShell } from "../lib/shell";
import { mcpCopy } from "../state/settingsRegistry";
import { machineRequest, useLoadScopeSettings, useScope } from "../state/settingsScope";
import { askConfirm, errorText, toast, useApp } from "../state/store";
import { cardInset, cx, noTextHelpers, Spinner, Switch, Tooltip } from "./primitives";
import { SettingsScopeBar } from "./SettingsScope";
import { Action, fieldClass, Group, Loading, Row, SectionTitle, Select } from "./settingsKit";

interface Skill {
  name: string;
  description: string;
  path: string;
  source: string;
  /** A harness owns the folder: listed here, never written by RustCode. */
  readOnly: boolean;
  enabled: boolean;
  /** Its SKILL.md, so Settings can copy the definition to another machine. */
  content?: string;
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
  envKeys: string[];
  headerKeys: string[];
  copyTarget: string[] | null;
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
        readOnly: bool(o.readOnly) ?? false,
        enabled: bool(o.enabled) ?? false,
        content: str(o.content),
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
        envKeys: arr(o.envKeys).flatMap((key) => (typeof key === "string" ? [key] : [])),
        headerKeys: arr(o.headerKeys).flatMap((key) => (typeof key === "string" ? [key] : [])),
        copyTarget: Array.isArray(o.copyTarget)
          ? arr(o.copyTarget).flatMap((part) => (typeof part === "string" ? [part] : []))
          : null,
      };
    }),
  };
};

/** A harness's own folder is read, never written. */
const HARNESS_SKILLS_NOTE =
  "Read from Claude Code's own folder; RustCode never writes there. Turning one off stops RustCode from offering it, and tells a new external-harness thread not to use it.";

/** Where a skill was found, in the words people use. */
const SOURCE: Record<string, { label: string; order: number; note?: string }> = {
  workspace: { label: "Project", order: 0 },
  "claude-project": { label: "Claude Code (project)", order: 1, note: HARNESS_SKILLS_NOTE },
  rustcode: { label: "User", order: 2 },
  universal: { label: "Shared (~/.agents)", order: 3 },
  "claude-user": { label: "Claude Code (~/.claude)", order: 4, note: HARNESS_SKILLS_NOTE },
  configured: { label: "Configured", order: 5 },
  builtin: { label: "Built-in", order: 6 },
};

const sourceOf = (s: string) => SOURCE[s] ?? { label: s || "Other", order: 9 };

/** One machine's skills and MCP servers, read over its own connection. */
function useCapabilities(host: string, version: number) {
  const [data, setData] = useState<Capabilities | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    try {
      setData(parseCapabilities(await machineRequest(host, Method.capabilities, {}, 30_000)));
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [host]);
  // `version` re-reads the machine after an add-to-all reached it.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `version` is the read trigger
  useEffect(() => {
    void load();
  }, [load, version]);
  useEffect(() => {
    if (!data?.mcpServers.some((server) => server.connection === "connecting")) return;
    const timer = setInterval(() => void load(), 1000);
    return () => clearInterval(timer);
  }, [data, load]);
  const run = async (method: string, params: JsonObject, timeout = 30_000) => {
    try {
      setData(parseCapabilities(await machineRequest(host, method, params, timeout)));
      return true;
    } catch (e) {
      toast(errorText(e), "error");
      return false;
    }
  };
  return { data, error, reload: load, run };
}

export function CapabilitiesPage() {
  useLoadScopeSettings();
  const { targets, writes, machines } = useScope();
  const [filter, setFilter] = useState("");
  const [version, bump] = useState(0);
  const [addingAll, setAddingAll] = useState(false);
  const q = filter.trim().toLowerCase();
  const online = targets.filter((machine) => machine.online);
  // Add to all machines sends the definition, never the values: a secret set
  // here stays here and is named for the user to set on each machine.
  const runAll = async (method: string, params: JsonObject, timeout?: number) => {
    let ok = true;
    for (const machine of writes) {
      const forMachine = method === Method.mcpAdd ? blankMcpSecrets(params) : params;
      try {
        await machineRequest(machine.host, method, forMachine, timeout);
      } catch (e) {
        ok = false;
        toast(`${machine.name}: ${errorText(e)}`, "error");
      }
    }
    if (ok) bump((n) => n + 1);
    return ok;
  };
  return (
    <>
      <SettingsScopeBar />
      <div className="mb-6 flex items-center gap-2">
        <label className={cx(fieldClass, "flex flex-1 items-center gap-2 focus-within:bg-fill-strong")}>
          <Search size={14} className="shrink-0 text-fg-faint" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter skills and servers"
            aria-label="Filter skills and servers"
            {...noTextHelpers}
            className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-fg-faint"
          />
        </label>
        <Action onClick={() => bump((n) => n + 1)} aria-label="Refresh">
          <RefreshCw size={12} /> Refresh
        </Action>
      </div>
      {online.length === 0 && (
        <p className="mb-6 rounded-lg bg-warning/10 px-3 py-2 text-callout">
          No connected machine. Connect one in Settings › Machines to read or change its skills and MCP
          servers.
        </p>
      )}
      {writes.length > 1 && (
        <Group
          title="Add to all machines"
          action={
            <Action size="sm" onClick={() => setAddingAll(!addingAll)} aria-expanded={addingAll}>
              <Plus size={12} /> Add server to all
            </Action>
          }
          footer="The definition is added to every connected machine. Environment variables and headers are left blank on each one; set their values on that machine."
        >
          {!addingAll && (
            <Row
              title="One MCP server on every connected machine"
              subtitle="Choose Add server to all and enter the definition once."
            />
          )}
          {addingAll && (
            <AddServer
              run={async (method, params, timeout) => {
                const keys = method === Method.mcpAdd ? mcpSecretKeys(params) : [];
                const ok = await runAll(method, params, timeout);
                if (ok && keys.length)
                  toast(`Added to ${writes.length} machines. Set ${keys.join(", ")} on each one.`, "success");
                return ok;
              }}
              onDone={() => setAddingAll(false)}
            />
          )}
        </Group>
      )}
      {targets.map((machine, index) => (
        <MachineCapabilities
          key={machine.host || "local"}
          machine={machine}
          others={machines.filter((other) => other.host !== machine.host && other.online)}
          filter={q}
          version={version}
          showName={targets.length > 1 || !machine.local}
          last={index === targets.length - 1}
          onChanged={() => bump((n) => n + 1)}
        />
      ))}
    </>
  );
}

/** One machine's two sections, under its name when the page shows more than one. */
function MachineCapabilities({
  machine,
  others,
  filter,
  version,
  showName,
  last,
  onChanged,
}: {
  machine: { host: string; name: string; online: boolean; local: boolean };
  others: { host: string; name: string; online: boolean; local: boolean }[];
  filter: string;
  version: number;
  showName: boolean;
  last: boolean;
  onChanged: () => void;
}) {
  const { data, error, run } = useCapabilities(machine.host, version);
  if (!machine.online)
    return (
      <Group title={showName ? machine.name : undefined}>
        <Row
          title="Offline — keeps its skills and servers"
          subtitle="Connect the machine in Settings › Machines to read or change them."
        />
      </Group>
    );
  if (!data) return <Loading error={error} />;
  return (
    <div className={cx(!last && "mb-8")}>
      {showName && <SectionTitle title={machine.local ? "This machine" : machine.name} />}
      <McpSection data={data} filter={filter} run={run} others={others} onChanged={onChanged} />
      <SkillsSection data={data} filter={filter} run={run} others={others} onChanged={onChanged} />
    </div>
  );
}

/** The env/header values of an add request as key names with blank values. */
function blankMcpSecrets(params: JsonObject): JsonObject {
  const env = Array.isArray(params.env)
    ? params.env.map((line) => `${String(line).split("=")[0]}=`)
    : undefined;
  const headers = Array.isArray(params.headers)
    ? params.headers.map((line) => `${String(line).split(":")[0]}: `)
    : undefined;
  return { ...params, ...(env ? { env } : {}), ...(headers ? { headers } : {}) };
}

function mcpSecretKeys(params: JsonObject): string[] {
  const env = Array.isArray(params.env)
    ? params.env.map((line) => String(line).split("=")[0] ?? "").filter(Boolean)
    : [];
  const headers = Array.isArray(params.headers)
    ? params.headers.map((line) => String(line).split(":")[0] ?? "").filter(Boolean)
    : [];
  return [...env, ...headers];
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
  connecting: "Connecting…",
  failed: "Failed",
  disabled: "Off",
  not_connected: "Not connected",
};

/** A "Copy to…" menu: one other machine, or every other connected one. */
function CopyMenu({
  label,
  busy,
  others,
  onCopy,
}: {
  label: string;
  busy: boolean;
  others: { host: string; name: string; online: boolean; local: boolean }[];
  onCopy: (target: string) => void;
}) {
  const [value, setValue] = useState("");
  const options = [
    { value: "", label: busy ? "Copying…" : "Copy to…" },
    ...others.map((machine) => ({
      value: `machine:${machine.host}`,
      label: `Copy to ${machine.name}`,
    })),
    ...(others.length > 1 ? [{ value: "all", label: "Copy to all machines" }] : []),
  ];
  return (
    <Select
      label={label}
      value={value}
      disabled={busy}
      options={options}
      onChange={(next) => {
        setValue("");
        if (!next) return;
        onCopy(next === "all" ? "all" : next.slice("machine:".length));
      }}
      className="max-w-[180px]"
    />
  );
}

function McpSection({
  data,
  filter,
  run,
  others,
  onChanged,
}: {
  data: Capabilities;
  filter: string;
  run: (method: string, params: JsonObject, timeout?: number) => Promise<boolean>;
  others: { host: string; name: string; online: boolean; local: boolean }[];
  onChanged: () => void;
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
          subtitle={filter ? undefined : "Add one to give agents its tools."}
        />
      )}
      {shown.map((s) => (
        <McpRow
          key={s.name}
          server={s}
          open={open === s.name}
          onToggleOpen={() => setOpen(open === s.name ? null : s.name)}
          run={run}
          others={others}
          onChanged={onChanged}
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
  others,
  onChanged,
}: {
  server: McpServer;
  open: boolean;
  onToggleOpen: () => void;
  run: (method: string, params: JsonObject, timeout?: number) => Promise<boolean>;
  others: { host: string; name: string; online: boolean; local: boolean }[];
  onChanged: () => void;
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
  // A server that failed, or dropped its connection, comes back in place:
  // no toggle off and on, and no change to the config file. A disabled or
  // unconfigured server has nothing to reconnect to.
  const canRetry = s.enabled && s.scope !== "session";
  const [retrying, setRetrying] = useState(false);
  const [copying, setCopying] = useState(false);
  const retry = async () => {
    setRetrying(true);
    try {
      await run(Method.mcpRestart, { name: s.name }, 45_000);
    } finally {
      setRetrying(false);
    }
  };
  const copyTo = async (target: string) => {
    const copy = mcpCopy({
      name: s.name,
      transport: s.transport,
      copyTarget: s.copyTarget,
      envKeys: s.envKeys,
      headerKeys: s.headerKeys,
    });
    if (!copy) {
      toast("This server's command has a masked secret; set it up on that machine itself.", "error");
      return;
    }
    const nodes = target === "all" ? others : others.filter((machine) => machine.host === target);
    if (!nodes.length) return;
    setCopying(true);
    try {
      for (const machine of nodes) {
        await machineRequest(
          machine.host,
          Method.mcpAdd,
          {
            name: copy.name,
            target: copy.target,
            transport: copy.transport,
            ...(copy.transport === "http" ? { headers: copy.headers } : { env: copy.env }),
          },
          45_000,
        );
      }
      const where = nodes.map((machine) => machine.name).join(", ");
      toast(
        copy.secretKeys.length
          ? `Copied ${s.name} to ${where}. Set ${copy.secretKeys.join(", ")} there.`
          : `Copied ${s.name} to ${where}.`,
        "success",
      );
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setCopying(false);
    }
  };
  return (
    <div>
      <div className="flex min-h-11 items-center gap-3 px-4 py-2">
        <button
          type="button"
          onClick={onToggleOpen}
          aria-expanded={open}
          className="-mx-1.5 flex min-w-0 flex-1 items-center gap-2 rounded-md px-1.5 py-1 text-left hover:bg-fill-subtle"
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
              {retrying
                ? "Reconnecting…"
                : (s.error ??
                  `${CONNECTION[s.connection] ?? s.connection} · ${s.transport}${
                    s.connection === "connected"
                      ? ` · ${s.toolCount} tool${s.toolCount === 1 ? "" : "s"}`
                      : ""
                  }`)}
            </span>
          </span>
        </button>
        <div className="flex shrink-0 items-center gap-1.5">
          <Tooltip
            label={
              !s.enabled
                ? "Turn the server on to reconnect"
                : s.scope === "session"
                  ? "This server is no longer in a config file"
                  : retrying
                    ? "Reconnecting…"
                    : "Reconnect"
            }
          >
            <button
              type="button"
              aria-label={`Reconnect ${s.name}`}
              aria-busy={retrying}
              disabled={!canRetry || retrying}
              onClick={() => void retry()}
              className="flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong disabled:opacity-40"
            >
              {retrying ? <Spinner size={13} /> : <RefreshCw size={13} />}
            </button>
          </Tooltip>
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
          {others.length > 0 && s.copyTarget?.length ? (
            <CopyMenu
              label={`Copy ${s.name}`}
              busy={copying}
              others={others}
              onCopy={(target) => void copyTo(target)}
            />
          ) : null}
        </div>
      </div>
      {open && (
        <div className="rc-expand flex flex-col gap-3 px-4 pb-3 pl-10">
          <dl className="grid grid-cols-[88px_1fr] gap-x-3 gap-y-1 text-callout">
            <dt className="text-fg-faint">Command</dt>
            <dd className="selectable m-0 truncate font-mono text-fg-muted">{s.target}</dd>
            <dt className="text-fg-faint">Scope</dt>
            <dd className="m-0 text-fg-muted">{s.scope}</dd>
            {(s.envKeys.length > 0 || s.headerKeys.length > 0) && (
              <>
                <dt className="text-fg-faint">Set here</dt>
                <dd className="m-0 font-mono text-caption text-fg-muted">
                  {[...s.envKeys, ...s.headerKeys].join(", ")} — values stay on this machine
                </dd>
              </>
            )}
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
            <pre className="selectable m-0 rounded-md bg-code-bg p-2 font-mono text-caption whitespace-pre-wrap text-danger">
              {s.error}
            </pre>
          )}
          <div>
            <div className="mb-1 text-caption text-fg-faint">Log</div>
            <pre className="selectable m-0 max-h-[160px] overflow-auto rounded-md bg-code-bg p-2 font-mono text-caption leading-[15px] whitespace-pre-wrap text-fg-muted">
              {log?.length ? log.join("\n") : "No lines from this MCP server in the RustCode server's log."}
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
    <div className="rc-expand flex flex-col gap-2 px-4 py-3">
      <input
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Name"
        aria-label="Server name"
        {...noTextHelpers}
        className={fieldClass}
      />
      <input
        value={target}
        onChange={(e) => setTarget(e.target.value)}
        placeholder="Command (npx some-server) or https:// URL"
        aria-label="Server command or URL"
        {...noTextHelpers}
        className={cx(fieldClass, "font-mono")}
      />
      <textarea
        value={env}
        onChange={(e) => setEnv(e.target.value)}
        rows={2}
        placeholder={isHttp ? "Headers, one per line (Name: value)" : "Environment, one per line (KEY=value)"}
        aria-label={isHttp ? "Headers" : "Environment"}
        {...noTextHelpers}
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
  others,
  onChanged,
}: {
  data: Capabilities;
  filter: string;
  run: (method: string, params: JsonObject, timeout?: number) => Promise<boolean>;
  others: { host: string; name: string; online: boolean; local: boolean }[];
  onChanged: () => void;
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
          footer={sourceOf(source).note}
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
                  {!s.readOnly && (
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
                  )}
                  <Tooltip label={`Show in ${fileManagerName()}`}>
                    <button
                      type="button"
                      aria-label={`Show ${s.name} in ${fileManagerName()}`}
                      onClick={() => revealPath(s.path)}
                      className="flex size-7 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
                    >
                      <FolderOpen size={13} />
                    </button>
                  </Tooltip>
                </div>
              )}
              {s.content && others.length > 0 && (
                <SkillCopy skill={s} others={others} onChanged={onChanged} />
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

/** Copy one skill's definition (its SKILL.md) to another connected machine. */
function SkillCopy({
  skill,
  others,
  onChanged,
}: {
  skill: Skill;
  others: { host: string; name: string; online: boolean; local: boolean }[];
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const copyTo = async (target: string) => {
    if (!skill.content) return;
    const nodes = target === "all" ? others : others.filter((machine) => machine.host === target);
    if (!nodes.length) return;
    setBusy(true);
    try {
      for (const machine of nodes) {
        await machineRequest(
          machine.host,
          "_rustcode/skills/add",
          { name: skill.name, content: skill.content },
          30_000,
        );
      }
      const where = nodes.map((machine) => machine.name).join(", ");
      toast(
        `Copied ${skill.name}'s SKILL.md to ${where}. Other files in the skill stay on the source machine.`,
        "success",
      );
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <CopyMenu
      label={`Copy ${skill.name}`}
      busy={busy}
      others={others}
      onCopy={(target) => void copyTo(target)}
    />
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
      toast("Skill created. The agent finds it on the next turn.", "success");
      onDone();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className={cx("rc-expand mb-6 flex flex-col gap-2 rounded-xl bg-surface", cardInset)}>
      <input
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value.toLowerCase())}
        onKeyDown={(e) => e.key === "Enter" && valid && void create()}
        placeholder="skill-name"
        aria-label="Skill name"
        {...noTextHelpers}
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
