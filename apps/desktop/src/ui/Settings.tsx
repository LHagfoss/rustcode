// Settings, a full panel inside the window: the shell stays mounted behind it,
// hidden and inert, so closing Settings shows the thread exactly where it was.
// A grouped nav on the left searches every row's title and keywords
// (settingsIndex.ts) and shows one page at a time beside it; picking a result
// opens its page and lights the row. After SettingsView.swift.
// Engine-backed pages (Agents, Connections, Capabilities, Appearance's
// themes, Usage, Statistics) read and write through `_rustcode/*` requests.

import {
  Blocks,
  ChartColumn,
  Check,
  FolderOpen,
  Gauge,
  Settings as Gear,
  Info,
  Keyboard,
  KeyRound,
  Layers,
  MessageSquare,
  Palette,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  TerminalSquare,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { arr, bool, type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import {
  BANNER_NAME_DEFAULT,
  BANNER_STYLE_DEFAULT,
  chooseBanner,
  removeBanner,
  setBannerStyle,
  useBanner,
} from "../lib/banner";
import { CHAT_WIDTH_LABEL, type ChatWidth } from "../lib/chatWidth";
import { HAPTICS_DEFAULT, haptic, hapticsEnabled, setHaptics } from "../lib/haptics";
import { LINK_PREVIEWS_DEFAULT, linkPreviewsEnabled, setLinkPreviews } from "../lib/links";
import { openExternal, pickFolder, revealPath } from "../lib/native";
import { hasShell } from "../lib/shell";
import { BUILTIN_THEMES, cssName, type Theme, TOKENS } from "../lib/themes";
import { canUpdate } from "../lib/updater";
import { validateDefault } from "../state/harness";
import { MAX_ACTIVE_LIMIT, MIN_ACTIVE_LINKS } from "../state/orchestration";
import { PERMISSION_MODES, type PermissionMode } from "../state/permissions";
import {
  APP_VERSION,
  createTheme,
  errorText,
  type NewThreadChoice,
  openSettings,
  refreshHarnesses,
  request,
  restartEngine,
  SETTINGS_DEFAULTS,
  type SettingsSection,
  setActiveTheme,
  setAnimateStreaming,
  setChatWidth,
  setContinueAfterLimit,
  setDefaultCwd,
  setDefaultHarness,
  setDefaultPermissionMode,
  setNotificationsBackgroundOnly,
  setOnboarding,
  setOrchestration,
  setReadAcrossProjects,
  setResumeAfterRestart,
  setRoundedFont,
  setSmartUi,
  setSystemNotifications,
  setTheme,
  setThreadLimit,
  toast,
  useApp,
} from "../state/store";
import {
  applyUpdate,
  checkIfStale,
  checkNow,
  installProgress,
  showWhatsNew,
  useUpdates,
} from "../state/updates";
import { CapabilitiesPage } from "./Capabilities";
import { DefaultsForm, HarnessList } from "./Harnesses";
import { HeaderButtonsSetting } from "./HeaderButtonsSetting";
import { NewThreadBanner } from "./NewThreadBanner";
import { ChromeButton, cx, Kbd, Spinner, Switch } from "./primitives";
import { ShortcutsPage } from "./ShortcutSettings";
import { StatisticsPage } from "./StatisticsPage";
import { type SettingEntry, searchSettings } from "./settingsIndex";
import {
  Action,
  Choice,
  fieldClass,
  Group,
  Loading,
  Row,
  RowResets,
  SectionTitle,
  Select,
  useEngineData,
  useRowResets,
} from "./settingsKit";
import { UsagePage } from "./UsagePage";

interface Page {
  id: SettingsSection;
  label: string;
  icon: typeof Gear;
}

/**
 * The nav, top to bottom: pages, then groups of pages under a heading. One
 * page shows at a time (Settings.tsx renders it), so a long list of settings
 * never lives in one scroll.
 */
const NAV: { heading?: string; pages: Page[] }[] = [
  {
    pages: [
      { id: "general", label: "General", icon: Gear },
      { id: "chat", label: "Chat", icon: MessageSquare },
    ],
  },
  {
    heading: "Setup",
    pages: [
      { id: "agents", label: "Agents", icon: Layers },
      { id: "connections", label: "Connections", icon: KeyRound },
      { id: "capabilities", label: "Capabilities", icon: Blocks },
    ],
  },
  {
    heading: "App",
    pages: [
      { id: "appearance", label: "Appearance", icon: Palette },
      { id: "keyboard", label: "Keyboard", icon: Keyboard },
      { id: "usage", label: "Usage", icon: Gauge },
      { id: "statistics", label: "Statistics", icon: ChartColumn },
    ],
  },
  {
    heading: "System",
    pages: [
      { id: "advanced", label: "Advanced", icon: TerminalSquare },
      { id: "about", label: "About", icon: Info },
    ],
  },
];

const PAGES: Page[] = NAV.flatMap((group) => group.pages);

const pageLabel = (id: SettingsSection) => PAGES.find((p) => p.id === id)?.label ?? id;

// ---------------------------------------------------------------- pages

/**
 * Discord Rich Presence (docs/discord-rich-presence.md): the engine publishes the open project's
 * folder name and whether a turn runs. It reads the same `discord_rpc_enabled` the CLI writes, so
 * this row and `rustcode discord` are one setting.
 */
function PresenceRow() {
  const projectCwd = useApp((s) => s.projectCwd || s.defaultCwd);
  const status = useApp((s) => s.status);
  const [state, setState] = useState<{ enabled: boolean; discordDetected: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    const show = (value: Json) => {
      const root = obj(value);
      setState({
        enabled: root?.enabled === true,
        discordDetected: root?.discordDetected === true,
      });
      setError(null);
    };
    try {
      const params: JsonObject = {};
      if (projectCwd) params.cwd = projectCwd;
      show(await request("_rustcode/presence", params));
    } catch (e) {
      setError(errorText(e));
    }
  }, [projectCwd]);
  useEffect(() => {
    if (status === "ready") void load();
  }, [status, load]);
  const setEnabled = (enabled: boolean) => {
    const params: JsonObject = { enabled };
    if (projectCwd) params.cwd = projectCwd;
    void request("_rustcode/presence/set", params).then(
      (value) => {
        const root = obj(value);
        setState({ enabled: root?.enabled === true, discordDetected: root?.discordDetected === true });
        setError(null);
      },
      (e) => setError(errorText(e)),
    );
  };
  const subtitle = error
    ? error
    : !state
      ? "Checking Discord…"
      : !state.enabled
        ? "Off: nothing is shared with Discord."
        : state.discordDetected
          ? "Shares the open project's folder name and whether a turn is running. Discord is running."
          : "Shares the open project's folder name and whether a turn is running. Discord's desktop app is not running right now.";
  // Off is the default: presence shares what you work on, so turning it on is the user's call.
  const reset = state?.enabled ? () => setEnabled(false) : undefined;
  return (
    <Row title="Discord Rich Presence" subtitle={subtitle} reset={reset}>
      {state && <Switch label="Discord Rich Presence" checked={state.enabled} onChange={setEnabled} />}
    </Row>
  );
}

function GeneralPage() {
  const defaultCwd = useApp((s) => s.defaultCwd);
  const resume = useApp((s) => s.resumeAfterRestart);
  const afterLimit = useApp((s) => s.continueAfterLimit);
  const defaultAccess = useApp((s) => s.defaultPermissionMode);
  const notifications = useApp((s) => s.systemNotifications);
  const backgroundOnly = useApp((s) => s.notificationsBackgroundOnly);
  const [haptics, setHapticsState] = useState(hapticsEnabled);
  return (
    <>
      <Group title="New threads">
        <Row
          title="Default project folder"
          subtitle="Where a new thread starts unless the composer's folder was changed."
          status={defaultCwd || "None chosen"}
        >
          <Action onClick={() => void pickFolder(defaultCwd).then((p) => p && setDefaultCwd(p))}>
            Choose…
          </Action>
        </Row>
        <Row title="Harness, model and effort" subtitle="Which agent runs a new thread by default.">
          <Action onClick={() => openSettings("agents")}>Agents…</Action>
        </Row>
        <Row
          title="Default access"
          reset={
            defaultAccess !== SETTINGS_DEFAULTS.defaultPermissionMode
              ? () => setDefaultPermissionMode(SETTINGS_DEFAULTS.defaultPermissionMode)
              : undefined
          }
          subtitle="New threads start here. The shield under the message box changes one thread only."
        >
          <Choice
            label="Default access"
            value={defaultAccess}
            options={PERMISSION_MODES.map((m) => ({ value: m.id, label: m.label }))}
            onChange={(value) => setDefaultPermissionMode(value as PermissionMode)}
          />
        </Row>
      </Group>
      <Group title="Threads">
        <Row
          title="Continue threads after a restart"
          reset={resume ? undefined : () => setResumeAfterRestart(SETTINGS_DEFAULTS.resumeAfterRestart)}
          subtitle="A thread whose turn was cut short by quitting, an update or an engine restart is sent “Continue where you left off” when the app is back."
        >
          <Switch
            label="Continue threads after a restart"
            checked={resume}
            onChange={setResumeAfterRestart}
          />
        </Row>
        <Row
          title="Continue threads when a usage limit resets"
          reset={afterLimit ? undefined : () => setContinueAfterLimit(SETTINGS_DEFAULTS.continueAfterLimit)}
          subtitle="A thread stopped by its plan's limit is sent “Continue where you left off” when the limit resets, unless you have moved it to another harness or account, or sent it something else."
        >
          <Switch
            label="Continue threads when a usage limit resets"
            checked={afterLimit}
            onChange={setContinueAfterLimit}
          />
        </Row>
      </Group>
      <Group title="Notifications">
        <Row
          title="System notifications"
          reset={
            notifications ? undefined : () => setSystemNotifications(SETTINGS_DEFAULTS.systemNotifications)
          }
          subtitle="A notification when a thread needs your approval, asks you a question, hits a usage limit or finishes. The app asks macOS for permission the first time one is due."
        >
          <Switch label="System notifications" checked={notifications} onChange={setSystemNotifications} />
        </Row>
        <Row
          title="Only when RustCode is in the background"
          reset={
            backgroundOnly
              ? undefined
              : () => setNotificationsBackgroundOnly(SETTINGS_DEFAULTS.notificationsBackgroundOnly)
          }
          subtitle="While the window is in front, a toast says it instead, and the thread's row keeps it. A thread on screen is never announced either way."
        >
          <Switch
            label="Only when RustCode is in the background"
            checked={backgroundOnly}
            onChange={setNotificationsBackgroundOnly}
          />
        </Row>
      </Group>
      <Group title="This Mac">
        <PresenceRow />
        <Row
          title="Trackpad haptics"
          reset={
            haptics
              ? undefined
              : () => {
                  setHaptics(HAPTICS_DEFAULT);
                  setHapticsState(HAPTICS_DEFAULT);
                }
          }
          subtitle="A light tap on the Force Touch trackpad when you send, stop a turn, or rest on Send."
        >
          <Switch
            label="Trackpad haptics"
            checked={haptics}
            onChange={(v) => {
              setHaptics(v);
              setHapticsState(v);
              if (v) haptic("generic");
            }}
          />
        </Row>
      </Group>
    </>
  );
}

function ChatPage() {
  const smartUi = useApp((s) => s.smartUi);
  const smartUiPending = useApp((s) => s.smartUiPending);
  const chatWidth = useApp((s) => s.chatWidth);
  const anyRunning = useApp((s) => Object.values(s.threads).some((t) => t.running));
  const [linkPreviews, setLinkPreviewsState] = useState(linkPreviewsEnabled);
  return (
    <Group title="In the chat">
      <Row
        title="Chat width"
        reset={
          chatWidth !== SETTINGS_DEFAULTS.chatWidth
            ? () => setChatWidth(SETTINGS_DEFAULTS.chatWidth)
            : undefined
        }
        subtitle="How wide the transcript and the composer grow on a large window. They always share one measure."
      >
        <Choice<ChatWidth>
          label="Chat width"
          value={chatWidth}
          onChange={setChatWidth}
          options={[
            { value: "comfortable", label: CHAT_WIDTH_LABEL.comfortable },
            { value: "wide", label: CHAT_WIDTH_LABEL.wide },
            { value: "full", label: CHAT_WIDTH_LABEL.full },
          ]}
        />
      </Row>
      <Row
        title="Smart UI"
        reset={smartUi ? undefined : () => setSmartUi(SETTINGS_DEFAULTS.smartUi)}
        subtitle="The agent can answer with a small interactive view (a filterable table, a chart with controls) that runs in the chat, sandboxed, with no network. Its buttons can send a message for you, only when you click them."
      >
        <Switch label="Smart UI" checked={smartUi} onChange={setSmartUi} />
      </Row>
      {smartUiPending && (
        <Row
          title="Restart the engine to apply"
          subtitle="The engine learns the Smart UI setting when it starts."
        >
          <Action prominent disabled={anyRunning} onClick={() => void restartEngine()}>
            Restart Engine
          </Action>
        </Row>
      )}
      <Row
        title="Link previews"
        reset={
          linkPreviews
            ? undefined
            : () => {
                setLinkPreviews(LINK_PREVIEWS_DEFAULT);
                setLinkPreviewsState(LINK_PREVIEWS_DEFAULT);
              }
        }
        subtitle="Links show as chips with the page's title, icon and colour. The app fetches them itself, with no cookies, from public hosts only."
      >
        <Switch
          label="Link previews"
          checked={linkPreviews}
          onChange={(v) => {
            setLinkPreviews(v);
            setLinkPreviewsState(v);
          }}
        />
      </Row>
      <HeaderButtonsSetting />
    </Group>
  );
}

function AgentsPage() {
  const snapshot = useApp((s) => s.harnesses);
  const [saved, setSaved] = useState(false);
  const [rechecking, setRechecking] = useState(false);
  // Installed, signed in, versions: detected again each time the page opens, and on Refresh.
  const recheck = async () => {
    setRechecking(true);
    await refreshHarnesses();
    setRechecking(false);
  };
  // biome-ignore lint/correctness/useExhaustiveDependencies: once per visit to the page
  useEffect(() => {
    void recheck();
  }, []);
  // An unsaved choice stays here while it is not valid yet (a harness that still lists models).
  const [choice, setChoice] = useState<NewThreadChoice | null>(null);
  if (!snapshot) return <Loading error={null} />;
  // The default saves as it changes: there is nothing to confirm, and the choice is easy to undo.
  const change = (next: NewThreadChoice) => {
    setSaved(false);
    setChoice(next);
    if (validateDefault(snapshot, next)) return;
    void setDefaultHarness(next).then((ok) => {
      if (!ok) return;
      setChoice(null);
      setSaved(true);
    });
  };
  return (
    <>
      {snapshot.unsupported && (
        <p className="mb-4 rounded-lg bg-warning/10 px-3.5 py-2.5 text-callout">
          This engine does not run other harnesses yet (no{" "}
          <code className="font-mono">_rustcode/harnesses</code>). Threads use RustCode's local engine. Update
          RustCode to add Claude Code, Codex, Gemini and others.
        </p>
      )}
      {!snapshot.unsupported && (
        <Group
          title="New threads start on"
          action={
            saved ? (
              <span className="flex items-center gap-1 text-caption text-success">
                <Check size={12} /> Saved
              </span>
            ) : undefined
          }
          footer="A thread can still switch harness and model from the composer."
        >
          <DefaultsForm snapshot={snapshot} value={choice ?? snapshot.default} onChange={change} />
        </Group>
      )}
      <Group
        title="Coding agents"
        action={
          <button
            type="button"
            onClick={() => void recheck()}
            disabled={rechecking}
            className="flex items-center gap-1 text-caption text-fg-muted hover:text-fg disabled:opacity-60"
          >
            <RefreshCw size={11} className={rechecking ? "animate-spin" : undefined} />
            {rechecking ? "Checking…" : "Refresh"}
          </button>
        }
        footer="Each agent signs in on its own; RustCode never stores its credentials. Advanced runs setup again."
      >
        <HarnessList snapshot={snapshot} />
      </Group>
      <OrchestrationGroup />
    </>
  );
}

/** Threads that start and message other threads (`state/host.ts`, docs/orchestrator.md). */
function OrchestrationGroup() {
  const orchestration = useApp((s) => s.orchestration);
  const threadLimit = useApp((s) => s.threadLimit);
  const readAcrossProjects = useApp((s) => s.readAcrossProjects);
  const pending = useApp((s) => s.orchestrationPending);
  const anyRunning = useApp((s) => Object.values(s.threads).some((t) => t.running));
  const limits = Array.from({ length: MAX_ACTIVE_LIMIT - MIN_ACTIVE_LINKS + 1 }, (_, i) => ({
    value: String(MIN_ACTIVE_LINKS + i),
    label: `${MIN_ACTIVE_LINKS + i} thread${MIN_ACTIVE_LINKS + i === 1 ? "" : "s"}`,
  }));
  return (
    <Group title="Orchestration">
      <Row
        title="Threads can start and message other threads"
        reset={orchestration ? undefined : () => setOrchestration(SETTINGS_DEFAULTS.orchestration)}
        subtitle="An agent can list and read threads, message one in this project, start new threads (in their own worktrees, on any harness, model or named account) and run one task on several harnesses to compare. A thread at Full access does this without asking; Auto-edit may list, read and wait. Otherwise you approve each: Allow for This Thread, Allow Once or Don't Allow. A thread it starts gets the lower of this thread's access and Default access, so it is never given more than the thread that started it."
      >
        <Switch
          label="Threads can start and message other threads"
          checked={orchestration}
          onChange={setOrchestration}
        />
      </Row>
      <Row
        title="Threads one thread can run at once"
        reset={
          threadLimit === SETTINGS_DEFAULTS.threadLimit
            ? undefined
            : () => setThreadLimit(SETTINGS_DEFAULTS.threadLimit)
        }
        subtitle={`How many distinct threads one thread may have working at the same time. A message to a thread it is already running is a follow-up and takes no slot. The whole delegation tree holds twice as many: ${threadLimit * 2} now.`}
      >
        <Select
          label="Threads one thread can run at once"
          value={String(threadLimit)}
          options={limits}
          onChange={(value) => setThreadLimit(Number(value))}
        />
      </Row>
      <Row
        title="Let agents read threads in other projects"
        reset={
          readAcrossProjects ? undefined : () => setReadAcrossProjects(SETTINGS_DEFAULTS.readAcrossProjects)
        }
        subtitle="When on, an agent can list, search and read any thread, including in other projects. Reading a thread still follows the approval rules above. Messaging and starting threads stay in the project."
      >
        <Switch
          label="Let agents read threads in other projects"
          checked={readAcrossProjects}
          onChange={setReadAcrossProjects}
        />
      </Row>
      {pending && (
        <Row title="Restart the engine to apply" subtitle="The engine learns this setting when it starts.">
          <Action prominent disabled={anyRunning} onClick={() => void restartEngine()}>
            Restart Engine
          </Action>
        </Row>
      )}
    </Group>
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
      <Group
        title="Model accounts"
        footer="Sign in or out with `/login` in a thread, or switch with the model picker's Account row."
      >
        {data.accounts.length ? (
          data.accounts.map((a, i) => (
            <Row
              key={i}
              title={a.display || a.provider}
              subtitle={`${a.provider} · ${a.method}${a.needsSignIn ? " · sign in again" : ""}`}
            >
              {a.active && !a.needsSignIn && <span className="text-caption text-success">Active</span>}
            </Row>
          ))
        ) : (
          <Row title="No accounts" />
        )}
      </Group>
    </>
  );
}

// ---------------------------------------------------------------- appearance

/** A small picture of the app in a theme's own colours: sidebar, rows, a reply and the composer. */
function ThemePreview({ theme }: { theme: Theme }) {
  const t = theme.tokens;
  const r = theme.radius === "sharp" ? 2 : theme.radius === "round" ? 7 : 4;
  return (
    <div className="flex h-[74px] w-full overflow-hidden" style={{ background: t.bg }} aria-hidden>
      <div className="flex w-[34%] flex-col gap-1 p-1.5" style={{ background: t.sidebar }}>
        <div className="h-2.5" style={{ background: t.surface_2, borderRadius: r }} />
        <div className="h-1.5 w-4/5" style={{ background: t.text_muted, opacity: 0.5, borderRadius: 2 }} />
        <div className="h-1.5 w-3/5" style={{ background: t.text_faint, opacity: 0.5, borderRadius: 2 }} />
      </div>
      <div className="flex flex-1 flex-col justify-between p-1.5">
        <div className="flex flex-col gap-1">
          <div className="ml-auto h-2.5 w-1/2" style={{ background: t.surface_2, borderRadius: r }} />
          <div className="h-1.5 w-4/5" style={{ background: t.text, opacity: 0.7, borderRadius: 2 }} />
          <div className="h-1.5 w-2/3" style={{ background: t.text_muted, opacity: 0.6, borderRadius: 2 }} />
        </div>
        <div
          className="flex h-4 items-center justify-end px-1"
          style={{ background: t.surface, borderRadius: r + 2, boxShadow: `0 0 0 1px ${t.border}` }}
        >
          <div className="size-2 rounded-full" style={{ background: t.accent }} />
        </div>
      </div>
    </div>
  );
}

function ThemeCard({ theme, active, onPick }: { theme: Theme; active: boolean; onPick: () => void }) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={active}
      onClick={onPick}
      className="group flex flex-col gap-1.5 text-left"
    >
      <span
        className={cx(
          "relative block overflow-hidden rounded-md shadow-1 transition-shadow duration-150",
          active ? "outline-2 outline-offset-2 outline-fg" : "group-hover:shadow-2",
        )}
      >
        <ThemePreview theme={theme} />
        {active && (
          <span className="absolute top-1 right-1 flex size-4 items-center justify-center rounded-full bg-fg text-bg">
            <Check size={10} strokeWidth={3} />
          </span>
        )}
      </span>
      <span className="flex items-baseline gap-1.5 px-0.5">
        <span className={cx("truncate text-callout", active ? "font-medium text-fg" : "text-fg-muted")}>
          {theme.name}
        </span>
        <span className="shrink-0 text-caption text-fg-faint">
          {theme.source === "builtin" ? theme.mode : theme.source}
        </span>
      </span>
    </button>
  );
}

