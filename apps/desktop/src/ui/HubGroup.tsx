// Settings › Machines › Hub: make this computer the fleet's hub, or join one
// (docs/remote.md › The hub). A hub is a machine that stays on; the others keep
// an outbound link to it, so outside agents reach them through it with no open
// port. The shell runs `rc machines hub …` (`state/hub.ts`).

import { useEffect, useState } from "react";
import { OUTSIDE_SCOPES, type OutsideScope, SCOPE_HELP, SCOPE_LABEL } from "../acp/outside";
import { hasShell } from "../lib/shell";
import { type HubMember, refreshHub, setHub, useHub } from "../state/hub";
import { machineName, useMachines } from "../state/machines";
import { Switch } from "./primitives";
import { Action, Chip, Group, Row, Select } from "./settingsKit";

/** The gateway reports members as it sees links come and go. */
const POLL_MS = 10_000;

const SCOPE_OPTIONS = OUTSIDE_SCOPES.map((scope) => ({ value: scope, label: SCOPE_LABEL[scope] }));

export function HubGroup() {
  const status = useHub((s) => s.status);
  const error = useHub((s) => s.error);
  const busy = useHub((s) => s.busy);

  useEffect(() => {
    if (!hasShell()) return;
    void refreshHub();
    const timer = setInterval(() => void refreshHub(), POLL_MS);
    return () => clearInterval(timer);
  }, []);

  if (!hasShell()) return null;

  return (
    <Group
      title="Hub"
      hint="Optional: machines pair and talk directly without one. A hub is the machine that is always on, such as a VPS. Turn it on there, then join it from each other machine: they keep a link to it, so agents on the hub reach them with no open port, and calls to an offline machine wait on the hub. Threads still run where they started."
    >
      {error && !status && (
        <Row title="Could not read the hub role" status={<span className="text-warning">{error}</span>} />
      )}
      <Row
        title="This computer is the hub"
        subtitle="For a machine that stays on, such as a VPS, rather than a laptop."
        badge={status?.hub ? <Chip tone="ok">Hub</Chip> : undefined}
      >
        <Switch
          label="This computer is the hub"
          checked={status?.hub === true}
          disabled={!status || busy}
          onChange={(on) => void setHub({ action: on ? "on" : "off" })}
        />
      </Row>
      {status?.hub && <Members members={status.members} />}
      {status && (status.member ? <Membership /> : <JoinRows />)}
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
        subtitle="On each one, pair with this computer, then join it under Settings › Machines › Hub."
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
  const member = useHub((s) => s.status?.member);
  const busy = useHub((s) => s.busy);
  if (!member) return null;
  return (
    <Row
      title={`Member of the hub ${member.name}`}
      subtitle={`${SCOPE_LABEL[member.access]} access, ${member.ask ? "asking here before each change" : "without asking"}. The remote gateway here keeps the link.`}
      badge={<Chip tone="ok">Joined</Chip>}
    >
      <Action disabled={busy} onClick={() => void setHub({ action: "leave" })}>
        Leave
      </Action>
    </Row>
  );
}

function JoinRows() {
  const machines = useMachines((s) => s.machines);
  const busy = useHub((s) => s.busy);
  const [machine, setMachine] = useState("");
  const [access, setAccess] = useState<OutsideScope>("admin");
  const [ask, setAsk] = useState(false);
  const chosen = machines.some((m) => m.id === machine) ? machine : "";

  if (machines.length === 0)
    return (
      <Row
        title="Join a hub"
        subtitle="Pair with the machine that is the hub first; it is then listed here to join."
      />
    );

  return (
    <>
      <Row title="Join a hub" subtitle="The always-on machine you made the hub there, such as a VPS.">
        <Select
          label="Hub"
          value={chosen}
          options={[
            { value: "", label: "Choose a machine" },
            ...machines.map((m) => ({ value: m.id, label: machineName(m) })),
          ]}
          onChange={setMachine}
        />
      </Row>
      {chosen && (
        <>
          <Row title="What the hub may do here" subtitle={SCOPE_HELP[access]}>
            <Select
              label="Access"
              value={access}
              options={SCOPE_OPTIONS}
              onChange={(value) => setAccess(value as OutsideScope)}
            />
          </Row>
          {access !== "read_only" && (
            <Row
              title="Ask me before it changes anything"
              subtitle="Each start or message from the hub waits for your Allow here."
            >
              <Switch label="Ask before changes" checked={ask} onChange={setAsk} />
            </Row>
          )}
          <Row title="Join" subtitle="Keep remote access on here: the gateway keeps the link.">
            <Action
              prominent
              disabled={busy}
              onClick={() => void setHub({ action: "join", machine: chosen, access, ask })}
            >
              {busy ? "Joining…" : "Join hub"}
            </Action>
          </Row>
        </>
      )}
    </>
  );
}
