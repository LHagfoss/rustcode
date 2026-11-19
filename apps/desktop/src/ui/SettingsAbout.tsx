import { useCallback, useEffect, useState } from "react";
import { type PayloadCapture, parsePayloadCapture } from "../acp/rustcode";
import { Method } from "../acp/types";
import { openExternal } from "../lib/native";
import {
  commandIn,
  loadSystemSetup,
  runSystemSetup,
  type SetupStep,
  setupChip,
  systemSetupAvailable,
} from "../lib/systemSetup";
import { canUpdate } from "../lib/updater";
import { APP_VERSION, errorText, request, restartEngine, setOnboarding, toast, useApp } from "../state/store";
import { isRunning } from "../state/thread";
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
import { Info } from "./icons";
import { cx, Spinner, Switch } from "./primitives";
import { Action, Chip, Choice, Group, Loading, Row, withCode } from "./settingsKit";

// Settings, a full panel inside the window: the shell stays mounted behind it,
// hidden and inert, so closing Settings shows the thread exactly where it was.
// A grouped nav on the left searches every row's title and keywords
// (settingsIndex.ts) and shows one page at a time beside it; picking a result
// opens its page and lights the row. After SettingsView.swift.
// Server-backed pages (Harnesses, Connections, Capabilities, Appearance's
// themes, Usage, Statistics) read and write through `_rustcode/*` requests.
// Diagnostics (DiagnosticsPage.tsx) asks the shell about the server itself.

/** A status read on a Settings page ends in an error row within this, never a spinner that stays. */
export const STATUS_READ_MS = 8_000;

/**
 * The app's version and its update, like the SwiftUI app's UpdateControl:
 * Check for Updates, then Update to X (installs in place on macOS, a Linux
 * AppImage or .deb and Windows, quits and reopens) or Download X (the release
 * page, for a copy that cannot replace itself).
 */
function AppVersion() {
  const info = useUpdates((s) => s.info);
  const channel = useUpdates((s) => s.channel);
  const installState = useUpdates((s) => s.install);
  const installing = installState.phase === "installing";
  const progress = installState.phase === "installing" ? installState.progress : null;
  const background = useUpdates((s) => s.error);
  const waiting = installState.phase === "waiting";
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
  const status = useApp((s) => s.engine.phase);
  const anyRunning = useApp((s) => Object.values(s.threads).some((t) => isRunning(t)));
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
      hint="Redacted, clipped and dropped after 7 days. Off, only timing and status are kept."
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
            Restart server
          </Action>
        </Row>
      )}
    </Group>
  );
}

export function AdvancedPage() {
  return (
    <>
      <RecordRequestBodiesGroup />
      <Group title="Setup">
        <Row title="Agent setup" subtitle="Find each harness, sign it in and check that it runs.">
          <Action onClick={() => setOnboarding(true)}>Run setup</Action>
        </Row>
      </Group>
    </>
  );
}

export function AboutPage() {
  return (
    <>
      <Group title="Version" hint="RustCode is open source; its licences ship in each release.">
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
      hint="Runs after an install or an update. RustCode never asks for an administrator silently."
      footer={error ?? undefined}
      action={
        <Action size="sm" disabled={running !== null} onClick={() => void run()}>
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
              title={withCode(step.title)}
              subtitle={withCode(step.message)}
              hint={step.about ? withCode(step.about) : undefined}
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
              {/* A step that stands needs no button of its own: the section's Run again checks it. */}
              {(step.status === "pending" || step.status === "failed" || running === step.id) && (
                <Action disabled={running !== null} onClick={() => void run(step.id)}>
                  {running === step.id ? <Spinner size={12} /> : null}
                  {step.status === "failed" ? "Try again" : "Set up"}
                </Action>
              )}
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
