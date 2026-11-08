// Settings › Remote access, the parts that answer devices (docs/remote.md › Connection requests):
// the phones asking to connect right now, with the same Accept and Deny as the prompt the app shows
// anywhere (ui/ConnectRequests.tsx), and Revoke on a paired device.

import { useState } from "react";
import type { RemoteDevice } from "../acp/remote";
import { deviceLabel, promptedRequests } from "../state/connectRequests";
import { answerConnectRequest, askConfirm, revokeRemoteDevice, useApp } from "../state/store";
import { Spinner } from "./primitives";
import { Action, Group, Row } from "./settingsKit";

/** The devices asking to connect; nothing while none is. */
export function ConnectRequestsGroup() {
  const remote = useApp((s) => s.remote);
  const [answering, setAnswering] = useState<string | null>(null);
  const requests = promptedRequests(remote);
  if (!requests.length) return null;
  const answer = async (id: string, accept: boolean) => {
    setAnswering(id);
    await answerConnectRequest(id, accept);
    setAnswering(null);
  };
  return (
    <Group
      title="Asking to connect"
      footer="A device on this network asked to connect. Accept shows a code to enter on it; only that device can use it."
    >
      {requests.map((request) => (
        <Row
          key={request.id}
          title={request.deviceName}
          subtitle={`${request.deviceKind === "desktop" ? "Can see and run all threads on this machine. " : ""}${deviceLabel(request.deviceKind, request.platform)}${request.remoteAddr ? ` at ${request.remoteAddr}` : ""}`}
        >
          <Action disabled={answering === request.id} onClick={() => void answer(request.id, false)}>
            Deny
          </Action>
          <Action prominent disabled={answering === request.id} onClick={() => void answer(request.id, true)}>
            {answering === request.id && <Spinner />}
            Accept
          </Action>
        </Row>
      ))}
    </Group>
  );
}

/** Revoke one paired device, after asking. */
export function RevokeDeviceAction({ device }: { device: RemoteDevice }) {
  const [busy, setBusy] = useState(false);
  const revoke = () =>
    askConfirm({
      title: `Revoke ${device.name}?`,
      message: `${device.name} is disconnected and can no longer reach this computer's threads. To use it again, pair it again.`,
      action: "Revoke",
      destructive: true,
      run: () => {
        setBusy(true);
        void revokeRemoteDevice(device.id).finally(() => setBusy(false));
      },
    });
  return (
    <Action quiet size="sm" disabled={busy} onClick={revoke} aria-label={`Revoke ${device.name}`}>
      {busy && <Spinner />}
      Revoke
    </Action>
  );
}
