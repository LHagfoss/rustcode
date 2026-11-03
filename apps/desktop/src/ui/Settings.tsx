// Settings, inside the app's shell: the sidebar stays as it is and the main
// column shows a section list beside the page, so opening Settings remounts
// nothing and does not flash. After SettingsView.swift. Engine-backed pages
// (Harnesses, Connections, Capabilities, Appearance's themes) read and
// write through `_rustcode/*` requests.

import {
  Blocks,
  Check,
  ChevronLeft,
  FolderOpen,
  Gauge,
  Settings as Gear,
  Info,
  KeyRound,
  Layers,
  Palette,
  Plus,
  RefreshCw,
  TerminalSquare,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { arr, bool, type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { chooseBanner, removeBanner, setBannerStyle, useBanner } from "../lib/banner";
import { haptic, hapticsEnabled, setHaptics } from "../lib/haptics";
import { linkPreviewsEnabled, setLinkPreviews } from "../lib/links";
import { openExternal, pickFolder, revealPath } from "../lib/native";
import { hasShell } from "../lib/shell";
import { BUILTIN_THEMES, cssName, type Theme, TOKENS } from "../lib/themes";
import { canUpdate } from "../lib/updater";
import { validateDefault } from "../state/harness";
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
  type SettingsSection,
  setActiveTheme,
  setAnimateStreaming,
  setDefaultCwd,
  setDefaultHarness,
  setDefaultPermissionMode,
  setOnboarding,
  setOrchestration,
  setResumeAfterRestart,
  setRoundedFont,
  setSmartUi,
  setTheme,
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
import { HEADER_HEIGHT, MainHeader } from "./MainHeader";
import { NewThreadBanner } from "./NewThreadBanner";
import { cx, Kbd, Pill, Segments, Spinner, Switch, Tooltip } from "./primitives";
import { Group, inputClass, Loading, Row, useEngineData } from "./settingsKit";
import { UsagePage } from "./UsagePage";

const SECTIONS: { id: SettingsSection; label: string; icon: typeof Gear }[] = [
  { id: "general", label: "General", icon: Gear },
  { id: "harnesses", label: "Harnesses", icon: Layers },
  { id: "connections", label: "Connections", icon: KeyRound },
  { id: "capabilities", label: "Capabilities", icon: Blocks },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "usage", label: "Usage", icon: Gauge },
  { id: "engine", label: "Engine", icon: TerminalSquare },
];

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
  return (
    <Row title="Discord Rich Presence" subtitle={subtitle}>
      {state && <Switch label="Discord Rich Presence" checked={state.enabled} onChange={setEnabled} />}
    </Row>
  );
}

