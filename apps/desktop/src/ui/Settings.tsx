// Settings, inside the app's shell: the sidebar stays as it is and the main
// column shows a section list beside the page, so opening Settings remounts
// nothing and does not flash. After SettingsView.swift. Engine-backed pages
// (Harnesses, Connections, Capabilities, Appearance's themes) read and
// write through `_rustcode/*` requests.

import {
  Blocks,
  Check,
  FolderOpen,
  Settings as Gear,
  Info,
  KeyRound,
  Layers,
  Palette,
  Plus,
  TerminalSquare,
} from "lucide-react";
import { useState } from "react";
import { arr, bool, type Json, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { chooseBanner, removeBanner, setBannerStyle, useBanner } from "../lib/banner";
import { linkPreviewsEnabled, setLinkPreviews } from "../lib/links";
import { pickFolder, revealPath } from "../lib/native";
import { hasShell } from "../lib/shell";
import { BUILTIN_THEMES, cssName, type Theme, TOKENS } from "../lib/themes";
import { validateDefault } from "../state/harness";
import {
  APP_VERSION,
  createTheme,
  type NewThreadChoice,
  openSettings,
  restartEngine,
  type SettingsSection,
  setActiveTheme,
  setDefaultHarness,
  setOnboarding,
  setOrchestration,
  setProjectCwd,
  setSmartUi,
  setTheme,
  toast,
  useApp,
} from "../state/store";
import { CapabilitiesPage } from "./Capabilities";
import { DefaultsForm, HarnessList } from "./Harnesses";
import { MainHeader } from "./MainHeader";
import { NewThreadBanner } from "./NewThreadBanner";
import { cx, Kbd, Pill, Segments, Switch } from "./primitives";
import { Group, inputClass, Loading, Row, useEngineData } from "./settingsKit";

const SECTIONS: { id: SettingsSection; label: string; icon: typeof Gear }[] = [
  { id: "general", label: "General", icon: Gear },
  { id: "harnesses", label: "Harnesses", icon: Layers },
  { id: "connections", label: "Connections", icon: KeyRound },
  { id: "capabilities", label: "Capabilities", icon: Blocks },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "engine", label: "Engine", icon: TerminalSquare },
];

// ---------------------------------------------------------------- pages

function General() {
  const projectCwd = useApp((s) => s.projectCwd);
  const [linkPreviews, setLinkPreviewsState] = useState(linkPreviewsEnabled);
  const smartUi = useApp((s) => s.smartUi);
  const smartUiPending = useApp((s) => s.smartUiPending);
  const orchestration = useApp((s) => s.orchestration);
  const orchestrationPending = useApp((s) => s.orchestrationPending);
  const anyRunning = useApp((s) => Object.values(s.threads).some((t) => t.running));
  const shortcuts: [string, string][] = [
    ["New thread", "⌘N"],
    ["Command palette: threads, actions", "⌘K"],
    ["Settings", "⌘,"],
    ["Toggle sidebar", "⌘B"],
    ["Find in thread", "⌘F"],
    ["Terminal", "⌘J"],
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
      <Group title="Chat">
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
          title="Threads can start and message other threads"
          subtitle="An agent can list and read this project's threads, message one, start new threads (in their own worktrees, on any harness) and run one task on several harnesses to compare. You approve each first: Allow for This Thread, Allow Once or Don't Allow."
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
        <p className="mb-4 rounded-lg bg-warning/10 px-3.5 py-2.5 text-callout">
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
          <h3 className="mb-1.5 px-1 font-semibold text-callout text-fg-muted">Default for new threads</h3>
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
      toast(e instanceof Error ? e.message : String(e), "warning");
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
      </Group>
      <Group
        title="Theme"
        action={
          <div className="flex items-center gap-1">
            {hasShell() && (
              <button
                type="button"
                onClick={() => revealPath(userDir)}
                className="flex h-6 items-center gap-1 rounded-md px-2 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
              >
                <FolderOpen size={12} /> Open themes folder
              </button>
            )}
            <button
              type="button"
              onClick={() => setCreating(!creating)}
              aria-expanded={creating}
              className="flex h-6 items-center gap-1 rounded-md px-2 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
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
      toast(`Could not use that picture: ${e instanceof Error ? e.message : e}`, "warning"),
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
          <Info size={14} className="text-fg-faint" />
        </Row>
      </Group>
      <Group title="Engine log" footer="The engine's stderr; turns log to each thread's logs/debug.log.">
        <pre className="selectable m-0 max-h-[320px] overflow-auto p-3 font-mono text-caption leading-[15px] whitespace-pre-wrap text-fg-muted">
          {log.length ? log.slice(-200).join("\n") : "Nothing yet."}
        </pre>
      </Group>
    </>
  );
}

/** Settings in the main column: the section list beside the page, under the shared header. */
export function Settings() {
  const section = useApp((s) => s.settings) ?? "general";
  const current = SECTIONS.find((s) => s.id === section) ?? SECTIONS[0]!;
  return (
    <main className="relative flex min-h-0 min-w-0 flex-1 flex-col bg-bg" aria-label="Settings">
      <MainHeader title={<h1 className="min-w-0 truncate font-semibold text-body">Settings</h1>} />
      <div className="flex min-h-0 flex-1">
        <nav className="flex w-[196px] shrink-0 flex-col gap-0.5 px-2.5 pt-3" aria-label="Settings sections">
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
        <div className="scroll-stable min-w-0 flex-1">
          <div className="mx-auto max-w-[720px] px-[clamp(20px,5%,56px)] pb-16">
            <h2 className="pt-4 pb-4 font-semibold text-title2">{current.label}</h2>
            {section === "general" && <General />}
            {section === "harnesses" && <Harnesses />}
            {section === "connections" && <ConnectionsPage />}
            {section === "capabilities" && <CapabilitiesPage />}
            {section === "appearance" && <Appearance />}
            {section === "engine" && <Engine />}
          </div>
        </div>
      </div>
    </main>
  );
}
