import { basename, join } from "../lib/paths";
import { appPlatform, fileManagerName } from "../lib/platform";

// Settings › Capabilities: skills and MCP servers in one place, through the
// engine process that runs them (`_rustcode/capabilities*`), so a status
// is never a guess. Skills by where they come from, each with its file;
// servers with their state, tools, errors and log lines.

import { DropdownMenu } from "radix-ui";
import { useCallback, useEffect, useMemo, useState } from "react";
import { arr, type JsonObject, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { createSkill, openPath, revealPath } from "../lib/native";
import { opError, opRunning, useOp } from "../lib/op";
import { hasShell } from "../lib/shell";
import { failureSummary, mcpCopy, runOnMachines, secretTargetReason } from "../state/settingsRegistry";
import { machineRequest, useLoadScopeSettings, useScope } from "../state/settingsScope";
import { askConfirm, errorText, toast, useApp } from "../state/store";
import { createSkillWithAgent } from "../state/threadOpen";
import {
  type Capabilities,
  type McpServer,
  parseCapabilities,
  SIGN_IN_TIMEOUT,
  type Skill,
} from "./capabilitiesData";
import { Fold } from "./Fold";
import { ChevronRight, Copy, Ellipsis, FolderOpen, Pencil, RefreshCw, Search } from "./icons";
import { cardInset, cx, MenuContent, menuItem, noTextHelpers, Spinner, Switch, Tooltip } from "./primitives";
import { SettingsScopeBar } from "./SettingsScope";
import {
  Action,
  ErrorDetails,
  fieldClass,
  Group,
  Loading,
  PathText,
  Row,
  SectionTitle,
  Select,
  SettingsHeaderActions,
} from "./settingsKit";

export { type Capabilities, type McpServer, parseCapabilities, SIGN_IN_TIMEOUT, type Skill };

/** A harness's own folder is read, never written. */
const HARNESS_SKILLS_NOTE = "Read from Claude Code's own folder; RustCode never writes there.";

const HARNESS_SKILLS_HINT =
  "Turning one off stops RustCode from offering it, and tells a new external-harness thread not to use it.";

/** Where a skill was found, in the words people use. */
const SOURCE: Record<string, { label: string; order: number; note?: string; hint?: string }> = {
  workspace: { label: "Project", order: 0 },
  "claude-project": {
    label: "Claude Code (project)",
    order: 1,
    note: HARNESS_SKILLS_NOTE,
    hint: HARNESS_SKILLS_HINT,
  },
  rustcode: { label: "User", order: 2 },
  universal: { label: "Shared (~/.agents)", order: 3 },
  "claude-user": {
    label: "Claude Code (~/.claude)",
    order: 4,
    note: HARNESS_SKILLS_NOTE,
    hint: HARNESS_SKILLS_HINT,
  },
  configured: { label: "Configured", order: 5 },
  builtin: { label: "Built-in", order: 6 },
};

const sourceOf = (s: string) => SOURCE[s] ?? { label: s || "Other", order: 9 };

/** One machine's skills and MCP servers, read over its own connection. */
function useCapabilities(host: string, version: number) {
  // A read that a newer read or a change overtook is dropped, not drawn over it.
  const [read, runRead] = useOp<Capabilities>();
  const data = read.data ?? null;
  const error = opError(read);
  const load = useCallback(
    () =>
      runRead(async () => parseCapabilities(await machineRequest(host, Method.capabilities, {}, 30_000)), {
        describe: errorText,
      }),
    [host, runRead],
  );
  // `version` re-reads the machine after an add-to-all reached it.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `version` is the read trigger
  useEffect(() => {
    void load();
  }, [load, version]);
  // The next poll waits for the last read to settle, so a read slower than the poll is not overtaken.
  const reading = opRunning(read);
  useEffect(() => {
    if (reading || !data?.mcpServers.some((server) => server.connection === "connecting")) return;
    const timer = setTimeout(() => void load(), 1000);
    return () => clearTimeout(timer);
  }, [data, reading, load]);
  const run = async (method: string, params: JsonObject, timeout = 30_000) => {
    try {
      const next = parseCapabilities(await machineRequest(host, method, params, timeout));
      void runRead(async () => next);
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
  // Add to all machines keeps the typed values on this machine and sends the
  // other machines the definition with blank values. A target that would carry
  // a secret goes to this machine only; the others are refused with the reason.
  const runAll = async (method: string, params: JsonObject, timeout?: number) => {
    const isAdd = method === Method.mcpAdd;
    const local = writes.find((machine) => machine.local);
    const reason = isAdd
      ? secretTargetReason(
          arr(params.target).map((part) => String(part)),
          str(params.transport) ?? "",
        )
      : null;
    if (reason && !local) {
      toast(`Not added: ${reason}, and this computer is not in the chosen scope.`, "error");
      return false;
    }
    const recipients = reason && local ? [local] : writes;
    const result = await runOnMachines(recipients, (machine) => {
      const forMachine = isAdd && !machine.local ? blankMcpSecrets(params) : params;
      return machineRequest(machine.host, method, forMachine, timeout);
    });
    for (const failure of result.failures) toast(`${failure.name}: ${failure.message}`, "error");
    if (reason && local && result.succeeded.length) {
      toast(
        `Added to ${local.name} only: ${reason}. The other machines did not get it; set it up there yourself.`,
        "info",
      );
    } else if (isAdd && result.succeeded.length) {
      const keys = mcpSecretKeys(params);
      const blanked = keys.length > 0 && result.succeeded.some((machine) => !machine.local);
      toast(
        blanked
          ? `Added to ${result.succeeded.length} machines. ${keys.join(", ")} ${keys.length === 1 ? "stays" : "stay"} on this computer; set ${keys.length === 1 ? "it" : "them"} on the others.`
          : `Added to ${result.succeeded.length} ${result.succeeded.length === 1 ? "machine" : "machines"}.`,
        "success",
      );
    }
    if (result.succeeded.length) bump((n) => n + 1);
    return result.failures.length === 0;
  };
  return (
    <>
      <SettingsHeaderActions>
        <Action onClick={() => bump((n) => n + 1)} aria-label="Refresh">
          Refresh
        </Action>
      </SettingsHeaderActions>
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
      </div>
      {online.length === 0 && (
        <p className="mb-6 rounded-lg bg-warning/10 px-3 py-2 text-callout">
          No connected machine. Add one in Settings › Machines to read or change its skills and MCP servers.
        </p>
      )}
      {writes.length > 1 && (
        <Group
          title="Add to all machines"
          action={
            <Action size="sm" onClick={() => setAddingAll(!addingAll)} aria-expanded={addingAll}>
              Add server to all
            </Action>
          }
          footer="Added to every connected machine. Environment variables and headers stay on this one."
          hint="The others get the definition with blank values, and a target that would carry a secret is added here only."
        >
          {!addingAll && (
            <Row
              title="One MCP server on every connected machine"
              subtitle="Choose Add server to all and enter the definition once."
            />
          )}
          {addingAll && <AddServer run={runAll} onDone={() => setAddingAll(false)} />}
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
      <SkillsSection
        data={data}
        filter={filter}
        run={run}
        host={machine.host}
        others={others}
        onChanged={onChanged}
      />
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
        <div className="flex items-center gap-2">
          {data.configPath && hasShell() && (
            <Tooltip label={`Edit ${data.configPath}`}>
              <Action
                quiet
                size="sm"
                aria-label="Edit the config file"
                onClick={() => openPath(data.configPath!)}
              >
                Edit config
              </Action>
            </Tooltip>
          )}
          <Action size="sm" onClick={() => setAdding(!adding)} aria-expanded={adding}>
            Add server
          </Action>
        </div>
      }
      footer={data.configPath ? `Saved in ${data.configPath}` : undefined}
    >
      <Fold open={adding}>
        <AddServer run={run} onDone={() => setAdding(false)} />
      </Fold>
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
  const [signingIn, setSigningIn] = useState(false);
  const [copying, setCopying] = useState(false);
  const retry = async () => {
    setRetrying(true);
    try {
      await run(Method.mcpRestart, { name: s.name }, 45_000);
    } finally {
      setRetrying(false);
    }
  };
  // Reconnecting a server with no token runs the engine's browser login.
  const needsSignIn = s.auth === "signed_out" && s.enabled && s.connection !== "connected";
  const signIn = async () => {
    setSigningIn(true);
    toast(`Finish signing in to ${s.name} in your browser.`, "info");
    try {
      await run(Method.mcpRestart, { name: s.name }, SIGN_IN_TIMEOUT);
    } finally {
      setSigningIn(false);
    }
  };
  const signOut = () =>
    askConfirm({
      title: `Sign out of ${s.name}?`,
      message:
        "RustCode forgets its sign-in and disconnects it. Threads lose its tools at their next message.",
      action: "Sign out",
      destructive: true,
      run: () => void run(Method.mcpLogout, { name: s.name }, 15_000),
    });
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
    const result = await runOnMachines(nodes, (machine) =>
      machineRequest(
        machine.host,
        Method.mcpAdd,
        {
          name: copy.name,
          target: copy.target,
          transport: copy.transport,
          ...(copy.transport === "http" ? { headers: copy.headers } : { env: copy.env }),
        },
        45_000,
      ),
    );
    if (result.succeeded.length) {
      const where = result.succeeded.map((machine) => machine.name).join(", ");
      toast(
        copy.secretKeys.length
          ? `Copied ${s.name} to ${where}. Set ${copy.secretKeys.join(", ")} there.`
          : `Copied ${s.name} to ${where}.`,
        "success",
      );
      onChanged();
    }
    if (result.failures.length) toast(failureSummary(result), "error");
    setCopying(false);
  };
  return (
    <div>
      <div className="group/row relative flex min-h-11 items-center gap-3 px-4 py-2">
        <button
          type="button"
          onClick={onToggleOpen}
          aria-expanded={open}
          className="flex min-w-0 flex-1 items-center gap-2 py-1 text-left outline-none before:absolute before:inset-x-1 before:inset-y-0.5 before:rounded-row before:transition-[background-color] before:duration-(--rc-hover-out) before:ease-out group-hover/row:before:bg-fill-subtle group-hover/row:before:duration-(--rc-hover-in) focus-visible:before:bg-fill active:before:bg-fill-strong"
        >
          <ChevronRight
            size={12}
            strokeWidth={2.5}
            className={cx(
              "relative shrink-0 text-fg-faint transition-transform duration-150",
              open && "rotate-90",
            )}
          />
          <StatusDot connection={s.connection} />
          <span className="relative min-w-0 flex-1">
            <span className="block truncate font-medium text-body">{s.name}</span>
            <span className={cx("block truncate text-caption", s.error ? "text-danger" : "text-fg-muted")}>
              {signingIn
                ? "Waiting for the browser sign-in…"
                : retrying
                  ? "Reconnecting…"
                  : needsSignIn
                    ? `Sign in required · ${s.transport}`
                    : (s.error ??
                      `${CONNECTION[s.connection] ?? s.connection} · ${s.transport}${
                        s.connection === "connected"
                          ? ` · ${s.toolCount} tool${s.toolCount === 1 ? "" : "s"}`
                          : ""
                      }${s.auth === "signed_in" ? " · Signed in" : ""}`)}
            </span>
          </span>
        </button>
        <div className="relative flex shrink-0 items-center gap-1.5">
          {needsSignIn && (
            <Action size="sm" disabled={signingIn} aria-busy={signingIn} onClick={() => void signIn()}>
              {signingIn ? <Spinner size={12} /> : null} Sign in
            </Action>
          )}
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
      <Fold open={open}>
        <div className="flex flex-col gap-3 px-4 pb-3 pl-10">
          <dl className="grid grid-cols-[88px_1fr] gap-x-3 gap-y-1 text-callout">
            <dt className="text-fg-faint">Command</dt>
            <dd className="selectable m-0 truncate font-mono text-fg-muted" title={s.target}>
              {s.target}
            </dd>
            <dt className="text-fg-faint">Scope</dt>
            <dd className="m-0 text-fg-muted">{s.scope}</dd>
            {s.auth && (
              <>
                <dt className="text-fg-faint">Sign-in</dt>
                <dd className="m-0 flex items-center gap-2 text-fg-muted">
                  {s.auth === "signed_in" ? "Signed in with OAuth" : "Not signed in"}
                  {s.auth === "signed_in" && s.scope !== "session" && (
                    <Action size="sm" quiet onClick={signOut}>
                      Sign out
                    </Action>
                  )}
                </dd>
              </>
            )}
            {(s.envKeys.length > 0 || s.headerKeys.length > 0) && (
              <>
                <dt className="text-fg-faint">Set here</dt>
                <dd className="m-0 text-fg-muted">
                  {[...s.envKeys, ...s.headerKeys].join(", ")}. Values stay on this computer.
                </dd>
              </>
            )}
            {s.tools.length > 0 && (
              <>
                <dt className="text-fg-faint">Tools</dt>
                <dd className="selectable m-0 text-fg-muted">{s.tools.join(", ")}</dd>
              </>
            )}
          </dl>
          {s.error && (
            <ErrorDetails className="text-danger" summary="This server could not start." detail={s.error} />
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
              Remove
            </Action>
          )}
        </div>
      </Fold>
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
  const [adding, runAdd] = useOp<boolean>();
  const busy = opRunning(adding);
  const isHttp = /^https?:\/\//.test(target.trim());
  const add = async () => {
    const lines = env
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    const added = await runAdd(() =>
      run(Method.mcpAdd, {
        name: name.trim(),
        target: isHttp ? [target.trim()] : target.trim().split(/\s+/),
        transport: isHttp ? "http" : "stdio",
        ...(isHttp ? { headers: lines } : { env: lines }),
      }),
    );
    if (added.status === "done" && added.value) onDone();
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
  host,
  others,
  onChanged,
}: {
  data: Capabilities;
  filter: string;
  run: (method: string, params: JsonObject, timeout?: number) => Promise<boolean>;
  /** The machine that owns these skills, and reads their SKILL.md on a copy. */
  host: string;
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
          <div className="flex items-center gap-2">
            <Tooltip label="Start a thread that asks an agent to write the skill with you">
              <Action quiet size="sm" onClick={() => createSkillWithAgent(host)}>
                Create with agent
              </Action>
            </Tooltip>
            {hasShell() && (
              <Action size="sm" onClick={() => setCreating(!creating)} aria-expanded={creating}>
                New skill
              </Action>
            )}
          </div>
        }
      />
      <Fold open={creating}>
        <NewSkill onDone={() => setCreating(false)} />
      </Fold>
      {groups.length === 0 && (
        <Group>
          <Row title={filter ? "No matching skills" : "No skills yet"} />
        </Group>
      )}
      {groups.map(([source, skills]) => (
        <Group
          key={source}
          minor
          title={sourceOf(source).label}
          footer={sourceOf(source).note}
          hint={sourceOf(source).hint}
        >
          {skills.map((s) => (
            <SkillRow
              key={`${s.source}/${s.name}`}
              skill={s}
              host={host}
              others={others}
              run={run}
              onChanged={onChanged}
            />
          ))}
        </Group>
      ))}
    </>
  );
}

/**
 * One skill on one line: its name, the start of its description, a menu for its file and copies,
 * and its switch. The full description and the path are in the tip on the text, so a page of
 * dozens of skills stays scannable.
 */
function SkillRow({
  skill: s,
  host,
  others,
  run,
  onChanged,
}: {
  skill: Skill;
  host: string;
  others: { host: string; name: string; online: boolean; local: boolean }[];
  run: (method: string, params: JsonObject, timeout?: number) => Promise<boolean>;
  onChanged: () => void;
}) {
  const [copying, runCopy] = useOp();
  const busy = opRunning(copying);
  const files = Boolean(s.path) && hasShell();
  const copyTo = async (target: string) => {
    const nodes = target === "all" ? others : others.filter((machine) => machine.host === target);
    if (!nodes.length) return;
    await runCopy(async () => {
      try {
        // The page lists skills without their text; read it only for a copy.
        const answer = await machineRequest(
          host,
          "_rustcode/skills/get",
          { name: s.name, path: s.path },
          15_000,
        );
        const content = str(obj(answer)?.content);
        if (!content) {
          toast(`Could not read ${s.name}'s SKILL.md.`, "error");
          return;
        }
        const result = await runOnMachines(nodes, (machine) =>
          machineRequest(machine.host, "_rustcode/skills/add", { name: s.name, content }, 30_000),
        );
        if (result.succeeded.length) {
          const where = result.succeeded.map((machine) => machine.name).join(", ");
          toast(
            `Copied ${s.name}'s SKILL.md to ${where}. Other files in the skill stay on the source machine.`,
            "success",
          );
          onChanged();
        }
        if (result.failures.length) toast(failureSummary(result), "error");
      } catch (e) {
        toast(errorText(e), "error");
      }
    });
  };
  const tip =
    s.description || s.path ? (
      <span className="block max-w-[420px]">
        {s.description}
        {s.path && <PathText path={s.path} className="mt-1 block text-fg-faint" />}
      </span>
    ) : undefined;
  return (
    <div className="flex min-h-10 items-center gap-3 px-4 py-1.5" data-setting={s.name}>
      <Tooltip label={tip}>
        <div className="flex min-w-0 flex-1 items-baseline gap-2">
          <span className="max-w-[45%] shrink-0 truncate font-medium text-body text-fg">{s.name}</span>
          <span className="min-w-0 flex-1 truncate text-callout text-fg-muted">{s.description}</span>
        </div>
      </Tooltip>
      {(files || others.length > 0) && (
        <DropdownMenu.Root modal={false}>
          <DropdownMenu.Trigger asChild>
            <button
              type="button"
              aria-label={`More for ${s.name}`}
              disabled={busy}
              className="flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong disabled:opacity-50"
            >
              {busy ? <Spinner size={12} /> : <Ellipsis size={14} />}
            </button>
          </DropdownMenu.Trigger>
          <MenuContent align="end" className="min-w-[200px]">
            {files && !s.readOnly && (
              <DropdownMenu.Item className={menuItem} onSelect={() => openPath(skillFile(s.path))}>
                <Pencil size={13} /> Edit SKILL.md
              </DropdownMenu.Item>
            )}
            {files && (
              <DropdownMenu.Item className={menuItem} onSelect={() => revealPath(s.path)}>
                <FolderOpen size={13} /> Show in {fileManagerName()}
              </DropdownMenu.Item>
            )}
            {others.map((machine) => (
              <DropdownMenu.Item
                key={machine.host}
                className={menuItem}
                onSelect={() => void copyTo(machine.host)}
              >
                <Copy size={13} /> Copy to {machine.name}
              </DropdownMenu.Item>
            ))}
            {others.length > 1 && (
              <DropdownMenu.Item className={menuItem} onSelect={() => void copyTo("all")}>
                <Copy size={13} /> Copy to all machines
              </DropdownMenu.Item>
            )}
          </MenuContent>
        </DropdownMenu.Root>
      )}
      <Switch
        label={`Use ${s.name}`}
        checked={s.enabled}
        onChange={(v) => void run(Method.capabilitiesSet, { kind: "skill", name: s.name, enabled: v })}
      />
    </div>
  );
}

/** A skill's path is its folder or its SKILL.md. */
function skillFile(path: string): string {
  return /\.md$/i.test(path) ? path : join(path, "SKILL.md");
}

function NewSkill({ onDone }: { onDone: () => void }) {
  const cwd = useApp((s) => (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd);
  const [name, setName] = useState("");
  const [scope, setScope] = useState<"user" | "project">("user");
  const [creating, runCreate] = useOp();
  const busy = opRunning(creating);
  const valid = /^[a-z0-9][a-z0-9-]{0,63}$/.test(name);
  const create = async () => {
    await runCreate(async () => {
      try {
        const path = await createSkill(name, scope, cwd);
        openPath(path);
        toast("Skill created. The agent finds it on the next turn.", "success");
        onDone();
      } catch (e) {
        toast(e instanceof Error ? e.message : String(e), "error");
      }
    });
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
              ? appPlatform() === "windows"
                ? "%APPDATA%\\rustcode\\skills"
                : "~/.config/rustcode/skills"
              : `${cwd ? basename(cwd) : "project"}/.rustcode/skills`}
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