function NewTheme({ base, onDone }: { base: string; onDone: () => void }) {
  const [name, setName] = useState("");
  const [cli, setCli] = useState<string | null>(null);
  const valid = /^[a-z0-9][a-z0-9-]{0,63}$/.test(name);
  const create = async () => {
    try {
      const path = await createTheme(name, base);
      if (path === null) {
        setCli(`rustcode theme new ${name} --from ${base}`);
        return;
      }
      toast(path ? `Created ${path}` : "Theme created", "success");
      onDone();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  };
  return (
    <div className="rc-expand flex flex-col gap-2 border-border border-t px-4 py-3.5">
      <div className="flex items-center gap-2">
        <input
          autoFocus
          value={name}
          onChange={(e) => {
            setName(e.target.value.toLowerCase());
            setCli(null);
          }}
          onKeyDown={(e) => e.key === "Enter" && valid && void create()}
          placeholder="theme-name"
          aria-label="Theme name"
          className={cx(fieldClass, "flex-1 font-mono")}
        />
        <Action onClick={onDone}>Cancel</Action>
        <Action prominent disabled={!valid} onClick={() => void create()}>
          Create
        </Action>
      </div>
      {cli ? (
        <p className="m-0 text-caption text-fg-muted">
          This engine cannot create themes from the app. In a terminal:{" "}
          <code className="selectable rounded-xs bg-fill px-1 py-px font-mono text-fg">{cli}</code>
        </p>
      ) : (
        <p className="m-0 text-caption text-fg-faint">A copy of {base} in your themes folder, to edit.</p>
      )}
    </div>
  );
}

function AppearancePage() {
  const mode = useApp((s) => s.theme);
  const roundedFont = useApp((s) => s.roundedFont);
  const animateStreaming = useApp((s) => s.animateStreaming);
  const active = useApp((s) => s.themeActive);
  const snapshot = useApp((s) => s.themes);
  const supported = useApp((s) => s.features.themes);
  const [creating, setCreating] = useState(false);
  const themes = snapshot?.themes.length ? snapshot.themes : BUILTIN_THEMES;
  const userDir =
    themes.find((t) => t.source === "user" && t.path)?.path?.replace(/\/[^/]+$/, "") ??
    "~/.config/rustcode/themes";
  return (
    <>
      <Group title="Mode">
        <Row
          title="Light or dark"
          reset={mode !== SETTINGS_DEFAULTS.theme ? () => setTheme(SETTINGS_DEFAULTS.theme) : undefined}
          subtitle="System follows macOS. Picking a theme sets its mode."
        >
          <Choice
            label="Appearance"
            value={mode}
            onChange={setTheme}
            options={[
              { value: "system", label: "System" },
              { value: "light", label: "Light" },
              { value: "dark", label: "Dark" },
            ]}
          />
        </Row>
        <Row
          title="Rounded font"
          reset={roundedFont ? () => setRoundedFont(SETTINGS_DEFAULTS.roundedFont) : undefined}
          subtitle="SF Pro Rounded for the interface and chat. Code stays monospaced."
        >
          <Switch label="Rounded font" checked={roundedFont} onChange={setRoundedFont} />
        </Row>
        <Row
          title="Animate streaming text"
          reset={animateStreaming ? undefined : () => setAnimateStreaming(SETTINGS_DEFAULTS.animateStreaming)}
          subtitle="A reply's new words fade in as the model writes them. Finished replies never animate."
        >
          <Switch label="Animate streaming text" checked={animateStreaming} onChange={setAnimateStreaming} />
        </Row>
      </Group>
      <Group
        title="Theme"
        action={
          <div className="flex items-center gap-1">
            {hasShell() && (
              <Action quiet size="sm" onClick={() => revealPath(userDir)}>
                <FolderOpen size={12} /> Open themes folder
              </Action>
            )}
            <Action quiet size="sm" onClick={() => setCreating(!creating)} aria-expanded={creating}>
              <Plus size={12} /> New theme…
            </Action>
          </div>
        }
        footer={
          supported === false
            ? "This engine has no themes yet, so only the built-in ones show. Update RustCode for theme files."
            : `Theme files live in ${userDir} and a project's .rustcode/themes; edits apply live. See docs/themes.md.`
        }
      >
        <div
          role="radiogroup"
          aria-label="Theme"
          className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-4 p-4"
        >
          {themes.map((t) => (
            <ThemeCard
              key={`${t.source}/${t.name}`}
              theme={t}
              active={t.name === active && (mode === "system" ? true : t.mode === mode)}
              onPick={() => void setActiveTheme(t.name)}
            />
          ))}
        </div>
        {creating && <NewTheme base={active} onDone={() => setCreating(false)} />}
      </Group>
      <Group title="New threads">
        <BannerSetting />
      </Group>
      <TokenList />
    </>
  );
}

/** The live tokens, for theme authors: what each variable resolves to now. */
function TokenList() {
  const [open, setOpen] = useState(false);
  return (
    <section className="mb-8">
      <SectionTitle
        title="Tokens"
        action={
          <Action quiet size="sm" onClick={() => setOpen(!open)} aria-expanded={open}>
            {open ? "Hide" : "Show"}
          </Action>
        }
      />
      {open && (
        <div className="rc-expand grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-2 rounded-xl border border-border-strong bg-surface-2/40 p-3">
          {TOKENS.map((t) => (
            <div
              key={t}
              className="flex items-center gap-2 rounded-lg border border-border-strong bg-surface-2/60 px-2.5 py-1.5"
            >
              <span
                className="size-4 shrink-0 rounded-xs shadow-1"
                style={{ background: `var(${cssName(t)})` }}
              />
              <span className="truncate font-mono text-caption text-fg-muted">{t}</span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

/** Settings ▸ Appearance ▸ New thread banner, after SettingsView.swift. */
function BannerSetting() {
  const banner = useBanner();
  const choose = () =>
    void chooseBanner().catch((e) =>
      toast(`Could not use that picture: ${e instanceof Error ? e.message : e}`, "error"),
    );
  // No picture is the default: choosing one is the user's call, so removing it is the reset.
  const reset = () => {
    void removeBanner();
    setBannerStyle(BANNER_STYLE_DEFAULT);
  };
  return (
    <div>
      <Row
        title="New thread banner"
        reset={banner.name !== BANNER_NAME_DEFAULT ? reset : undefined}
        subtitle={
          banner.name
            ? "Behind the top of new threads, fading toward the message field"
            : "A wide picture over new threads, drawn in text or as is. Cropped to fit"
        }
      >
        <div className="flex shrink-0 items-center gap-2">
          {banner.name && (
            <>
              <Choice
                label="Banner style"
                value={banner.style}
                onChange={setBannerStyle}
                options={[
                  { value: "ascii", label: "Text" },
                  { value: "picture", label: "Picture" },
                ]}
              />
              <Action onClick={() => void removeBanner()}>Remove</Action>
            </>
          )}
          <Action onClick={choose}>{banner.name ? "Change…" : "Choose…"}</Action>
        </div>
      </Row>
      {banner.name && (
        // As a new thread shows it, at a size that reads.
        <div className="px-4 pb-4">
          <div className="overflow-hidden rounded-lg border border-border-strong bg-bg">
            <NewThreadBanner key={banner.style} name={banner.name} style={banner.style} height={240} />
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * The app's version and its update, like the SwiftUI app's UpdateControl:
 * Check for Updates, then Update to X (macOS installs in place, quits and
 * reopens) or Download X (the release page, where the installer updates).
 */
function AppVersion() {
  const info = useUpdates((s) => s.info);
  const installing = useUpdates((s) => s.installing);
  const progress = useUpdates((s) => s.progress);
  const background = useUpdates((s) => s.error);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const shellVersion = useUpdates((s) => s.version);
  const version = info?.current ?? shellVersion ?? APP_VERSION;
  useEffect(() => checkIfStale(), []);
  const check = async () => {
    setChecking(true);
    setError(null);
    await checkNow();
    // A check that failed says so, even when an earlier one found the app up to date.
    setError(useUpdates.getState().error);
    setChecking(false);
  };
  const install = async () => {
    if (!info?.canInstall) {
      if (info?.page) openExternal(info.page);
      return;
    }
    setError(null);
    // One install path: the sidebar row shows the same progress when it is visible.
    const failed = await applyUpdate();
    if (failed) setError(failed);
  };
  const { fraction, step } = installProgress(progress);
  const failure = error ?? (info ? null : background);
  const subtitle = failure
    ? failure
    : installing
      ? step
      : info && !info.available
        ? "RustCode is up to date."
        : info?.development
          ? "A development build; it does not update itself."
          : undefined;
  return (
    <Row
      title={`RustCode ${version}`}
      subtitle={
        <span className={failure ? "text-warning" : undefined}>
          {subtitle ?? "App version"}
          {installing && (
            <span
              role="progressbar"
              aria-label="Update progress"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={fraction === undefined ? undefined : Math.round(fraction * 100)}
              className="mt-1.5 block h-[4px] max-w-[280px] overflow-hidden rounded-full bg-fill-strong"
            >
              <span
                className={cx(
                  "block h-full rounded-full bg-accent transition-[width] duration-200 ease-snappy",
                  fraction === undefined && "w-1/3 animate-pulse",
                )}
                style={fraction === undefined ? undefined : { width: `max(4px, ${fraction * 100}%)` }}
              />
            </span>
          )}
        </span>
      }
    >
      {canUpdate() && <Action onClick={() => void showWhatsNew(version)}>What's New</Action>}
      {!canUpdate() ? (
        <Info size={14} className="text-fg-faint" />
      ) : info?.available && info.latest ? (
        <Action prominent disabled={installing} onClick={() => void install()}>
          {installing ? (
            <span className="flex items-center gap-1.5">
              <Spinner size={12} />
              {fraction === undefined ? "Updating…" : `Updating… ${Math.round(fraction * 100)}%`}
            </span>
          ) : info.canInstall ? (
            `Update to ${info.latest}`
          ) : (
            `Download ${info.latest}`
          )}
        </Action>
      ) : (
        <Action disabled={checking} onClick={() => void check()}>
          {checking ? "Checking…" : "Check for Updates"}
        </Action>
      )}
    </Row>
  );
}

function AdvancedPage() {
  const status = useApp((s) => s.status);
  const detail = useApp((s) => s.statusDetail);
  const transport = useApp((s) => s.transport);
  const log = useApp((s) => s.engineLog);
  return (
    <>
      <Group title="Engine">
        <Row
          title={status === "ready" ? "Running" : status === "connecting" ? "Starting…" : "Stopped"}
          subtitle="rustcode --acp is the engine every thread runs on."
          status={
            detail ??
            (transport === "tauri"
              ? "Run by the app"
              : transport === "websocket"
                ? "Through the dev bridge"
                : "Mock engine (no real turns)")
          }
        >
          <Action onClick={() => void restartEngine()}>Restart</Action>
        </Row>
      </Group>
      <Group title="Engine log" footer="The engine's stderr; turns log to each thread's logs/debug.log.">
        <pre className="selectable m-0 max-h-[320px] overflow-auto p-3 font-mono text-caption leading-[15px] whitespace-pre-wrap text-fg-muted">
          {log.length ? log.slice(-200).join("\n") : "Nothing yet."}
        </pre>
      </Group>
      <Group title="Setup" footer="Each agent signs in on its own; RustCode never stores its credentials.">
        <Row
          title="Set up your agents again"
          subtitle="Walks through finding each agent, signing it in and checking that it runs."
        >
          <Action onClick={() => setOnboarding(true)}>Run setup again</Action>
        </Row>
      </Group>
    </>
  );
}

function AboutPage() {
  return (
    <Group
      title="Version"
      footer="RustCode is open source. The licences of everything it ships are in its release."
    >
      <AppVersion />
    </Group>
  );
}

// ---------------------------------------------------------------- the panel

/**
 * Settings as a floating panel over the app (docs/design.md › Settings): a
 * nav column with the search field at its top, then one page beside it under
 * the panel's own header (breadcrumb, "Restore defaults", close). The app
 * stays mounted and inert behind a scrim; a press on the scrim, Esc or ⌘, puts
 * you back where you were.
 */
export function Settings() {
  const section = useApp((s) => s.settings) ?? "general";
  const current = PAGES.find((s) => s.id === section) ?? PAGES[0]!;
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const field = useRef<HTMLInputElement>(null);
  const results = useMemo(() => searchSettings(query), [query]);
  // The row to land on, set when a result is picked and cleared once it is lit.
  const [landing, setLanding] = useState<{ section: SettingsSection; title: string } | null>(null);
  // The panel takes focus when it opens: keys then land inside it, never on the
  // shell it covers. Not its first control, so no stray ring in the search field.
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    panel.current?.focus({ preventScroll: true });
  }, []);

  const clear = () => {
    setQuery("");
    setActive(0);
  };

  const pick = (entry: SettingEntry) => {
    setLanding({ section: entry.section, title: entry.title });
    openSettings(entry.section);
    clear();
    field.current?.blur();
  };

  // The picked row lands: scroll to it once the page is on screen, then light it
  // for a moment. The class comes off in a timer of its own, so the effect can
  // clear its target at once without leaving a row lit.
  useEffect(() => {
    if (!landing || landing.section !== section) return;
    let frame = 0;
    let again = 0;
    frame = requestAnimationFrame(() => {
      again = requestAnimationFrame(() => {
        setLanding(null);
        const target = findSetting(landing.title);
        if (!target) return;
        target.scrollIntoView({ block: "center" });
        target.classList.add("rc-setting-hit");
        window.setTimeout(() => target.classList.remove("rc-setting-hit"), 1200);
      });
    });
    return () => {
      cancelAnimationFrame(frame);
      cancelAnimationFrame(again);
    };
  }, [landing, section]);

  // "/" puts the cursor in the search field, unless a field already has it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "/" || e.metaKey || e.ctrlKey || e.altKey) return;
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, select, [contenteditable='true']")) return;
      e.preventDefault();
      field.current?.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // ↑↓ move through the results, ⏎ picks one. The first Esc clears the search and
  // stops there; with nothing left to clear it closes the panel instead.
  const onFieldKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!results.length) return;
      const step = e.key === "ArrowDown" ? 1 : -1;
      setActive((i) => (i + step + results.length) % results.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const entry = results[active];
      if (entry) pick(entry);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (query) clear();
      else openSettings(null);
    }
  };

  return (
    <div role="dialog" aria-modal="true" aria-label="Settings" className="fixed inset-0 z-30">
      {/* The app behind, dimmed; a press on it (not on the panel) closes Settings. */}
      <div aria-hidden className="absolute inset-0 bg-bg/60" />
      <div
        className="absolute inset-0 flex items-center justify-center p-1 sm:p-6 lg:p-10"
        onMouseDown={(e) => {
          if (e.target === e.currentTarget) openSettings(null);
        }}
      >
        <div
          ref={panel}
          tabIndex={-1}
          className="relative flex size-full max-h-[60rem] max-w-[80rem] overflow-hidden rounded-2xl border border-border bg-bg shadow-2 outline-none"
        >
          <RowResets>
            <nav
              aria-label="Settings sections"
              className="flex w-60 shrink-0 flex-col gap-3 border-border border-e bg-sidebar px-2 py-3"
            >
              <div className="flex h-8 shrink-0 items-center gap-2 rounded-md bg-fill-subtle px-2 text-body focus-within:bg-fill">
                <Search size={16} className="shrink-0 text-fg-faint" />
                <input
                  ref={field}
                  value={query}
                  onChange={(e) => {
                    setQuery(e.target.value);
                    setActive(0);
                  }}
                  onKeyDown={onFieldKey}
                  placeholder="Search settings"
                  aria-label="Search settings"
                  className="min-w-0 flex-1 bg-transparent font-medium text-fg outline-none placeholder:text-fg-muted"
                />
                {query ? (
                  <button
                    type="button"
                    aria-label="Clear search"
                    onClick={() => {
                      clear();
                      field.current?.focus();
                    }}
                    className="flex size-4 shrink-0 items-center justify-center rounded-xs text-fg-faint hover:text-fg"
                  >
                    <X size={12} strokeWidth={2.5} />
                  </button>
                ) : (
                  <Kbd>/</Kbd>
                )}
              </div>
              <div className="scroll-stable flex min-h-0 flex-1 flex-col gap-4 pb-1">
                {query.trim() ? (
                  results.length ? (
                    <div role="listbox" aria-label="Settings results" className="flex flex-col gap-0.5">
                      {results.map((r, i) => (
                        <button
                          key={`${r.section}:${r.title}`}
                          type="button"
                          role="option"
                          aria-selected={i === active}
                          onMouseMove={() => setActive(i)}
                          onClick={() => pick(r)}
                          className={cx(
                            "flex flex-col items-start rounded-md px-2 py-1.5 text-left",
                            i === active ? "bg-fill" : "hover:bg-fill",
                          )}
                        >
                          <span className="w-full truncate font-medium text-body">{r.title}</span>
                          <span className="text-caption text-fg-faint">{pageLabel(r.section)}</span>
                        </button>
                      ))}
                    </div>
                  ) : (
                    <p className="px-2 py-1.5 text-caption text-fg-muted">
                      Nothing matches “{query.trim()}”.
                    </p>
                  )
                ) : (
                  NAV.map((group) => (
                    <div key={group.heading ?? group.pages[0]?.id} className="flex flex-col gap-1">
                      {group.heading && (
                        <div className="px-2 pb-0.5 font-medium text-callout text-fg-muted">
                          {group.heading}
                        </div>
                      )}
                      {group.pages.map((s) => (
                        <button
                          key={s.id}
                          type="button"
                          aria-current={s.id === section ? "page" : undefined}
                          onClick={() => openSettings(s.id)}
                          className={cx(
                            "flex h-8 items-center gap-2 rounded-md px-2 text-left font-medium text-body",
                            s.id === section
                              ? "bg-fill text-fg"
                              : "text-fg-muted hover:bg-fill hover:text-fg",
                          )}
                        >
                          <s.icon size={16} className="shrink-0" />
                          {s.label}
                        </button>
                      ))}
                    </div>
                  ))
                )}
              </div>
            </nav>
            <div className="flex min-w-0 flex-1 flex-col">
              <PanelHeader label={current.label} />
              <main className="scroll-stable min-h-0 flex-1">
                <div className="mx-auto w-full max-w-[760px] px-4 pt-5 pb-24">
                  {section === "general" && <GeneralPage />}
                  {section === "chat" && <ChatPage />}
                  {section === "agents" && <AgentsPage />}
                  {section === "connections" && <ConnectionsPage />}
                  {section === "capabilities" && <CapabilitiesPage />}
                  {section === "appearance" && <AppearancePage />}
                  {section === "keyboard" && <ShortcutsPage />}
                  {section === "usage" && <UsagePage />}
                  {section === "statistics" && <StatisticsPage />}
                  {section === "advanced" && <AdvancedPage />}
                  {section === "about" && <AboutPage />}
                </div>
              </main>
            </div>
          </RowResets>
        </div>
      </div>
    </div>
  );
}

/**
 * The panel's own header: where you are, then the page's whole-page action and
 * the close button. "Restore defaults" only shows while a row on this page can
 * go back to its default, and puts back everything that changed at once
 * (settingsKit.tsx › RowResets).
 */
function PanelHeader({ label }: { label: string }) {
  const resets = useRowResets();
  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-border border-b ps-5 pe-2">
      <nav aria-label="Settings breadcrumb" className="min-w-0">
        <ol className="m-0 flex min-w-0 list-none items-center gap-2 p-0 text-body">
          <li className="shrink-0 font-medium text-fg-muted">Settings</li>
          <li aria-hidden className="shrink-0 text-fg-faint">
            /
          </li>
          <li aria-current="page" className="min-w-0 truncate font-medium text-fg">
            {label}
          </li>
        </ol>
      </nav>
      <div className="ms-auto flex shrink-0 items-center gap-1">
        {resets.count > 0 && (
          <Action
            quiet
            onClick={() => {
              resets.restore();
            }}
          >
            <RotateCcw size={13} />
            Restore defaults
          </Action>
        )}
        <ChromeButton label="Close settings" tooltip="Close (Esc)" onClick={() => openSettings(null)}>
          <X size={16} />
        </ChromeButton>
      </div>
    </header>
  );
}

/**
 * The row (or group) the search index named, by its title. A few titles carry
 * live state after them (the engine's status, the app's version), so the index
 * names the part that stays and a prefix finds it.
 */
function findSetting(title: string): Element | null {
  const nodes = [...document.querySelectorAll("[data-setting]")];
  return (
    nodes.find((n) => n.getAttribute("data-setting") === title) ??
    nodes.find((n) => (n.getAttribute("data-setting") ?? "").startsWith(title)) ??
    null
  );
}