function General() {
  const defaultCwd = useApp((s) => s.defaultCwd);
  const [linkPreviews, setLinkPreviewsState] = useState(linkPreviewsEnabled);
  const [haptics, setHapticsState] = useState(hapticsEnabled);
  const smartUi = useApp((s) => s.smartUi);
  const smartUiPending = useApp((s) => s.smartUiPending);
  const orchestration = useApp((s) => s.orchestration);
  const resume = useApp((s) => s.resumeAfterRestart);
  const orchestrationPending = useApp((s) => s.orchestrationPending);
  const defaultAccess = useApp((s) => s.defaultPermissionMode);
  const anyRunning = useApp((s) => Object.values(s.threads).some((t) => t.running));
  const shortcuts: [string, string][] = [
    ["New thread", "⌘N"],
    ["Folder of this thread (a new one, or move this one)", "⌘O"],
    ["Default folder for new threads", "⌘⇧O"],
    ["Command palette: threads, actions", "⌘K"],
    ["Settings", "⌘,"],
    ["Toggle sidebar", "⌘B"],
    ["Find in thread", "⌘F"],
    ["Terminal", "⌘J"],
    ["Text larger, smaller, actual size", "⌘+  ⌘-  ⌘0"],
    ["Stop the turn", "Esc / ⌘."],
    ["Send · queue while running", "⏎"],
    ["Steer the running turn", "⌘⏎"],
    ["New line", "⇧⏎"],
    ["Answer an approval", "1–9"],
  ];
  return (
    <>
      <Group title="New threads">
        <Row title="Default project folder" subtitle={defaultCwd || "None chosen"}>
          <Pill onClick={() => void pickFolder(defaultCwd).then((p) => p && setDefaultCwd(p))}>Choose…</Pill>
        </Row>
        <Row title="Harness, model and effort" subtitle="Which agent runs a new thread by default.">
          <Pill onClick={() => openSettings("harnesses")}>Harnesses…</Pill>
        </Row>
      </Group>
      <Group title="Chat">
        <Row
          title="Default access"
          subtitle="New threads start here. The shield under the message box changes one thread only."
        >
          <Segments
            label="Default access"
            value={defaultAccess}
            options={PERMISSION_MODES.map((m) => ({ value: m.id, label: m.label }))}
            onChange={(value) => setDefaultPermissionMode(value as PermissionMode)}
          />
        </Row>
        <Row
          title="Smart UI"
          subtitle="The agent can answer with a small interactive view (a filterable table, a chart with controls) that runs in the chat, sandboxed, with no network. Its buttons can send a message for you, only when you click them."
        >
          <Switch label="Smart UI" checked={smartUi} onChange={setSmartUi} />
        </Row>
        {smartUiPending && (
          <Row
            title="Restart the engine to apply"
            subtitle="The engine learns the Smart UI setting when it starts."
          >
            <Pill prominent disabled={anyRunning} onClick={() => void restartEngine()}>
              Restart Engine
            </Pill>
          </Row>
        )}
        <Row
          title="Continue threads after a restart"
          subtitle="A thread whose turn was cut short by quitting, an update or an engine restart is sent “Continue where you left off” when the app is back."
        >
          <Switch
            label="Continue threads after a restart"
            checked={resume}
            onChange={setResumeAfterRestart}
          />
        </Row>
        <Row
          title="Threads can start and message other threads"
          subtitle="An agent can list and read this project's threads, message one, start new threads (in their own worktrees, on any harness) and run one task on several harnesses to compare. A thread at Full access does this without asking; Auto-edit may list, read and wait. Otherwise you approve each: Allow for This Thread, Allow Once or Don't Allow."
        >
          <Switch
            label="Threads can start and message other threads"
            checked={orchestration}
            onChange={setOrchestration}
          />
        </Row>
        {orchestrationPending && (
          <Row title="Restart the engine to apply" subtitle="The engine learns this setting when it starts.">
            <Pill prominent disabled={anyRunning} onClick={() => void restartEngine()}>
              Restart Engine
            </Pill>
          </Row>
        )}
        <Row
          title="Link previews"
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
        <Row
          title="Trackpad haptics"
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
        <PresenceRow />
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
        footer={
          <>
            Each agent signs in on its own; RustCode never stores its credentials.{" "}
            <button type="button" onClick={() => setOnboarding(true)} className="text-accent hover:underline">
              Run setup again
            </button>
          </>
        }
      >
        <HarnessList snapshot={snapshot} />
      </Group>
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
    <div className="rc-expand flex flex-col gap-2 border-border border-t p-3.5">
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
          className={cx(inputClass, "flex-1 font-mono")}
        />
        <Pill onClick={onDone}>Cancel</Pill>
        <Pill prominent disabled={!valid} onClick={() => void create()}>
          Create
        </Pill>
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

function Appearance() {
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
        <Row title="Light or dark" subtitle="System follows macOS. Picking a theme sets its mode.">
          <Segments
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
          subtitle="SF Pro Rounded for the interface and chat. Code stays monospaced."
        >
          <Switch label="Rounded font" checked={roundedFont} onChange={setRoundedFont} />
        </Row>
        <Row
          title="Animate streaming text"
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
              <button
                type="button"
                onClick={() => revealPath(userDir)}
                className="flex h-6 items-center gap-1 rounded-full px-2.5 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
              >
                <FolderOpen size={12} /> Open themes folder
              </button>
            )}
            <button
              type="button"
              onClick={() => setCreating(!creating)}
              aria-expanded={creating}
              className="flex h-6 items-center gap-1 rounded-full px-2.5 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
            >
              <Plus size={12} /> New theme…
            </button>
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
          className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-4 p-3.5"
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
    <section className="mb-6">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        aria-expanded={open}
        className="-mx-1 mb-2 rounded-md px-2 py-0.5 font-semibold text-callout text-fg-muted hover:bg-fill-subtle"
      >
        {open ? "Hide" : "Show"} tokens
      </button>
      {open && (
        <div className="rc-expand grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-2">
          {TOKENS.map((t) => (
            <div key={t} className="flex items-center gap-2 rounded-md bg-surface px-2 py-1.5">
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
  return (
    <div>
      <Row
        title="New thread banner"
        subtitle={
          banner.name
            ? "Behind the top of new threads, fading toward the message field"
            : "A wide picture over new threads, drawn in text or as is. Cropped to fit"
        }
      >
        <div className="flex shrink-0 items-center gap-2">
          {banner.name && (
            <>
              <Segments
                label="Banner style"
                value={banner.style}
                onChange={setBannerStyle}
                options={[
                  { value: "ascii", label: "Text" },
                  { value: "picture", label: "Picture" },
                ]}
              />
              <Pill onClick={() => void removeBanner()}>Remove</Pill>
            </>
          )}
          <Pill onClick={choose}>{banner.name ? "Change…" : "Choose…"}</Pill>
        </div>
      </Row>
      {banner.name && (
        // As a new thread shows it, at a size that reads.
        <div className="px-3.5 pb-3.5">
          <div className="overflow-hidden rounded-md bg-bg">
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
      {canUpdate() && <Pill onClick={() => void showWhatsNew(version)}>What's New</Pill>}
      {!canUpdate() ? (
        <Info size={14} className="text-fg-faint" />
      ) : info?.available && info.latest ? (
        <Pill prominent disabled={installing} onClick={() => void install()}>
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
        </Pill>
      ) : (
        <Pill disabled={checking} onClick={() => void check()}>
          {checking ? "Checking…" : "Check for Updates"}
        </Pill>
      )}
    </Row>
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
        <AppVersion />
      </Group>
      <Group title="Engine log" footer="The engine's stderr; turns log to each thread's logs/debug.log.">
        <pre className="selectable m-0 max-h-[320px] overflow-auto p-3 font-mono text-caption leading-[15px] whitespace-pre-wrap text-fg-muted">
          {log.length ? log.slice(-200).join("\n") : "Nothing yet."}
        </pre>
      </Group>
    </>
  );
}

/**
 * Settings over the whole window: the shared glass header (Back, the title)
 * clear of the traffic lights, and one calm centred column with the section
 * list at its left and the page beside it. The page scrolls under the header.
 * Back, Esc and ⌘, return to where you were; the thread underneath stayed
 * mounted (App.tsx).
 */
export function Settings() {
  const section = useApp((s) => s.settings) ?? "general";
  const current = SECTIONS.find((s) => s.id === section) ?? SECTIONS[0]!;
  return (
    <main className="relative flex min-h-0 min-w-0 flex-1 flex-col bg-bg" aria-label="Settings">
      <MainHeader
        lights
        title={
          <>
            <Tooltip label="Back (Esc)">
              <button
                type="button"
                onClick={() => openSettings(null)}
                className="flex h-7 items-center gap-1 rounded-full pr-2.5 pl-1.5 font-medium text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
              >
                <ChevronLeft size={14} />
                Back
              </button>
            </Tooltip>
            <h1 className="min-w-0 truncate font-semibold text-body">Settings</h1>
          </>
        }
      />
      <div className="scroll-stable min-h-0 flex-1">
        <div
          className="mx-auto flex w-full max-w-[840px] gap-8 px-[clamp(16px,4%,32px)] pb-16"
          style={{ paddingTop: HEADER_HEIGHT + 20 }}
        >
          <nav
            className="sticky flex w-[164px] shrink-0 flex-col gap-0.5 self-start"
            style={{ top: HEADER_HEIGHT + 20 }}
            aria-label="Settings sections"
          >
            {SECTIONS.map((s) => (
              <button
                key={s.id}
                type="button"
                aria-current={s.id === section ? "page" : undefined}
                onClick={() => openSettings(s.id)}
                className={cx(
                  "flex h-8 items-center gap-2 rounded-md px-2 text-left text-body",
                  s.id === section
                    ? "bg-fill font-medium text-fg"
                    : "text-fg-muted hover:bg-fill-subtle hover:text-fg",
                )}
              >
                <s.icon size={14} className="shrink-0" />
                {s.label}
              </button>
            ))}
          </nav>
          <div className="min-w-0 flex-1">
            <h2 className="pt-1 pb-4 font-semibold text-title2">{current.label}</h2>
            {section === "general" && <General />}
            {section === "harnesses" && <Harnesses />}
            {section === "connections" && <ConnectionsPage />}
            {section === "capabilities" && <CapabilitiesPage />}
            {section === "appearance" && <Appearance />}
            {section === "usage" && <UsagePage />}
            {section === "engine" && <Engine />}
          </div>
        </div>
      </div>
    </main>
  );
}
