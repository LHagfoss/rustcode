import { selfName } from "../state/machineLabels";
import { setFollowAll, useSettingsFollow } from "../state/settingsFollow";
import { errorText, toast } from "../state/store";
import { Switch } from "./primitives";
import { Group, Row } from "./settingsKit";

/** Settings › Machines: one switch that keeps every paired machine on this computer's shared settings (state/settingsFollow.ts). */
export function SameSettingsGroup() {
  const all = useSettingsFollow((state) => state.all);
  const here = selfName();
  const toggle = (next: boolean) => {
    void setFollowAll(next).then(
      (reached) => {
        if (next && reached)
          toast(
            `Copied ${here}'s settings to ${reached} ${reached === 1 ? "machine" : "machines"}.`,
            "success",
          );
      },
      (error) => toast(errorText(error), "error"),
    );
  };
  return (
    <Group
      title="Same settings everywhere"
      hint="Credentials, accounts and local paths stay on each machine. `rc config sync` still keeps a copy of your config in Git."
    >
      <Row
        title={`Use ${here}'s settings on every machine`}
        subtitle={
          all
            ? `Every paired machine takes ${here}'s shared settings, skills, themes and commands as it connects and whenever they change here. A change made on another machine is put back.`
            : `Each machine keeps its own settings unless its page says otherwise. Turn this on to copy ${here}'s to all of them and keep them the same.`
        }
      >
        <Switch label={`Use ${here}'s settings on every machine`} checked={all} onChange={toggle} />
      </Row>
    </Group>
  );
}
