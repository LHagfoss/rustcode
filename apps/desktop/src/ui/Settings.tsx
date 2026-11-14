import { setTabMovesFocus, tabMovesFocus } from "../lib/focusModality";
import { dirname } from "../lib/paths";
import { appPlatform, currentPlatform } from "../lib/platform";
import { ComputerUseGroup } from "./ComputerUseGroup";
// Settings, a full panel inside the window: the shell stays mounted behind it,
// hidden and inert, so closing Settings shows the thread exactly where it was.
// A grouped nav on the left searches every row's title and keywords
// (settingsIndex.ts) and shows one page at a time beside it; picking a result
// opens its page and lights the row. After SettingsView.swift.
// Server-backed pages (Harnesses, Connections, Capabilities, Appearance's
// themes, Usage, Statistics) read and write through `_rustcode/*` requests.
// Diagnostics (DiagnosticsPage.tsx) asks the shell about the server itself.

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
  Laptop,
  Layers,
  MessageSquare,
  Palette,
  Plug,
  Plus,
  RotateCcw,
  Search,
  Smartphone,
  Stethoscope,
  TerminalSquare,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { arr, bool, type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import { type PayloadCapture, parsePayloadCapture } from "../acp/rustcode";
import { Method } from "../acp/types";
import {
  BANNER_BLUR_DEFAULT,
  BANNER_BLUR_MAX,
  BANNER_DARKEN_DEFAULT,
  BANNER_DARKEN_MAX,
  BANNER_IN_THREADS_DEFAULT,
  BANNER_NAME_DEFAULT,
  BANNER_STYLE_DEFAULT,
  chooseBanner,
  darkenOverlay,
  removeBanner,
  setBannerBlur,
  setBannerDarken,
  setBannerInThreads,
  setBannerStyle,
  useBanner,
} from "../lib/banner";
import { CHAT_WIDTH_LABEL, type ChatWidth } from "../lib/chatWidth";
import { HAPTICS_DEFAULT, haptic, hapticsEnabled, setHaptics } from "../lib/haptics";
import { LINK_PREVIEWS_DEFAULT, linkPreviewsEnabled, setLinkPreviews } from "../lib/links";
import { openExternal, pickFolder, revealPath } from "../lib/native";
import { NEXT_PROMPT_DEFAULT, nextPromptEnabled, setNextPrompt } from "../lib/nextPrompt";
import { hasShell } from "../lib/shell";
import {
  commandIn,
  loadSystemSetup,
  runSystemSetup,
  type SetupStep,
  setupChip,
  systemSetupAvailable,
} from "../lib/systemSetup";
import { BUILTIN_THEMES, cssName, type Theme, TOKENS } from "../lib/themes";
import { canUpdate } from "../lib/updater";
import { AUTO_COMPACT_DEFAULT_THRESHOLD, AUTO_COMPACT_MODES } from "../state/autoCompact";
import { validateDefault } from "../state/harness";
import { runAllHarnessChecks, useHarnessChecks } from "../state/harnessChecks";
import {
  activeDelegates,
  busiestThreadDelegates,
  effectivePerThreadLimit,
  MAX_DELEGATES_PER_THREAD,
  MAX_DELEGATES_TOTAL,
  MIN_DELEGATES_PER_THREAD,
  MIN_DELEGATES_TOTAL,
  WAKE_BATCH_DEFAULT_S,
  WAKE_BATCH_MAX_S,
} from "../state/orchestration";
import { PERMISSION_MODES } from "../state/permissions";
import { RECAP_HOUR_OPTIONS } from "../state/recap";
import {
  useLoadScopeSettings,
  useScope,
  useScopedDefaultHarness,
  useScopedSetting,
} from "../state/settingsScope";
import {
  APP_VERSION,
  createTheme,
  errorText,
  type NewThreadChoice,
  openSettings,
  request,
  restartEngine,
  SETTINGS_DEFAULTS,
  type SettingsSection,
  setActiveTheme,
  setAnimateStreaming,
  setChatWidth,
  setDefaultCwd,
  setNotificationsBackgroundOnly,
  setOnboarding,
  setRecapHours,
  setRecapWhileAway,
  setReduceTransparency,
  setRoundedFont,
  setSmartUi,
  setSystemNotifications,
  setTheme,
  toast,
  useApp,
} from "../state/store";
import {
  checkIfStale,
  checkNow,
  installProgress,
  loadChannels,
  offersUpdate,
  requestUpdate,
  setAutoInstall,
  setUpdateChannel,
  showWhatsNew,
  useUpdates,
} from "../state/updates";
import { AddHarnessForm } from "./AddHarness";
import { CapabilitiesPage } from "./Capabilities";
import { DelegatePresetsGroup } from "./DelegatePresets";
import { DiagnosticsPage } from "./DiagnosticsPage";
import { DefaultsForm, HarnessList, useAutoDetect } from "./Harnesses";
import { HeaderButtonsSetting } from "./HeaderButtonsSetting";
import { LimitFallbackGroup } from "./LimitFallback";
import { MachineSettingsPage } from "./MachineSettingsPage";
import { MachinesPage } from "./MachinesPage";
import { NewThreadBanner } from "./NewThreadBanner";
import { OutsideAgentsPage } from "./OutsideAgentsPage";
import { ChromeButton, cx, Kbd, noTextHelpers, Spinner, Switch } from "./primitives";
import { RemoteAccessPage } from "./RemoteAccessPage";
import { ScopedHarnessList } from "./ScopedHarnesses";
import { ScopedValue, SettingsScopeBar } from "./SettingsScope";
import { ShortcutsPage } from "./ShortcutSettings";
import { StatisticsPage } from "./StatisticsPage";
import { type SettingEntry, searchSettings } from "./settingsIndex";
import {
  Action,
  Chip,
  Choice,
  fieldClass,
  Group,
  Loading,
  Row,
  RowResets,
  SectionTitle,
  Slider,
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
      { id: "agents", label: "Harnesses", icon: Layers },
      { id: "connections", label: "Connections", icon: KeyRound },
      { id: "capabilities", label: "Capabilities", icon: Blocks },
      { id: "remote", label: "Remote access", icon: Smartphone },
      { id: "outside", label: "Outside agents", icon: Plug },
      { id: "machines", label: "Environments", icon: Laptop },
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
      { id: "diagnostics", label: "Diagnostics", icon: Stethoscope },
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
/** A status read on a Settings page ends in an error row within this, never a spinner that stays. */
const STATUS_READ_MS = 8_000;

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
      show(await request("_rustcode/presence", params, STATUS_READ_MS));
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
          ? "Shares the project folder's name and whether a turn runs."
          : "Shares the project folder's name and whether a turn runs. Discord is not running.";
  // Off is the default: presence shares what you work on, so turning it on is the user's call.
  const reset = state?.enabled ? () => setEnabled(false) : undefined;
  return (
    <Row title="Discord Rich Presence" subtitle={subtitle} reset={reset}>
      {state && <Switch label="Discord Rich Presence" checked={state.enabled} onChange={setEnabled} />}
    </Row>
  );
}

const permissionLabel = (value: Json | undefined) =>
  PERMISSION_MODES.find((mode) => mode.id === value)?.label ?? String(value ?? "Ask");
const boolLabel = (value: Json | undefined) => (value ? "On" : "Off");
const compactLabel = (value: Json | undefined) =>
  AUTO_COMPACT_MODES.find((mode) => mode.value === value)?.label ?? String(value ?? "Off");
const thresholdLabel = (value: Json | undefined) =>
  typeof value === "number" ? `${Math.round(value / 1_000)}k tokens` : "150k tokens";

function GeneralPage() {
  const defaultCwd = useApp((s) => s.defaultCwd);
  useLoadScopeSettings();
  const defaultAccess = useScopedSetting("default-permission-mode", permissionLabel);
  const resume = useScopedSetting("resume-after-restart", boolLabel);
  const afterLimit = useScopedSetting("continue-after-limit", boolLabel);
  const autoCompactMode = useScopedSetting("auto-compact-mode", compactLabel);
  const autoCompactThreshold = useScopedSetting("auto-compact-threshold", thresholdLabel);
  const defaultAccessValue = String(defaultAccess.value ?? SETTINGS_DEFAULTS.defaultPermissionMode);
  const resumeValue = Boolean(resume.value ?? SETTINGS_DEFAULTS.resumeAfterRestart);
  const afterLimitValue = Boolean(afterLimit.value ?? SETTINGS_DEFAULTS.continueAfterLimit);
  const autoCompactValue = String(autoCompactMode.value ?? SETTINGS_DEFAULTS.autoCompact.mode);
  const autoCompactThresholdValue = Number(autoCompactThreshold.value ?? AUTO_COMPACT_DEFAULT_THRESHOLD);
  const notifications = useApp((s) => s.systemNotifications);
  const backgroundOnly = useApp((s) => s.notificationsBackgroundOnly);
  const recap = useApp((s) => s.recapWhileAway);
  const recapHours = useApp((s) => s.recapHours);
  const [tabFocus, setTabFocus] = useState(tabMovesFocus);
  const [haptics, setHapticsState] = useState(hapticsEnabled);
  return (
    <>
      <SettingsScopeBar />
      <Group title="New threads">
        <Row
          title="Default project folder"
          subtitle="Where a new thread starts, unless you pick a folder for it."
          hint="Used in place of the open thread's folder. A folder picked for one new thread still wins."
          status={defaultCwd || "None chosen"}
        >
          <Action onClick={() => void pickFolder(defaultCwd).then((p) => p && setDefaultCwd(p))}>
            Choose…
          </Action>
        </Row>
        <Row title="Harness, model and effort" subtitle="Which harness runs a new thread by default.">
          <Action onClick={() => openSettings("agents")}>Harnesses…</Action>
        </Row>
        <Row
          title="Default access"
          reset={
            defaultAccess.mixed || defaultAccessValue !== SETTINGS_DEFAULTS.defaultPermissionMode
              ? () => void defaultAccess.set(SETTINGS_DEFAULTS.defaultPermissionMode)
              : undefined
          }
          subtitle="What a new thread may do without asking."
          hint="The shield under the message box changes one thread only."
        >
          <ScopedValue mixed={defaultAccess.mixed} breakdown={defaultAccess.breakdown}>
            <Choice
              label="Default access"
              value={defaultAccessValue}
              options={PERMISSION_MODES.map((m) => ({ value: m.id, label: m.label }))}
              onChange={(value) => void defaultAccess.set(value)}
            />
          </ScopedValue>
        </Row>
      </Group>
      <Group title="Threads">
        <Row
          title="Continue threads after a restart"
          reset={
            resume.mixed || !resumeValue
              ? () => void resume.set(SETTINGS_DEFAULTS.resumeAfterRestart)
              : undefined
          }
          subtitle="Pick up a turn that quitting or an update cut short."
          hint="The thread is sent “Continue where you left off” when the app is back."
        >
          <ScopedValue mixed={resume.mixed} breakdown={resume.breakdown}>
            <Switch
              label="Continue threads after a restart"
              checked={resumeValue}
              onChange={(value) => void resume.set(value)}
            />
          </ScopedValue>
        </Row>
        <Row
          title="Continue threads on their own"
          reset={
            afterLimit.mixed || !afterLimitValue
              ? () => void afterLimit.set(SETTINGS_DEFAULTS.continueAfterLimit)
              : undefined
          }
          subtitle="Carry on after a usage limit, an overloaded provider or a dropped network."
          hint="Once it can go on again. Not if you moved the thread to another harness or account, or sent it something else."
        >
          <ScopedValue mixed={afterLimit.mixed} breakdown={afterLimit.breakdown}>
            <Switch
              label="Continue threads on their own"
              checked={afterLimitValue}
              onChange={(value) => void afterLimit.set(value)}
            />
          </ScopedValue>
        </Row>
        <Row
          title="Auto-compact"
          stacked
          reset={
            autoCompactMode.mixed || autoCompactValue !== SETTINGS_DEFAULTS.autoCompact.mode
              ? () => void autoCompactMode.set(SETTINGS_DEFAULTS.autoCompact.mode)
              : undefined
          }
          subtitle="Compact a thread's context when a turn ends above the threshold."
          hint="The second mode also compacts shortly before Claude Code's or Codex's prompt cache expires. Never while a turn runs, a message waits, an approval is open or the last answer is unread. Turn it off for one thread in its menu."
        >
          <ScopedValue mixed={autoCompactMode.mixed} breakdown={autoCompactMode.breakdown}>
            <Choice
              label="Auto-compact"
              value={autoCompactValue}
              options={AUTO_COMPACT_MODES}
              onChange={(value) => void autoCompactMode.set(value)}
            />
          </ScopedValue>
        </Row>
        {autoCompactValue !== "off" && (
          <Row
            title="Auto-compact threshold"
            reset={
              autoCompactThreshold.mixed || autoCompactThresholdValue !== AUTO_COMPACT_DEFAULT_THRESHOLD
                ? () => void autoCompactThreshold.set(AUTO_COMPACT_DEFAULT_THRESHOLD)
                : undefined
            }
            subtitle="Compact once the context passes this many tokens."
            hint="The second mode's idle, pre-cache rule uses a smaller 60k threshold."
          >
            <ScopedValue mixed={autoCompactThreshold.mixed} breakdown={autoCompactThreshold.breakdown}>
              <Slider
                label="Auto-compact threshold"
                value={Math.round(autoCompactThresholdValue / 1_000)}
                min={20}
                max={500}
                step={10}
                onCommit={(value) => void autoCompactThreshold.set(value * 1_000)}
                unit="k tokens"
              />
            </ScopedValue>
          </Row>
        )}
      </Group>
      <Group title="Notifications">
        <Row
          title="System notifications"
          reset={
            notifications ? undefined : () => setSystemNotifications(SETTINGS_DEFAULTS.systemNotifications)
          }
          subtitle="When a thread needs approval, asks a question, hits a limit or finishes."
          hint="The app asks the system for permission the first time one is due."
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
          subtitle="With the window in front, a toast says it instead."
          hint="The thread's row keeps it. A thread on screen is never announced either way."
        >
          <Switch
            label="Only when RustCode is in the background"
            checked={backgroundOnly}
            onChange={setNotificationsBackgroundOnly}
          />
        </Row>
        <Row
          title="While-you-were-away recap"
          reset={recap ? undefined : () => setRecapWhileAway(SETTINGS_DEFAULTS.recapWhileAway)}
          subtitle="A card after the app sat idle: what needs you, what failed, what finished."
          hint="It shows at the top of the chat and the inbox. Dismissed, the inbox keeps a row that brings it back."
        >
          <Switch label="While-you-were-away recap" checked={recap} onChange={setRecapWhileAway} />
        </Row>
        <Row
          title="Away for"
          reset={
            recapHours !== SETTINGS_DEFAULTS.recapHours
              ? () => setRecapHours(SETTINGS_DEFAULTS.recapHours)
              : undefined
          }
          subtitle="How long the app sits idle before the recap."
        >
          <Choice
            label="Away before the recap"
            value={String(recapHours)}
            options={RECAP_HOUR_OPTIONS.map((h) => ({ value: String(h), label: `${h}h` }))}
            onChange={(value) => setRecapHours(Number(value))}
          />
        </Row>
      </Group>
      <Group title="This computer">
        <PresenceRow />
        <Row title="Tab moves focus between controls" subtitle="Tab and Shift+Tab move between controls.">
          <Switch
            label="Tab moves focus between controls"
            checked={tabFocus}
            onChange={(value) => {
              setTabMovesFocus(value);
              setTabFocus(value);
            }}
          />
        </Row>
        {currentPlatform() === "mac" && (
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
            subtitle="A light tap when you send or stop a turn."
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
        )}
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
  const [nextPrompt, setNextPromptState] = useState(nextPromptEnabled);
  return (
    <Group title="In the chat">
      <Row
        title="Chat width"
        reset={
          chatWidth !== SETTINGS_DEFAULTS.chatWidth
            ? () => setChatWidth(SETTINGS_DEFAULTS.chatWidth)
            : undefined
        }
        subtitle="How wide the transcript and composer grow on a large window."
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
        subtitle="Let replies include a small interactive view, such as a table or chart."
        hint="It runs in the chat, sandboxed, with no network. Its buttons send a message only when you click them."
      >
        <Switch label="Smart UI" checked={smartUi} onChange={setSmartUi} />
      </Row>
      {smartUiPending && (
        <Row title="Restart the server to apply" subtitle="The server reads this setting when it starts.">
          <Action prominent disabled={anyRunning} onClick={() => void restartEngine()}>
            Restart Server
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
        subtitle="Show links as chips with the page's title and icon."
        hint="The app fetches them itself, with no cookies, from public hosts only."
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
        title="Suggested next prompt"
        reset={
          nextPrompt === NEXT_PROMPT_DEFAULT
            ? undefined
            : () => {
                setNextPrompt(NEXT_PROMPT_DEFAULT);
                setNextPromptState(NEXT_PROMPT_DEFAULT);
              }
        }
        subtitle="The empty composer suggests an answer to what a reply offers."
        hint="Tab puts it in the field to edit, Esc dismisses it. It is never sent on its own, and needs no extra model requests."
      >
        <Switch
          label="Suggested next prompt"
          checked={nextPrompt}
          onChange={(v) => {
            setNextPrompt(v);
            setNextPromptState(v);
          }}
        />
      </Row>
      <HeaderButtonsSetting />
    </Group>
  );
}

function AgentsPage() {
  useLoadScopeSettings();
  const { scope, targets, writes } = useScope();
  const scoped = useScopedDefaultHarness();
  const snapshot = scoped.snapshot;
  const thisMachine = scope.mode === "this";
  const [saved, setSaved] = useState(false);
  const [adding, setAdding] = useState(false);
  const checking = useHarnessChecks((s) => Object.values(s.byHarness).some((e) => "running" in e));
  const checkingAll = useHarnessChecks((s) => s.all);
  // Installed, signed in, versions: detected again each time the page opens and the window is back.
  useAutoDetect();
  // An unsaved choice stays here while it is not valid yet (a harness that still lists models).
  const [choice, setChoice] = useState<NewThreadChoice | null>(null);
  if (!snapshot) return <Loading error={scoped.error} onRetry={scoped.reload} />;
  // The default saves as it changes: there is nothing to confirm, and the choice is easy to undo.
  const change = (next: NewThreadChoice) => {
    setSaved(false);
    setChoice(next);
    if (validateDefault(snapshot, next)) return;
    void scoped.set(next).then(
      () => {
        setChoice(null);
        setSaved(true);
      },
      () => undefined,
    );
  };
  const value = choice ?? scoped.value ?? snapshot.default;
  return (
    <>
      <SettingsScopeBar />
      {snapshot.unsupported && (
        <p className="mb-4 rounded-lg bg-warning/10 px-3 py-2 text-callout">
          This server does not run other harnesses yet. Threads use the built-in harness; update RustCode to
          add more.
        </p>
      )}
      {!snapshot.unsupported && (
        <Group
          title="New threads start on"
          action={
            saved ? (
              <Chip tone="ok">
                <Check size={11} /> Saved
              </Chip>
            ) : undefined
          }
          footer="A thread can still switch harness and model from the composer."
        >
          {scoped.mixed && (
            <div className="px-4 pb-2">
              <ScopedValue mixed breakdown={scoped.breakdown}>
                <span className="text-callout text-fg-muted">The connected machines disagree</span>
              </ScopedValue>
            </div>
          )}
          <DefaultsForm snapshot={snapshot} value={value} onChange={change} />
        </Group>
      )}
      <Group
        title="Harnesses"
        action={
          thisMachine ? (
            <div className="flex items-center gap-1">
              <Action quiet size="sm" onClick={() => void runAllHarnessChecks()} disabled={checking}>
                {checking && <Spinner size={11} />}
                {checkingAll
                  ? `Checking ${Math.min(checkingAll.done + 1, checkingAll.total)} of ${checkingAll.total}…`
                  : "Check all"}
              </Action>
              <Action size="sm" onClick={() => setAdding(true)} disabled={adding}>
                <Plus size={11} />
                Add harness
              </Action>
            </div>
          ) : undefined
        }
        footer={
          thisMachine
            ? "Each harness signs in on its own. Check confirms it runs and lists its models."
            : "A switch here turns a harness on or off on every connected machine."
        }
      >
        {thisMachine && adding && <AddHarnessForm onClose={() => setAdding(false)} />}
        {thisMachine ? (
          <HarnessList snapshot={snapshot} />
        ) : (
          <ScopedHarnessList targets={targets} writes={writes} />
        )}
      </Group>
      {thisMachine && <ComputerUseGroup />}
      <OrchestrationGroup />
      <DelegatePresetsGroup />
      <LimitFallbackGroup />
    </>
  );
}

/** Threads that start and message other threads (`state/host.ts`, docs/orchestrator.md). */
function OrchestrationGroup() {
  const orchestration = useScopedSetting("orchestration", boolLabel);
  const threadLimitSetting = useScopedSetting("delegates-per-thread", (value) =>
    typeof value === "number" ? String(value) : "4",
  );
  const totalLimitSetting = useScopedSetting("delegates-total", (value) =>
    typeof value === "number" ? String(value) : "8",
  );
  const wakeBatchSetting = useScopedSetting("wake-batch-seconds", (value) =>
    typeof value === "number" ? (value === 0 ? "Off" : `${value} s`) : "60 s",
  );
  const wakeBatch = Number(wakeBatchSetting.value ?? WAKE_BATCH_DEFAULT_S);
  const readAcrossProjects = useScopedSetting("read-across-projects", boolLabel);
  const orchestrationValue = Boolean(orchestration.value ?? true);
  const readAcrossValue = Boolean(readAcrossProjects.value ?? true);
  const threadLimit = Number(threadLimitSetting.value ?? SETTINGS_DEFAULTS.threadLimit);
  const totalLimit = Number(totalLimitSetting.value ?? SETTINGS_DEFAULTS.totalLimit);
  const pending = useApp((s) => s.orchestrationPending);
  const anyRunning = useApp((s) => Object.values(s.threads).some((t) => t.running));
  const links = useApp((s) => s.links);
  const running = useMemo(() => activeDelegates(links), [links]);
  const busiest = useMemo(() => busiestThreadDelegates(links), [links]);
  const possible = effectivePerThreadLimit({ perThread: threadLimit, total: totalLimit });
  return (
    <>
      <Group title="Orchestration">
        <Row
          title="Threads can start and message other threads"
          reset={
            orchestration.mixed || !orchestrationValue
              ? () => void orchestration.set(SETTINGS_DEFAULTS.orchestration)
              : undefined
          }
          subtitle="An agent can read, message and start threads, and compare a task across harnesses."
          hint="Full access does this without asking; Auto-edit may list, read and wait; otherwise you approve each. A thread it starts gets the lower of this thread's access and Default access."
        >
          <ScopedValue mixed={orchestration.mixed} breakdown={orchestration.breakdown}>
            <Switch
              label="Threads can start and message other threads"
              checked={orchestrationValue}
              onChange={(value) => void orchestration.set(value)}
            />
          </ScopedValue>
        </Row>
        <Row
          title="Let agents read threads in other projects"
          reset={
            readAcrossProjects.mixed || !readAcrossValue
              ? () => void readAcrossProjects.set(SETTINGS_DEFAULTS.readAcrossProjects)
              : undefined
          }
          subtitle="An agent can list, search and read any thread, in any project."
          hint="Reading still follows the approval rules above. Messaging and starting threads stay in the project."
        >
          <ScopedValue mixed={readAcrossProjects.mixed} breakdown={readAcrossProjects.breakdown}>
            <Switch
              label="Let agents read threads in other projects"
              checked={readAcrossValue}
              onChange={(value) => void readAcrossProjects.set(value)}
            />
          </ScopedValue>
        </Row>
        {pending && (
          <Row title="Restart the server to apply" subtitle="The server reads this setting when it starts.">
            <Action prominent disabled={anyRunning} onClick={() => void restartEngine()}>
              Restart Server
            </Action>
          </Row>
        )}
      </Group>
      <Group
        title="Delegation"
        disabled={!orchestrationValue}
        footer={
          orchestrationValue
            ? "Only live delegates take a slot. A follow-up to a thread this one already runs takes none."
            : "These limits have no effect while “Threads can start and message other threads” is off."
        }
      >
        <Row
          title="Delegates across all threads"
          reset={
            totalLimitSetting.mixed || totalLimit !== SETTINGS_DEFAULTS.totalLimit
              ? () => void totalLimitSetting.set(SETTINGS_DEFAULTS.totalLimit)
              : undefined
          }
          subtitle="How many delegates may run at once, whichever thread started them."
          status={
            running === 0
              ? "No delegates running now"
              : running === 1
                ? "1 running now"
                : `${running} running now`
          }
        >
          <ScopedValue mixed={totalLimitSetting.mixed} breakdown={totalLimitSetting.breakdown}>
            <Slider
              label="Delegates across all threads"
              value={totalLimit}
              min={MIN_DELEGATES_TOTAL}
              max={MAX_DELEGATES_TOTAL}
              onCommit={(value) => void totalLimitSetting.set(value)}
              unit="delegates"
            />
          </ScopedValue>
        </Row>
        <Row
          title="Delegates one thread can run at once"
          reset={
            threadLimitSetting.mixed || threadLimit !== SETTINGS_DEFAULTS.threadLimit
              ? () => void threadLimitSetting.set(SETTINGS_DEFAULTS.threadLimit)
              : undefined
          }
          subtitle="How many delegates one thread may have working at once."
          hint="The total above can make this lower."
          status={
            threadLimit > totalLimit
              ? `${threadLimit} set, ${possible} possible while the total is ${totalLimit}`
              : busiest === 0
                ? "No delegates on any thread now"
                : `The busiest thread has ${busiest} now`
          }
        >
          <ScopedValue mixed={threadLimitSetting.mixed} breakdown={threadLimitSetting.breakdown}>
            <Slider
              label="Delegates one thread can run at once"
              value={threadLimit}
              min={MIN_DELEGATES_PER_THREAD}
              max={MAX_DELEGATES_PER_THREAD}
              onCommit={(value) => void threadLimitSetting.set(value)}
              unit="delegates"
            />
          </ScopedValue>
        </Row>
        <Row
          title="Merge delegate wakes within"
          reset={
            wakeBatchSetting.mixed || wakeBatch !== WAKE_BATCH_DEFAULT_S
              ? () => void wakeBatchSetting.set(WAKE_BATCH_DEFAULT_S)
              : undefined
          }
          subtitle="Delegates that finish or stall inside this window reach the thread in one turn."
          hint="Each wake is a model turn that resends the thread's whole context. A question for you is never held back. Off sends each as it comes."
        >
          <ScopedValue mixed={wakeBatchSetting.mixed} breakdown={wakeBatchSetting.breakdown}>
            <Slider
              label="Merge delegate wakes within"
              value={wakeBatch}
              min={0}
              max={WAKE_BATCH_MAX_S}
              step={15}
              onCommit={(value) => void wakeBatchSetting.set(value)}
              unit="seconds"
            />
          </ScopedValue>
        </Row>
      </Group>
    </>
  );
}

interface Connections {
  github?: {
    cliInstalled: boolean;
    installHint?: string;
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
          installHint: str(gh.installHint),
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
  const { scope, machines } = useScope();
  const remote =
    scope.mode === "machine"
      ? machines.find((machine) => machine.host === scope.host && !machine.local)
      : undefined;
  if (remote)
    return (
      <>
        <SettingsScopeBar machineOnly />
        <Group title="Credentials and accounts">
          <Row
            title={`Sign in on ${remote.name} itself`}
            subtitle="Sign in on that computer. Credentials never leave their machine."
          />
        </Group>
      </>
    );
  if (!data)
    return (
      <>
        <SettingsScopeBar machineOnly />
        <Loading error={error} />
      </>
    );
  return (
    <>
      <SettingsScopeBar machineOnly />
      <Group title="GitHub" footer="Pull requests use the GitHub CLI's sign-in.">
        {!data.github?.cliInstalled ? (
          <Row
            title="GitHub CLI not installed"
            subtitle={
              data.github?.installHint
                ? `Install it with: ${data.github.installHint}`
                : "Install GitHub CLI: https://cli.github.com"
            }
          />
        ) : data.github.accounts.length ? (
          data.github.accounts.map((a) => (
            <Row key={`${a.host}/${a.login}`} title={a.login} subtitle={a.host}>
              {a.valid ? (
                <Chip tone="ok">{a.active ? "Signed in" : "Usable"}</Chip>
              ) : (
                <Chip tone="warn" title="Run: gh auth login">
                  Token invalid
                </Chip>
              )}
            </Row>
          ))
        ) : (
          <Row title="Not signed in" subtitle="Run: gh auth login" />
        )}
      </Group>
      <Group title="Model accounts" footer="Sign in or out with `/login` in a thread.">
        {data.accounts.length ? (
          data.accounts.map((a, i) => (
            <Row key={i} title={a.display || a.provider} subtitle={`${a.provider} · ${a.method}`}>
              {a.needsSignIn ? (
                <Chip tone="warn">Sign in again</Chip>
              ) : a.active ? (
                <Chip tone="ok">Active</Chip>
              ) : (
                <Chip>Idle</Chip>
              )}
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
    <div className="rc-expand rule-t flex flex-col gap-2 px-4 py-3">
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
          aria-invalid={name.length > 0 && !valid}
          {...noTextHelpers}
          className={cx(fieldClass, "flex-1 font-mono")}
        />
        <Action onClick={onDone}>Cancel</Action>
        <Action prominent disabled={!valid} onClick={() => void create()}>
          Create
        </Action>
      </div>
      {name.length > 0 && !valid ? (
        <p className="m-0 text-caption text-warning">
          A theme name is lowercase letters, digits and dashes, up to 64, starting with a letter or digit.
        </p>
      ) : cli ? (
        <p className="m-0 text-caption text-fg-muted">
          This server cannot create themes from the app. In a terminal:{" "}
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
  const reduceTransparency = useApp((s) => s.reduceTransparency);
  const active = useApp((s) => s.themeActive);
  const snapshot = useApp((s) => s.themes);
  const supported = useApp((s) => s.features.themes);
  const [creating, setCreating] = useState(false);
  const themes = snapshot?.themes.length ? snapshot.themes : BUILTIN_THEMES;
  const userPath = themes.find((t) => t.source === "user" && t.path)?.path;
  const userDir = userPath
    ? dirname(userPath)
    : appPlatform() === "windows"
      ? "%APPDATA%\\rustcode\\themes"
      : "~/.config/rustcode/themes";
  return (
    <>
      <Group title="Mode">
        <Row
          title="Light or dark"
          reset={mode !== SETTINGS_DEFAULTS.theme ? () => setTheme(SETTINGS_DEFAULTS.theme) : undefined}
          subtitle="System follows your operating system."
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
          subtitle="New words fade in as the model writes them."
        >
          <Switch label="Animate streaming text" checked={animateStreaming} onChange={setAnimateStreaming} />
        </Row>
        <Row
          title="Reduce transparency"
          reset={
            reduceTransparency !== SETTINGS_DEFAULTS.reduceTransparency
              ? () => setReduceTransparency(SETTINGS_DEFAULTS.reduceTransparency)
              : undefined
          }
          subtitle="Menus, dialogs and the sidebar turn solid instead of frosted."
          hint="Also on when your system asks for it."
        >
          <Switch label="Reduce transparency" checked={reduceTransparency} onChange={setReduceTransparency} />
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
            ? "This server has no theme files yet. Update RustCode to add your own."
            : `Theme files live in ${userDir}; edits apply live. See docs/themes.md.`
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
    <section className="mb-6">
      <SectionTitle
        title="Tokens"
        action={
          <Action quiet size="sm" onClick={() => setOpen(!open)} aria-expanded={open}>
            {open ? "Hide" : "Show"}
          </Action>
        }
      />
      {open && (
        <div className="rc-expand grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-2 rounded-xl bg-surface p-3">
          {TOKENS.map((t) => (
            <div key={t} className="flex items-center gap-2 rounded-lg bg-surface-2 px-2 py-1.5">
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
    setBannerDarken(BANNER_DARKEN_DEFAULT);
    setBannerBlur(BANNER_BLUR_DEFAULT);
    setBannerInThreads(BANNER_IN_THREADS_DEFAULT);
  };
  return (
    <div>
      <Row
        title="New thread banner"
        reset={banner.name !== BANNER_NAME_DEFAULT ? reset : undefined}
        subtitle={
          banner.name
            ? "Behind the top of new threads."
            : "A wide picture over new threads, drawn in text or as is."
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
        <>
          <Row
            title="Darken"
            reset={
              banner.darken !== BANNER_DARKEN_DEFAULT
                ? () => setBannerDarken(BANNER_DARKEN_DEFAULT)
                : undefined
            }
            subtitle="Black over the picture, so the text reads"
          >
            <Slider
              label="Darken the banner"
              value={banner.darken}
              min={0}
              max={BANNER_DARKEN_MAX}
              step={5}
              unit="%"
              onChange={setBannerDarken}
            />
          </Row>
          <Row
            title="Blur"
            reset={banner.blur !== BANNER_BLUR_DEFAULT ? () => setBannerBlur(BANNER_BLUR_DEFAULT) : undefined}
            subtitle="Softens the picture"
          >
            <Slider
              label="Blur the banner"
              value={banner.blur}
              min={0}
              max={BANNER_BLUR_MAX}
              step={2}
              unit="px"
              onChange={setBannerBlur}
            />
          </Row>
          <Row
            title="Show in threads"
            reset={
              banner.inThreads !== BANNER_IN_THREADS_DEFAULT
                ? () => setBannerInThreads(BANNER_IN_THREADS_DEFAULT)
                : undefined
            }
            subtitle="Behind open threads too, with the same darkening and blur"
          >
            <Switch
              label="Show the banner in threads"
              checked={banner.inThreads}
              onChange={setBannerInThreads}
            />
          </Row>
          {/* As a new thread shows it, at a size that reads. */}
          <div className="px-4 pb-4">
            <div className="relative overflow-hidden rounded-lg bg-bg">
              <NewThreadBanner
                key={banner.style}
                name={banner.name}
                style={banner.style}
                height={240}
                blur={banner.blur}
              />
              {banner.darken > 0 && (
                <div
                  aria-hidden
                  className="pointer-events-none absolute inset-0"
                  style={{ background: darkenOverlay(banner.darken) ?? undefined }}
                />
              )}
            </div>
          </div>
        </>
      )}
    </div>
  );
}

/**
 * The app's version and its update, like the SwiftUI app's UpdateControl:
 * Check for Updates, then Update to X (installs in place on macOS, a Linux
 * AppImage or .deb and Windows, quits and reopens) or Download X (the release
 * page, for a copy that cannot replace itself).
 */
function AppVersion() {
  const info = useUpdates((s) => s.info);
  const channel = useUpdates((s) => s.channel);
  const installing = useUpdates((s) => s.installing);
  const progress = useUpdates((s) => s.progress);
  const background = useUpdates((s) => s.error);
  const waiting = useUpdates((s) => s.waiting);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const shellVersion = useUpdates((s) => s.version);
  const version = info?.current ?? shellVersion ?? APP_VERSION;
  const offered = info !== null && offersUpdate(info, channel);
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
    if (!offered || !info?.canInstall) {
      if (info?.page) openExternal(info.page);
      return;
    }
    setError(null);
    // One install path: the sidebar row shows the same progress when it is visible.
    const failed = await requestUpdate();
    if (failed) setError(failed);
  };
  const { fraction, step } = installProgress(progress);
  const failure = error ?? (info ? null : background);
  const subtitle = failure
    ? failure
    : installing
      ? step
      : waiting
        ? "Waiting for threads to finish, then reopens."
        : info && !info.available
          ? "RustCode is up to date."
          : info?.development
            ? "A development build; it does not update itself."
            : offered
              ? (info?.installNote ?? undefined)
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
      {error && offered && info?.canInstall && info.page && (
        <Action onClick={() => openExternal(info.page ?? "")}>Release page</Action>
      )}
      {!canUpdate() ? (
        <Info size={14} className="text-fg-faint" />
      ) : offered && info.latest ? (
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

/**
 * Payload capture (`docs/data-store.md` › Captured payloads): the engine's own
 * `capture_provider_payloads` and `capture_harness_rpc_payloads`, which are
 * global and off by default, so nobody has to hand-edit `config.toml`. The
 * engine reads both from the config it started with, so a change offers the
 * restart right below it.
 */
function RecordRequestBodiesGroup() {
  const status = useApp((s) => s.status);
  const anyRunning = useApp((s) => Object.values(s.threads).some((t) => t.running));
  const [capture, setCapture] = useState<PayloadCapture | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const load = useCallback(async () => {
    try {
      setCapture(parsePayloadCapture(await request(Method.telemetry, {}, STATUS_READ_MS)));
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, []);
  useEffect(() => {
    if (status === "ready") void load();
  }, [status, load]);
  // Both keys are saved together, so a switch turns one on and leaves the other as it was.
  const save = (next: PayloadCapture) => {
    void request(Method.telemetrySet, { ...next }).then(
      (value) => {
        setCapture(parsePayloadCapture(value));
        setPending(true);
        setError(null);
      },
      (e) => setError(errorText(e)),
    );
  };
  if (!capture) return <Loading error={error} />;
  return (
    <Group
      title="Record request bodies"
      footer="Redacted, clipped and dropped after 7 days. Off, only timing and status are kept."
    >
      <Row
        title="Provider requests (built-in harness)"
        reset={capture.providerPayloads ? () => save({ ...capture, providerPayloads: false }) : undefined}
        subtitle="Save each request the built-in harness sends a provider."
        hint="With a summary of the response, so a slow or failed turn can be read back. Agents query it with query_data (docs/data-store.md)."
      >
        <Switch
          label="Provider requests (built-in harness)"
          checked={capture.providerPayloads}
          onChange={(providerPayloads) => save({ ...capture, providerPayloads })}
        />
      </Row>
      <Row
        title="Harness messages"
        reset={capture.harnessRpcPayloads ? () => save({ ...capture, harnessRpcPayloads: false }) : undefined}
        subtitle="Save each message exchanged with a harness."
        hint="ACP or Codex JSON-RPC, so a failed handshake or tool call can be read back."
      >
        <Switch
          label="Harness messages"
          checked={capture.harnessRpcPayloads}
          onChange={(harnessRpcPayloads) => save({ ...capture, harnessRpcPayloads })}
        />
      </Row>
      {error && <Row title="Could not save" subtitle={error} />}
      {pending && (
        <Row title="Restart the server to apply" subtitle="The server reads these when it starts.">
          <Action
            prominent
            disabled={anyRunning}
            onClick={() => {
              setPending(false);
              void restartEngine();
            }}
          >
            Restart Server
          </Action>
        </Row>
      )}
    </Group>
  );
}

function AdvancedPage() {
  return (
    <>
      <RecordRequestBodiesGroup />
      <Group title="Server status and logs" footer="The server's process, log and a debug report.">
        <Row title="Diagnostics" subtitle="Moved to its own page.">
          <Action onClick={() => openSettings("diagnostics")}>Open Diagnostics</Action>
        </Row>
      </Group>
      <Group title="Setup">
        <Row
          title="Set up your agents again"
          subtitle="Find each harness, sign it in and check that it runs."
        >
          <Action onClick={() => setOnboarding(true)}>Run setup again</Action>
        </Row>
      </Group>
    </>
  );
}

function AboutPage() {
  return (
    <>
      <Group title="Version" footer="RustCode is open source; its licences ship in each release.">
        <AppVersion />
        <UpdateChannelRow />
        <AutoInstallRow />
      </Group>
      <SystemSetupGroup />
    </>
  );
}

/**
 * Settings › About › System setup: the per-OS steps (PATH, quarantine, launcher
 * entry, the background server), each with what it found. The automatic ones
 * ran after the install or the last update; Run again checks a step and fixes
 * only what is missing. A step that needs an administrator or a shell-profile
 * edit shows its command to copy instead of running it.
 */
function SystemSetupGroup() {
  const [steps, setSteps] = useState<SetupStep[] | null>(null);
  const [running, setRunning] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!systemSetupAvailable()) return;
    loadSystemSetup().then(setSteps, (e) => setError(errorText(e)));
  }, []);
  if (!systemSetupAvailable()) return null;
  const run = async (step?: string) => {
    setRunning(step ?? "all");
    setError(null);
    try {
      setSteps(await runSystemSetup(step));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setRunning(null);
    }
  };
  return (
    <Group
      title="System setup"
      footer={
        error ?? "Runs after an install or an update. RustCode never asks for an administrator silently."
      }
      action={
        <Action disabled={running !== null} onClick={() => void run()}>
          {running === "all" ? <Spinner size={12} /> : null}
          Run again
        </Action>
      }
    >
      {steps === null ? (
        <Row title="Checking…" subtitle={error ?? undefined} />
      ) : (
        steps.map((step) => {
          const chip = setupChip(step.status);
          const command = step.status === "needs-you" ? commandIn(step.message) : null;
          return (
            <Row
              key={step.id}
              title={step.title}
              subtitle={step.about}
              status={step.message}
              badge={<Chip tone={chip.tone}>{chip.label}</Chip>}
            >
              {command && (
                <Action
                  onClick={() =>
                    void navigator.clipboard.writeText(command).then(() => toast("Command copied"))
                  }
                >
                  Copy command
                </Action>
              )}
              <Action disabled={running !== null} onClick={() => void run(step.id)}>
                {running === step.id ? <Spinner size={12} /> : null}
                {step.auto ? "Run again" : step.status === "done" ? "Check again" : "Set up"}
              </Action>
            </Row>
          );
        })
      )}
    </Group>
  );
}

/**
 * Settings › About › Update channel: Stable follows releases only; Beta also
 * offers the betas of the next version, which show in the same row and install
 * the same way. Betas build the macOS app alone, so the row is hidden where
 * there is no beta to install. The channels come from the shell without a
 * release check, so the row is there before one answers and when it fails.
 */
function UpdateChannelRow() {
  const channels = useUpdates((s) => s.channels);
  const channel = useUpdates((s) => s.channel);
  useEffect(() => {
    if (canUpdate()) void loadChannels();
  }, []);
  if (!canUpdate() || !channels.includes("beta")) return null;
  return (
    <Row
      title="Update channel"
      subtitle={channel === "beta" ? "Beta: also offers betas of the next version." : "Stable releases only."}
    >
      <Choice
        label="Update channel"
        value={channel}
        onChange={setUpdateChannel}
        options={[
          { value: "stable", label: "Stable" },
          { value: "beta", label: "Beta" },
        ]}
      />
    </Row>
  );
}

/**
 * Settings › About › Install updates automatically: a newer release on the
 * selected channel installs and reopens RustCode by itself once no thread is
 * working. A running turn, an approval or a question is never interrupted.
 */
function AutoInstallRow() {
  const enabled = useUpdates((s) => s.autoInstall);
  if (!canUpdate()) return null;
  return (
    <Row
      title="Install updates automatically"
      subtitle="Install a newer release once no thread is working."
      hint="RustCode reopens by itself. A running turn is never interrupted."
    >
      <Switch label="Install updates automatically" checked={enabled} onChange={setAutoInstall} />
    </Row>
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
  const machine = useApp((s) => s.settingsMachine);
  const current = PAGES.find((s) => s.id === section) ?? PAGES[0]!;
  const label = section === "machines" && machine ? "Environment settings" : current.label;
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
    <div role="dialog" aria-modal="true" aria-label="Settings" data-tab-free className="fixed inset-0 z-30">
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
          className="relative flex size-full max-h-[60rem] max-w-[80rem] overflow-hidden rounded-2xl bg-bg shadow-2 outline-none"
        >
          <RowResets>
            <nav
              aria-label="Settings sections"
              className="flex w-60 shrink-0 flex-col gap-3 bg-sidebar px-2 py-3"
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
                  {...noTextHelpers}
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
              <PanelHeader label={label} />
              <main className="scroll-stable min-h-0 flex-1">
                <div className="@container mx-auto w-full max-w-[760px] px-4 pt-4 pb-24">
                  {section === "general" && <GeneralPage />}
                  {section === "chat" && <ChatPage />}
                  {section === "agents" && <AgentsPage />}
                  {section === "connections" && <ConnectionsPage />}
                  {section === "capabilities" && <CapabilitiesPage />}
                  {section === "remote" && <RemoteAccessPage />}
                  {section === "outside" && <OutsideAgentsPage />}
                  {section === "machines" &&
                    (machine ? <MachineSettingsPage id={machine} /> : <MachinesPage />)}
                  {section === "appearance" && <AppearancePage />}
                  {section === "keyboard" && <ShortcutsPage />}
                  {section === "usage" && <UsagePage />}
                  {section === "statistics" && <StatisticsPage />}
                  {section === "advanced" && <AdvancedPage />}
                  {section === "diagnostics" && <DiagnosticsPage />}
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
    <header className="rule-b flex h-12 shrink-0 items-center gap-3 ps-4 pe-2">
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
