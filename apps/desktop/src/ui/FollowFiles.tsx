import { useState } from "react";
import {
  copyFiles,
  describeCopy,
  FILE_KINDS,
  type FileKind,
  setKindFollowed,
  useFilesFollowOff,
} from "../state/configFilesFollow";
import { type Machine, machineName, selfName } from "../state/machines";
import { copySettingsTo } from "../state/settingsFollow";
import { machineRequest } from "../state/settingsScope";
import { errorText, toast } from "../state/store";
import { LOCAL_HOST } from "../state/threadKeys";
import { Switch } from "./primitives";
import { Action, Row } from "./settingsKit";

const NONE_OFF: FileKind[] = [];

/**
 * Under **Use ‹this computer›'s settings**: which of the config folders travel with it, and a
 * one-off **Fetch from ‹machine›** the other way (state/configFilesFollow.ts).
 */
export function FollowFiles({
  machine,
  online,
  follows,
}: {
  machine: Machine;
  online: boolean;
  follows: boolean;
}) {
  const off = useFilesFollowOff((state) => state.off[machine.id] ?? NONE_OFF);
  const [fetching, setFetching] = useState(false);
  const name = machineName(machine);
  const here = selfName();

  const toggle = (kind: (typeof FILE_KINDS)[number]["kind"], next: boolean) => {
    setKindFollowed(machine.id, kind, next);
    if (next && online) void copySettingsTo(machine.id).catch((error) => toast(errorText(error), "error"));
  };

  const fetchOnce = () => {
    setFetching(true);
    void copyFiles(
      machineRequest,
      machine.id,
      LOCAL_HOST,
      machine.id,
      FILE_KINDS.map((k) => k.kind),
    )
      .then(
        (report) => toast(describeCopy(report, name, here), report ? "success" : "error"),
        (error) => toast(errorText(error), "error"),
      )
      .finally(() => setFetching(false));
  };

  return (
    <>
      {follows &&
        FILE_KINDS.map(({ kind, label }) => (
          <Row
            key={kind}
            title={label}
            subtitle={
              off.includes(kind)
                ? `${name} keeps its own ${label.toLowerCase()}.`
                : `${here}'s ${label.toLowerCase()} are copied to ${name} and kept up to date. ${name}'s own stay.`
            }
          >
            <Switch
              label={`Copy ${here}'s ${label.toLowerCase()} to ${name}`}
              checked={!off.includes(kind)}
              onChange={(next) => toggle(kind, next)}
            />
          </Row>
        ))}
      <Row
        title={`Skills, themes and commands from ${name}`}
        subtitle={`Copy ${name}'s skills, themes and prompt commands to ${here} once, without following it.`}
      >
        <Action disabled={!online || fetching} onClick={fetchOnce}>
          {fetching ? "Fetching…" : `Fetch from ${name}`}
        </Action>
      </Row>
    </>
  );
}
