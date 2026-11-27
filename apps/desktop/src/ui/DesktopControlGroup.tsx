import { useState } from "react";
import { obj } from "../acp/jsonrpc";
import { errorText, request, toast } from "../state/store";
import { Action, Group, Loading, Row, useEngineData } from "./settingsKit";

const KEY = "desktop-control";

function permissionText(granted: unknown): string {
  if (granted === true) return "Granted";
  if (granted === false) return "Not granted";
  return "Unknown";
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
      footer="The first action on an app in a thread asks you; allow the app for the thread to stop asking. Password managers and system settings ask every time. Choose Stop in the menu bar item to end it at once."
    >
      {!supported && (
        <Row
          title="Not available on this computer yet"
          subtitle={typeof settings.reason === "string" ? settings.reason : undefined}
        />
      )}
      {supported && (
        <>
          <Row
            title="Accessibility"
            subtitle="Needed to read apps and act on them."
            hint={permissionText(settings.accessibility)}
          />
          <Row
            title="Screen Recording"
            subtitle="Needed for window screenshots."
            hint={permissionText(settings.screen_recording)}
          />
          {missing && (
            <Row
              title="Grant the permissions"
              subtitle="macOS shows its prompts; turn RustCode on in Privacy & Security, then come back."
              hint={executable ? `macOS lists it as ${executable}` : undefined}
            >
              <Action disabled={pending || !enabled} onClick={() => void save(true, true)}>
                Ask macOS
              </Action>
            </Row>
          )}
          {missing && (
            <Row title="Check again" subtitle="After changing System Settings.">
              <Action disabled={pending} onClick={() => void reload()}>
                Check
              </Action>
            </Row>
          )}
        </>
      )}
    </Group>
  );
}
