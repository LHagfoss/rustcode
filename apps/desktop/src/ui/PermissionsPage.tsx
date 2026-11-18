// Settings › Permissions: everything about what threads may do without asking, in one page
// (docs/desktop.md › Permissions). The default access, a project's own default, the threads that
// chose their own, the approvals the app remembers, and what still asks at Full access. Every
// control applies at once: the store changes first and the server follows (settingsScope.ts).

import { useEffect, useMemo } from "react";
import type { Json } from "../acp/jsonrpc";
import { useNow } from "../lib/useNow";
import { activeGrants, grantRemaining } from "../state/grants";
import { forgetThreadGrant } from "../state/host";
import { threadGrants } from "../state/permissionGrants";
import {
  ALWAYS_ASKS,
  modeLabel,
  NOT_ACCESS,
  PERMISSION_MODES,
  type PermissionMode,
  parsePermissionMode,
} from "../state/permissions";
import { projectKey, projectLabel, registeredProjects } from "../state/projectBook";
import { projectsOn } from "../state/projects";
import { useLoadScopeSettings, useScopedSetting } from "../state/settingsScope";
import {
  clearPermissionModes,
  inheritedModeOf,
  openSettings,
  refreshGrants,
  revokeGrant,
  SETTINGS_DEFAULTS,
  selectThread,
  setProjectPermissionMode,
  useApp,
} from "../state/store";
import { Check, Shield, ShieldCheck, ShieldOff } from "./icons";
import { cx } from "./primitives";
import { ScopedValue, SettingsScopeBar } from "./SettingsScope";
import { Action, Group, Row, Select } from "./settingsKit";

const ICON: Record<PermissionMode, typeof Shield> = { ask: Shield, edits: ShieldCheck, full: ShieldOff };

const permissionLabel = (value: Json | undefined) =>
  PERMISSION_MODES.find((mode) => mode.id === value)?.label ?? String(value ?? "Ask");

/** The value a project's picker shows when the project follows the default. */
const FOLLOW = "default";

export function PermissionsPage() {
  useLoadScopeSettings();
  return (
    <>
      <SettingsScopeBar />
      <DefaultAccessGroup />
      <ProjectAccessGroup />
      <ThreadAccessGroup />
      <RememberedGroup />
      <Group
        title="Still asks at Full access"
        hint="Nothing else asks a Full-access thread. The card of each of these says why it appears."
      >
        {ALWAYS_ASKS.map((item) => (
          <Row key={item.title} title={item.title} subtitle={item.why} />
        ))}
      </Group>
      <Group
        title="Not controlled by access"
        hint="These can refuse at any access level, Full included. No card asks; the refusal names the place to change it."
      >
        {NOT_ACCESS.map((item) => (
          <Row key={item.title} title={item.title} subtitle={item.why} />
        ))}
      </Group>
    </>
  );
}

/** The three modes as a list, each with what it means in practice. Picking one applies at once. */
function DefaultAccessGroup() {
  const setting = useScopedSetting("default-permission-mode", permissionLabel);
  const value = parsePermissionMode(setting.value ?? SETTINGS_DEFAULTS.defaultPermissionMode);
  return (
    <Group
      title="Default access"
      hint="What a thread may do without asking, unless its project or the thread itself says otherwise. Threads that follow the default change with it."
      action={
        setting.mixed || setting.project ? (
          <ScopedValue mixed={setting.mixed} breakdown={setting.breakdown} project={setting.project}>
            {null}
          </ScopedValue>
        ) : undefined
      }
    >
      <div role="radiogroup" aria-label="Default access" className="flex flex-col">
        {PERMISSION_MODES.map((mode) => {
          const Icon = ICON[mode.id];
          const checked = !setting.mixed && mode.id === value;
          return (
            <button
              key={mode.id}
              type="button"
              role="radio"
              aria-checked={checked}
              onClick={() => void setting.set(mode.id)}
              className="flex w-full items-start gap-3 px-4 py-3 text-left hover:bg-fill-subtle"
            >
              <Icon
                size={16}
                className={cx("mt-0.5 shrink-0", mode.id === "full" ? "text-warning" : "text-fg-muted")}
              />
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="font-medium text-body text-fg">{mode.label}</span>
                <span className="text-callout text-fg-muted">{mode.explain}</span>
              </span>
              <Check size={16} className={cx("mt-0.5 shrink-0 text-accent", !checked && "invisible")} />
            </button>
          );
        })}
      </div>
    </Group>
  );
}

