// Settings › Machines › Hub: make this computer the fleet's hub, or join one
// (docs/remote.md › The hub). A hub is a machine that stays on; the others keep
// an outbound link to it, so outside agents reach them through it with no open
// port. The shell runs `rc machines hub …` (`state/hub.ts`).

import { useEffect } from "react";
import { SCOPE_LABEL } from "../acp/outside";
import { hasShell } from "../lib/shell";
import { type HubMember, hubChanging, hubError, hubStatus, refreshHub, setHub, useHub } from "../state/hub";
import { chooseHub, reachableMachines, suggestedHub } from "../state/hubFleet";
import { LOCAL_MACHINE_ID, machineName, useMachines } from "../state/machines";
import { Switch } from "./primitives";
import { Action, Chip, Group, Row, Select } from "./settingsKit";

/** The gateway reports members as it sees links come and go. */
const POLL_MS = 10_000;

const NO_HUB = "";

export function HubGroup() {
  const status = useHub(hubStatus);
  const error = useHub(hubError);
  const busy = useHub(hubChanging);
  const machines = useMachines((s) => s.machines);

  useEffect(() => {
    if (!hasShell()) return;
    void refreshHub();
    const timer = setInterval(() => void refreshHub(), POLL_MS);
    return () => clearInterval(timer);
  }, []);

  if (!hasShell()) return null;

  const current = status?.hub ? LOCAL_MACHINE_ID : (status?.member?.machine ?? NO_HUB);
  const reachable = reachableMachines(machines);
  const suggested = suggestedHub(machines);
  const options = [
    { value: NO_HUB, label: "No hub" },
    { value: LOCAL_MACHINE_ID, label: "This computer" },
    ...reachable.map((m) => ({
      value: m.id,
      label: m.id === suggested?.id ? `${machineName(m)} (suggested)` : machineName(m),
    })),
  ];
  if (status?.member && !options.some((option) => option.value === status.member?.machine))
    options.push({ value: status.member.machine, label: status.member.name });

  return (
    <Group
      title="Hub (optional)"
      hint="Most setups need no hub: paired machines talk to each other directly. A hub is a machine that is always on, such as a VPS, that your other machines keep a link open to. Agents on the hub then reach a laptop behind a router with no open port, and work sent to a sleeping machine waits on the hub until it wakes. Threads still run where they started."
    >
      {error && (
        <Row title="Could not read the hub role" status={<span className="text-warning">{error}</span>} />
      )}
      <Row
        title="Hub"
        subtitle={
          suggested && current === NO_HUB
            ? `Pick a machine that stays on, such as ${machineName(suggested)}. It turns the hub on there, and this computer and every connected machine link to it with full access.`
            : "Pick a machine that stays on. It turns the hub on there, and this computer and every connected machine link to it with full access."
        }
        badge={current !== NO_HUB ? <Chip tone="ok">On</Chip> : undefined}
      >
        <Select
          label="Hub"
          value={current}
          options={options}
          disabled={!status || busy}
          onChange={(value) => void chooseHub(value === NO_HUB ? null : value)}
        />
      </Row>
      {status?.hub && (
        <Row
          title="Relay for machines on other networks"
          subtitle="This hub relays for its members, so your devices reach them from anywhere. Let TCP port 17880 through to it."
          hint="Each member registers with the relay on its own. The relay sees which device reaches which machine and how many bytes; what they say stays encrypted end to end."
        >
          <Switch
            label="Relay for machines on other networks"
            checked={status.relay}
            disabled={busy}
            onChange={(on) => void setHub({ action: on ? "relay_on" : "relay_off" })}
          />
        </Row>
      )}
      {status?.hub && <Members members={status.members} />}
      {status?.member && <Membership />}
    </Group>
  );
}

function Members({ members }: { members: HubMember[] | null }) {
  if (members === null)
    return (
      <Row
        title="Its gateway is not running"
        subtitle="No machine can link to it until remote access is on here (Settings › Remote access)."
        badge={<Chip tone="warn">Not reachable</Chip>}
      />
    );
  if (members.length === 0)
    return (
      <Row
        title="No machine has joined yet"
        subtitle="Each machine links when it is connected as you choose the hub; it must be paired with this computer both ways."
      />
    );
  return (
    <>
      {members.map((member) => (
        <Row
          key={member.hostId}
          title={member.name}
          subtitle={`${SCOPE_LABEL[member.access]} access${member.ask ? ", asks before each change" : ""}`}
          status={
            [
              member.online
                ? null
                : member.lastSeenAt
                  ? `Last seen ${new Date(member.lastSeenAt).toLocaleString()}`
                  : null,
              member.queued > 0 ? `${member.queued} waiting` : null,
            ]
              .filter(Boolean)
              .join(" · ") || undefined
          }
          badge={<Chip tone={member.online ? "ok" : "idle"}>{member.online ? "Linked" : "Offline"}</Chip>}
        />
      ))}
    </>
  );
}

function Membership() {
  const member = useHub((s) => hubStatus(s)?.member);
  const busy = useHub(hubChanging);
  if (!member) return null;
  return (
    <Row
      title={
        <>
          Linked to the hub {member.name}
          <Chip tone="ok">Linked</Chip>
        </>
      }
      subtitle={`${SCOPE_LABEL[member.access]} access, ${member.ask ? "asking here before each change" : "without asking"}. The remote gateway here keeps the link.`}
    >
      <Action danger disabled={busy} onClick={() => void setHub({ action: "leave" })}>
        Leave
      </Action>
    </Row>
  );
}
