import { useEffect, useState } from "react";
import { obj } from "../acp/jsonrpc";
import { openDesktopControlSettings } from "../lib/nativeAccess";
import { errorText, request, toast } from "../state/store";
import { Action, Group, Loading, Row, useEngineData } from "./settingsKit";

const KEY = "desktop-control";
/** How often the page looks again while a permission is still off. */
const RECHECK_MS = 2000;

function permissionText(granted: unknown): string {
  if (granted === true) return "Allowed";
  if (granted === false) return "Off";
  return "Unknown";
}

function openPane(permission: "accessibility" | "screen_recording") {
  openDesktopControlSettings(permission).catch((e: unknown) => toast(errorText(e), "error"));
}

/**
 * Settings › Voice & computer use › Desktop control: the switch for the
 * `computer_*` tools on this machine, and the macOS permissions they need.
 */
export function DesktopControlGroup() {
  const { data, error, reload, setData } = useEngineData(
    "_rustcode/settings/get",
    (answer) => obj(obj(answer)?.values)?.[KEY] ?? null,
    { keys: [KEY] },
  );
  const [pending, setPending] = useState(false);
  const settings = obj(data);
  async function save(enabled: boolean, requestPermissions: boolean) {
    setPending(true);
    try {
      const answer = await request("_rustcode/settings/set", {
        values: { [KEY]: { enabled, request_permissions: requestPermissions } },
      });
      setData(obj(obj(answer)?.values)?.[KEY] ?? null);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setPending(false);
    }
  }
  if (!settings)
    return (
      <Group title="Desktop control">
        <Loading error={error} onRetry={() => void reload()} />
      </Group>
    );
  const enabled = settings.enabled === true;
  const supported = settings.supported === true;
  const missing = settings.accessibility !== true || settings.screen_recording !== true;
  const executable = typeof settings.executable === "string" ? settings.executable : null;
  const listedAs = settings.hosted === true ? "RustCode" : executable;
  const watching = supported && enabled && missing;
  return (
    <Group
      title="Desktop control"
      switchLabel="Desktop control"
      hint="Let agents see and use this computer's apps, with their own cursor. Off by default."
      enabled={enabled}
      disabled={!supported}
      onEnabledChange={(value) => {
        if (!pending) void save(value, value);
      }}
      footer="The first action on an app in a thread asks you; allow the app for the thread to stop asking. Password managers and system settings ask every time. Full access allows it without asking. Turn this off to end it at once."
    >
      {!supported && (
        <Row
          title="Not available on this computer yet"
          subtitle={typeof settings.reason === "string" ? settings.reason : undefined}
        />
      )}
      {watching && <Recheck reload={reload} />}
      {supported && (
        <>
          <PermissionRow
            title="Accessibility"
            subtitle="Needed to read apps and act on them."
            granted={settings.accessibility}
            enabled={enabled}
            onOpen={() => openPane("accessibility")}
          />
          <PermissionRow
            title="Screen Recording"
            subtitle="Needed for window screenshots."
            granted={settings.screen_recording}
            enabled={enabled}
            onOpen={() => openPane("screen_recording")}
          />
          {watching && (
            <Row
              title="Turn RustCode on in each list"
              subtitle="This page notices the change by itself; no restart needed."
              hint={listedAs ? `macOS lists it as ${listedAs}` : undefined}
            >
              <Action disabled={pending} onClick={() => void save(true, true)}>
                Ask macOS
              </Action>
            </Row>
          )}
        </>
      )}
    </Group>
  );
}

function PermissionRow({
  title,
  subtitle,
  granted,
  enabled,
  onOpen,
}: {
  title: string;
  subtitle: string;
  granted: unknown;
  enabled: boolean;
  onOpen: () => void;
}) {
  return (
    <Row title={title} subtitle={subtitle} hint={permissionText(granted)}>
      {enabled && granted !== true && <Action onClick={onOpen}>Open System Settings</Action>}
    </Row>
  );
}

/** Reads the status again every few seconds while a permission is off. */
function Recheck({ reload }: { reload: () => Promise<unknown> }) {
  useEffect(() => {
    const timer = window.setInterval(() => void reload(), RECHECK_MS);
    return () => window.clearInterval(timer);
  }, [reload]);
  return null;
}