/** Each project the user works in, with its own default or "Use default". */
function ProjectAccessGroup() {
  const sessions = useApp((s) => s.sessions);
  const overrides = useApp((s) => s.projectPermissionModes);
  const fallback = useApp((s) => s.defaultPermissionMode);
  const projects = useMemo(() => {
    const paths = [
      ...Object.keys(overrides),
      ...projectsOn(sessions).map((project) => projectKey(project.path)),
      ...registeredProjects().map(({ path }) => projectKey(path)),
    ];
    return [...new Set(paths)].filter(Boolean);
  }, [sessions, overrides]);
  const options = [
    { value: FOLLOW, label: `Use default (${modeLabel(fallback)})` },
    ...PERMISSION_MODES.map((mode) => ({ value: mode.id, label: mode.label })),
  ];
  return (
    <Group
      title="Projects"
      hint="A project's own default applies to its threads that did not pick a mode themselves, and to the threads it starts."
      footer={projects.length ? undefined : "Projects appear here once a thread runs in one."}
    >
      {projects.map((path) => (
        <Row key={path} title={projectLabel(path)} subtitle={path}>
          <Select
            label={`Access in ${projectLabel(path)}`}
            value={overrides[path] ?? FOLLOW}
            options={options}
            onChange={(next) =>
              setProjectPermissionMode(path, next === FOLLOW ? null : parsePermissionMode(next))
            }
          />
        </Row>
      ))}
    </Group>
  );
}

/** Threads with a mode picked under their own composer, which no default reaches. */
function ThreadAccessGroup() {
  const own = useApp((s) => s.permissionModes);
  const sessions = useApp((s) => s.sessions);
  // The "default here" line follows a project's default and the default as they change.
  useApp((s) => s.projectPermissionModes);
  useApp((s) => s.defaultPermissionMode);
  const state = useApp.getState;
  const rows = useMemo(
    () =>
      sessions
        .filter((session) => own[session.id] !== undefined)
        .map((session) => ({
          id: session.id,
          title: session.title || "Untitled thread",
          mode: own[session.id]!,
        })),
    [sessions, own],
  );
  if (!rows.length) return null;
  return (
    <Group
      title="Threads with their own access"
      hint="Picked from a thread's composer, or given to a delegate when it started. The default does not change these."
      action={
        <Action size="sm" onClick={() => clearPermissionModes(rows.map((row) => row.id))}>
          Use default for all
        </Action>
      }
    >
      {rows.map((row) => (
        <Row
          key={row.id}
          title={
            <button
              type="button"
              className="truncate text-left hover:underline"
              onClick={() => selectThread(row.id)}
            >
              {row.title}
            </button>
          }
          subtitle={`${modeLabel(row.mode)} · default here is ${modeLabel(inheritedModeOf(state(), row.id))}`}
        >
          <Action size="sm" onClick={() => clearPermissionModes([row.id])}>
            Use default
          </Action>
        </Row>
      ))}
    </Group>
  );
}

/** What the app answers on its own because the user said so once. Each can be revoked here. */
function RememberedGroup() {
  const timed = useApp((s) => s.grants);
  const book = useApp((s) => s.links);
  const sessions = useApp((s) => s.sessions);
  const status = useApp((s) => s.engine.phase);
  useEffect(() => {
    if (status === "ready") void refreshGrants();
  }, [status]);
  const now = useNow(timed.length > 0);
  const hourly = activeGrants(timed, now);
  const titles = useMemo(
    () => new Map(sessions.map((s) => [s.id, s.title || "Untitled thread"])),
    [sessions],
  );
  const threads = useMemo(() => threadGrants(book, (id) => titles.get(id)), [book, titles]);
  return (
    <Group
      title="Remembered approvals"
      hint="“Allow for 1 hour” on a command, and “Allow for This Thread” on a thread's request to start, message or change other threads."
      footer={
        <>
          Delegates using your named accounts are approved per thread above.{" "}
          <button
            type="button"
            className="text-accent hover:underline"
            onClick={() => openSettings("orchestration")}
          >
            “Let delegates use my named accounts without asking” in Orchestration
          </button>
        </>
      }
    >
      {!hourly.length && !threads.length && (
        <Row title="None" subtitle="Nothing is allowed beyond each thread's access." />
      )}
      {hourly.map((grant) => (
        <Row
          key={grant.id}
          title={<span className="font-mono">{grant.prefix}</span>}
          subtitle={`Runs without asking for ${grantRemaining(grant, now)} more`}
        >
          <Action size="sm" onClick={() => void revokeGrant(grant.id)}>
            Revoke
          </Action>
        </Row>
      ))}
      {threads.map((grant) => (
        <Row
          key={`${grant.caller}\u0000${grant.key}`}
          title={grant.label}
          subtitle={`In “${titles.get(grant.caller) ?? grant.caller}”`}
        >
          <Action size="sm" onClick={() => forgetThreadGrant(grant.caller, grant.key)}>
            Revoke
          </Action>
        </Row>
      ))}
    </Group>
  );
}
