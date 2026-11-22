import { useVirtualizer } from "@tanstack/react-virtual";
import { setTabMovesFocus, tabMovesFocus } from "../lib/focusModality";
import { currentPlatform } from "../lib/platform";
import { ComputerUseGroup } from "./ComputerUseGroup";
import { ProjectGenerators } from "./ProjectGenerators";
import { TextGeneration } from "./TextGeneration";
import { VoiceGroup } from "./VoiceGroup";

// Settings, a full panel inside the window: the shell stays mounted behind it,
// hidden and inert, so closing Settings shows the thread exactly where it was.
// A grouped nav on the left searches every row's title and keywords
// (settingsIndex.ts) and shows one page at a time beside it; picking a result
// opens its page and lights the row. After SettingsView.swift.
// Server-backed pages (Harnesses, Connections, Capabilities, Appearance's
// themes, Usage, Statistics) read and write through `_rustcode/*` requests.
// Diagnostics (DiagnosticsPage.tsx) asks the shell about the server itself.

import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { arr, bool, type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { CHAT_WIDTH_LABEL, type ChatWidth } from "../lib/chatWidth";
import {
  CRASH_REPORTS_DEFAULT,
  crashReportsEnabled,
  setCrashReportsEnabled,
  subscribeCrashReports,
} from "../lib/crashReport";
import { HAPTICS_DEFAULT, haptic, hapticsEnabled, setHaptics } from "../lib/haptics";
import { LINK_PREVIEWS_DEFAULT, linkPreviewsEnabled, setLinkPreviews } from "../lib/links";
import { pickFolder } from "../lib/native";
import { NEXT_PROMPT_DEFAULT, nextPromptEnabled, setNextPrompt } from "../lib/nextPrompt";
import { AUTO_COMPACT_DEFAULT_THRESHOLD, AUTO_COMPACT_MODES } from "../state/autoCompact";
import { validateDefault } from "../state/harness";
import { runAllHarnessChecks, useHarnessChecks } from "../state/harnessChecks";
import { PERMISSION_MODES } from "../state/permissions";
import { RECAP_HOUR_OPTIONS } from "../state/recap";
import {
  useLoadScopeSettings,
  useProjectScope,
  useScope,
  useScopedDefaultHarness,
  useScopedSetting,
  useSettingsSave,
} from "../state/settingsScope";
import {
  errorText,
  type NewThreadChoice,
  openSettingAt,
  openSettings,
  request,
  SETTINGS_DEFAULTS,
  type SettingsSection,
  setChatWidth,
  setDefaultCwd,
  setNotificationsBackgroundOnly,
  setRecapHours,
  setRecapWhileAway,
  setShowToolCalls,
  setSmartUi,
  setSystemNotifications,
  settingLanded,
  useApp,
} from "../state/store";
import { AddHarnessForm } from "./AddHarness";
import {
  Blocks,
  ChartColumn,
  Check,
  Gauge,
  Settings as Gear,
  Info,
  Keyboard,
  KeyRound,
  Laptop,
  Layers,
  MessageSquare,
  Mic,
  Network,
  Palette,
  Plug,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  ShieldCheck,
  Smartphone,
  Stethoscope,
  TerminalSquare,
  X,
} from "./icons";
import { ViewHeader } from "./MainHeader";
import { RestartToApply } from "./RestartToApply";
import { AboutPage, AdvancedPage, STATUS_READ_MS } from "./SettingsAbout";
import { AppearancePage } from "./SettingsAppearance";
import { OrchestrationPage } from "./SettingsOrchestration";

const CapabilitiesPage = lazy(() => import("./Capabilities").then((m) => ({ default: m.CapabilitiesPage })));

const DiagnosticsPage = lazy(() => import("./DiagnosticsPage").then((m) => ({ default: m.DiagnosticsPage })));

import { DefaultsForm, HarnessList, useAutoDetect } from "./Harnesses";
import { HeaderButtonsSetting } from "./HeaderButtonsSetting";
import { MachineSettingsPage } from "./MachineSettingsPage";
import { MachinesPage } from "./MachinesPage";

const OutsideAgentsPage = lazy(() =>
  import("./OutsideAgentsPage").then((m) => ({ default: m.OutsideAgentsPage })),
);
const PermissionsPage = lazy(() => import("./PermissionsPage").then((m) => ({ default: m.PermissionsPage })));

import { cx, Kbd, noTextHelpers, Spinner, Switch, Tooltip } from "./primitives";
import { RemoteAccessPage } from "./RemoteAccessPage";
import { SchemeWarning } from "./SchemeWarning";
import { ScopedHarnessList } from "./ScopedHarnesses";
import { MixedChip, ScopedValue, SettingsScopeBar, SourceChip } from "./SettingsScope";
import { ShortcutsPage } from "./ShortcutSettings";

const StatisticsPage = lazy(() => import("./StatisticsPage").then((m) => ({ default: m.StatisticsPage })));

import { refreshServerService, useMachines } from "../state/machines";
import { useSettingsAttention } from "../state/settingsAttention";
import { type SettingEntry, searchSettings } from "./settingsIndex";
import {
  Action,
  boolLabel,
  Chip,
  Choice,
  Group,
  Loading,
  Row,
  RowResets,
  Slider,
  useEngineData,
  useRowResets,
  withCode,
} from "./settingsKit";

const UsagePage = lazy(() => import("./UsagePage").then((m) => ({ default: m.UsagePage })));

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
      { id: "permissions", label: "Permissions", icon: ShieldCheck },
      { id: "appearance", label: "Appearance", icon: Palette },
      { id: "keyboard", label: "Keyboard", icon: Keyboard },
    ],
  },
  {
    // What runs a thread and what it can reach.
    heading: "Agents",
    pages: [
      { id: "agents", label: "Harnesses", icon: Layers },
      { id: "orchestration", label: "Orchestration", icon: Network },
      { id: "capabilities", label: "Capabilities", icon: Blocks },
      { id: "connections", label: "Connections", icon: KeyRound },
      { id: "voice", label: "Voice & computer use", icon: Mic },
    ],
  },
  {
    // Other computers, phones and agents that work with this app.
    heading: "Devices & access",
    pages: [
      { id: "machines", label: "Machines", icon: Laptop },
      { id: "remote", label: "Remote access", icon: Smartphone },
      { id: "outside", label: "Outside agents", icon: Plug },
    ],
  },
  {
    heading: "Activity",
    pages: [
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

function PresenceRow() {
  const projectCwd = useApp((s) => s.projectCwd || s.defaultCwd);
  const status = useApp((s) => s.engine.phase);
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
  const [crashReports, setCrashReportsState] = useState(crashReportsEnabled);
  useEffect(() => subscribeCrashReports(() => setCrashReportsState(crashReportsEnabled())), []);
  return (
    <>
      <SettingsScopeBar />
      <SchemeWarning />
      <Group title="New threads">
        <Row
          title="Default project folder"
          subtitle="Where a new thread starts, unless you pick a folder for it."
          hint="Used in place of the open thread's folder. A folder picked for one new thread still wins."
          status={defaultCwd || "None chosen"}
        >
          <Action onClick={() => void pickFolder(defaultCwd).then((p) => p && setDefaultCwd(p))}>
            {defaultCwd ? "Change folder" : "Choose folder"}
          </Action>
        </Row>
        <Row
          title="Access"
          subtitle="What threads may do without asking, by default and per project."
          status={permissionLabel(defaultAccess.value ?? SETTINGS_DEFAULTS.defaultPermissionMode)}
        >
          <Action onClick={() => openSettings("permissions")}>Open Permissions</Action>
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
          <ScopedValue mixed={resume.mixed} breakdown={resume.breakdown} project={resume.project}>
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
          <ScopedValue mixed={afterLimit.mixed} breakdown={afterLimit.breakdown} project={afterLimit.project}>
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
          <ScopedValue
            mixed={autoCompactMode.mixed}
            breakdown={autoCompactMode.breakdown}
            project={autoCompactMode.project}
          >
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
            <ScopedValue
              mixed={autoCompactThreshold.mixed}
              breakdown={autoCompactThreshold.breakdown}
              project={autoCompactThreshold.project}
            >
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
        <Row
          title="Send crash reports"
          reset={
            crashReports !== CRASH_REPORTS_DEFAULT
              ? () => {
                  setCrashReportsEnabled(CRASH_REPORTS_DEFAULT);
                  setCrashReportsState(CRASH_REPORTS_DEFAULT);
                }
              : undefined
          }
          subtitle="Sends stack traces, app version and OS when RustCode crashes. Never prompts, code, file paths or secrets."
        >
          <Switch
            label="Send crash reports"
            checked={crashReports}
            onChange={(value) => {
              setCrashReportsEnabled(value);
              setCrashReportsState(value);
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
  const showToolCalls = useApp((s) => s.showToolCalls);
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
      <Row
        title="Show tool calls"
        reset={showToolCalls ? undefined : () => setShowToolCalls(SETTINGS_DEFAULTS.showToolCalls)}
        subtitle="Show the commands, reads and edits a turn makes."
        hint="Off, a turn shows its “Worked for” line and the reply. Approvals, questions, errors, plans and changed files still show."
      >
        <Switch label="Show tool calls" checked={showToolCalls} onChange={setShowToolCalls} />
      </Row>
      {smartUiPending && <RestartToApply />}
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
  const chosenProject = useProjectScope((state) => state.project);
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
            // Where the value comes from sits in the title row, beside the title it qualifies,
            // instead of crowding the top edge of the card.
            saved || scoped.project || scoped.mixed ? (
              <div className="flex items-center gap-2">
                {scoped.project ? (
                  <SourceChip project={scoped.project} />
                ) : (
                  scoped.mixed && <MixedChip breakdown={scoped.breakdown} />
                )}
                {saved && (
                  <Chip tone="ok">
                    <Check size={12} /> Saved
                  </Chip>
                )}
              </div>
            ) : undefined
          }
          hint="A thread can still switch harness and model from the composer."
        >
          <DefaultsForm snapshot={snapshot} value={value} onChange={change} />
        </Group>
      )}
      <Group
        title="Harnesses"
        action={
          thisMachine ? (
            <div className="flex items-center gap-1">
              {checkingAll && (
                <span className="text-caption text-fg-muted tabular-nums">
                  Checking {Math.min(checkingAll.done + 1, checkingAll.total)} of {checkingAll.total}…
                </span>
              )}
              <Tooltip label="Check all">
                <button
                  type="button"
                  aria-label="Check all harnesses"
                  onClick={() => void runAllHarnessChecks()}
                  disabled={checking}
                  className="inline-flex size-6 items-center justify-center rounded-full text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg disabled:opacity-60"
                >
                  {checking ? <Spinner size={12} /> : <RefreshCw size={13} />}
                </button>
              </Tooltip>
              <Action size="sm" onClick={() => setAdding(true)} disabled={adding}>
                <Plus size={12} />
                Add harness
              </Action>
            </div>
          ) : undefined
        }
        footer={
          thisMachine
            ? "Each harness signs in on its own."
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
      {thisMachine && <TextGeneration />}
      {thisMachine && chosenProject && <ProjectGenerators project={chosenProject} />}
    </>
  );
}

/** Dictation in the composer and the browser tools: both kept by this machine, each behind a switch. */
function VoicePage() {
  const { scope, machines } = useScope();
  const remote =
    scope.mode === "machine"
      ? machines.find((machine) => machine.host === scope.host && !machine.local)
      : undefined;
  return (
    <>
      <SettingsScopeBar machineOnly />
      {remote ? (
        <Group title="Voice and computer use">
          <Row
            title={`Set these on ${remote.name} itself`}
            subtitle="Dictation and browser tools are kept by each machine."
          />
        </Group>
      ) : (
        <>
          <VoiceGroup />
          <ComputerUseGroup />
        </>
      )}
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
      <Group title="Model accounts" footer={withCode("Sign in or out with `/login` in a thread.")}>
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

// ---------------------------------------------------------------- the panel

/**
 * Settings in the app's own frame (docs/design.md › Settings): the sidebar keeps
 * its title row (the gear there closes Settings) and swaps its thread list for
 * this section list, and the main column swaps its view for the page. The
 * thread list and the view stay mounted underneath, hidden and inert, so Esc,
 * ⌘, or the gear put you back exactly where you were.
 */
export function SettingsNav() {
  const section = useApp((s) => s.settings) ?? "general";
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const field = useRef<HTMLInputElement>(null);
  const results = useMemo(() => searchSettings(query), [query]);
  const resultsScroll = useRef<HTMLDivElement>(null);
  const virtualResults = useVirtualizer({
    count: query.trim() ? results.length : 0,
    getScrollElement: () => resultsScroll.current,
    estimateSize: () => 48,
    overscan: 5,
  });
  useEffect(() => {
    if (query.trim()) virtualResults.scrollToIndex(active, { align: "auto" });
  }, [active, query, virtualResults]);
  // A dot on each page that needs attention: the gear's dot says something does, the nav says where.
  const attention = useSettingsAttention();
  // The list takes focus when Settings opens: keys then land in Settings, never on the
  // thread they replaced. Not its first control, so no stray ring in the search field.
  const nav = useRef<HTMLElement>(null);
  useEffect(() => {
    nav.current?.focus({ preventScroll: true });
  }, []);

  const clear = () => {
    setQuery("");
    setActive(0);
  };

  const pick = (entry: SettingEntry) => {
    openSettingAt(entry.section, entry.title);
    clear();
    field.current?.blur();
  };

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
  // stops there; with nothing left to clear it closes Settings instead.
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
    <nav
      ref={nav}
      tabIndex={-1}
      aria-label="Settings sections"
      data-focus-area="sections"
      className="flex h-full min-h-0 flex-col gap-3 px-3 pb-3 outline-none"
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
      <div ref={resultsScroll} className="scroll-stable flex min-h-0 flex-1 flex-col gap-4 pb-1">
        {query.trim() ? (
          results.length ? (
            <div
              role="listbox"
              aria-label="Settings results"
              className="relative shrink-0"
              style={{ height: virtualResults.getTotalSize() }}
            >
              {virtualResults.getVirtualItems().map((item) => {
                const i = item.index;
                const r = results[i]!;
                return (
                  <button
                    key={`${r.section}:${r.title}`}
                    style={{
                      position: "absolute",
                      top: item.start,
                      height: item.size,
                      width: "100%",
                    }}
                    aria-posinset={i + 1}
                    aria-setsize={results.length}
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
                );
              })}
            </div>
          ) : (
            <p className="px-2 py-1.5 text-caption text-fg-muted">Nothing matches “{query.trim()}”.</p>
          )
        ) : (
          NAV.map((group) => (
            <div key={group.heading ?? group.pages[0]?.id} className="flex flex-col gap-1">
              {group.heading && (
                <div className="px-2 pb-0.5 font-medium text-callout text-fg-muted">{group.heading}</div>
              )}
              {group.pages.map((s) => (
                <button
                  key={s.id}
                  type="button"
                  aria-current={s.id === section ? "page" : undefined}
                  aria-description={attention[s.id]}
                  onClick={() => openSettings(s.id)}
                  className={cx(
                    "flex h-8 items-center gap-2 rounded-md px-2 text-left font-medium text-body",
                    s.id === section ? "bg-fill text-fg" : "text-fg-muted hover:bg-fill hover:text-fg",
                  )}
                >
                  <s.icon size={16} className="shrink-0" />
                  <span className="min-w-0 flex-1 truncate">{s.label}</span>
                  {attention[s.id] && (
                    <span
                      data-testid={`settings-attention-${s.id}`}
                      title={attention[s.id]}
                      className="size-1.5 shrink-0 rounded-full bg-warning"
                    />
                  )}
                </button>
              ))}
            </div>
          ))
        )}
      </div>
    </nav>
  );
}

/** The open Settings page, in the main column in place of the view. */
export function SettingsPage() {
  const section = useApp((s) => s.settings) ?? "general";
  const machine = useApp((s) => s.settingsMachine);
  const current = PAGES.find((s) => s.id === section) ?? PAGES[0]!;
  const label = section === "machines" && machine ? "Machine settings" : current.label;
  // The row to land on, set when a result is picked (in the nav or in ⌘K) and cleared once it is lit.
  const landing = useApp((s) => s.settingsLanding);
  // The server's warnings are read once when Settings opens, if no page has read them.
  useEffect(() => {
    if (useMachines.getState().server === null) void refreshServerService();
  }, []);

  // The picked row lands: scroll to it once the page is on screen, then light it
  // for a moment. The class comes off in a timer of its own, so the effect can
  // clear its target at once without leaving a row lit.
  useEffect(() => {
    if (!landing || landing.section !== section) return;
    let frame = 0;
    let again = 0;
    frame = requestAnimationFrame(() => {
      again = requestAnimationFrame(() => {
        settingLanded();
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

  return (
    <section aria-label="Settings" data-tab-free className="flex min-w-0 flex-1 flex-col bg-bg">
      <RowResets>
        <PanelHeader label={label} />
        <main aria-label={label} data-focus-area="section" className="scroll-stable min-h-0 flex-1">
          <div className="@container mx-auto w-full max-w-[760px] px-4 pt-4 pb-24">
            <Suspense fallback={<Loading error={null} />}>
              {section === "general" && <GeneralPage />}
              {section === "chat" && <ChatPage />}
              {section === "permissions" && <PermissionsPage />}
              {section === "agents" && <AgentsPage />}
              {section === "orchestration" && <OrchestrationPage />}
              {section === "voice" && <VoicePage />}
              {section === "connections" && <ConnectionsPage />}
              {section === "capabilities" && <CapabilitiesPage />}
              {section === "remote" && <RemoteAccessPage />}
              {section === "outside" && <OutsideAgentsPage />}
              {section === "machines" && (machine ? <MachineSettingsPage id={machine} /> : <MachinesPage />)}
              {section === "appearance" && <AppearancePage />}
              {section === "keyboard" && <ShortcutsPage />}
              {section === "usage" && <UsagePage />}
              {section === "statistics" && <StatisticsPage />}
              {section === "advanced" && <AdvancedPage />}
              {section === "diagnostics" && <DiagnosticsPage />}
              {section === "about" && <AboutPage />}
            </Suspense>
          </div>
        </main>
      </RowResets>
    </section>
  );
}

/**
 * The page's header, the main column's own title row: where you are, then the
 * save status and "Restore defaults", which only shows while a row on this page
 * can go back to its default and puts back everything that changed at once
 * (settingsKit.tsx › RowResets). The sidebar's gear closes Settings.
 */
function PanelHeader({ label }: { label: string }) {
  const save = useSettingsSave();
  const resets = useRowResets();
  return (
    <ViewHeader
      title={
        <nav aria-label="Settings breadcrumb" className="min-w-0">
          <ol className="m-0 flex min-w-0 list-none items-center gap-2 p-0 font-medium text-body">
            <li className="shrink-0 text-fg-muted">Settings</li>
            <li aria-hidden className="shrink-0 text-fg-faint">
              /
            </li>
            <li aria-current="page" className="min-w-0 truncate text-fg">
              {label}
            </li>
          </ol>
        </nav>
      }
    >
      <span role="status" className="text-caption text-fg-muted">
        {save.pending ? "Saving…" : save.error ? `Could not save: ${save.error}` : save.saved ? "Saved" : ""}
      </span>
      {resets.count > 0 && (
        <Action quiet onClick={() => resets.restore()}>
          <RotateCcw size={13} />
          Restore defaults
        </Action>
      )}
    </ViewHeader>
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
