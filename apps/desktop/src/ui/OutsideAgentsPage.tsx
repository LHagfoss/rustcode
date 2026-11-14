// Settings › Outside agents (docs/mcp-outside-agents.md): other apps — Claude
// Desktop, Cursor, Codex, a script — that use RustCode's MCP tools with a
// token and an access level of their own.
//
// The engine owns the grants and checks the level on every call, so lowering
// an app's access or revoking it applies to its next call. This page lists the
// connected apps (with when each last called), connects a new one (name, app,
// access, then the config to paste, shown once), and shows the calls waiting
// for an answer and the recent ones.

import { Plug } from "lucide-react";
import { useEffect, useState } from "react";
import {
  lastUsedLabel,
  OUTSIDE_PLATFORMS,
  OUTSIDE_SCOPES,
  type OutsideCreated,
  type OutsideGrant,
  type OutsidePlatform,
  type OutsideScope,
  outcomeLabel,
  PLATFORM_LABEL,
  SCOPE_HELP,
  SCOPE_LABEL,
} from "../acp/outside";
import {
  answerOutsideCall,
  connectOutsideApp,
  OUTSIDE_POLL_MS,
  refreshOutside,
  revokeOutsideApp,
  updateOutsideApp,
  useOutside,
} from "../state/outside";
import { askConfirm, useApp } from "../state/store";
import { copyText, cx, Switch } from "./primitives";
import { Action, Chip, fieldClass, Group, Loading, Row, Select } from "./settingsKit";

const SCOPE_OPTIONS = OUTSIDE_SCOPES.map((scope) => ({ value: scope, label: SCOPE_LABEL[scope] }));
const PLATFORM_OPTIONS = OUTSIDE_PLATFORMS.map((platform) => ({
  value: platform,
  label: PLATFORM_LABEL[platform],
}));

export function OutsideAgentsPage() {
  const engine = useApp((s) => s.status);
  const status = useOutside((s) => s.status);
  const error = useOutside((s) => s.error);
  const [created, setCreated] = useState<OutsideCreated | null>(null);

  useEffect(() => {
    if (engine !== "ready") return;
    void refreshOutside();
    const timer = setInterval(() => void refreshOutside(), OUTSIDE_POLL_MS);
    return () => clearInterval(timer);
  }, [engine]);

  if (!status) return <Loading error={error} onRetry={() => void refreshOutside()} />;

  return (
    <>
      <p className="mb-5 max-w-[70ch] px-4 text-callout text-fg-muted">
        Let another app use RustCode's threads: read them, start work, or message a thread. Each app gets a
        token of its own with an access level you choose, and you can change or revoke it here at any time.
        The token is only for RustCode's tools on this computer; it opens nothing else.
      </p>
      {!status.available && (
        <Group title="Not available here">
          <Row
            title="Outside agents need macOS or Linux"
            subtitle="The connection runs over a local socket this system does not have."
          />
        </Group>
      )}
      <Group title="Connection">
        <Row
          title="HTTP connection URL"
          subtitle={status.httpUrl ?? "HTTP endpoint is not running. Restart RustCode to try again."}
          status={status.httpUrl ? "For apps using Streamable HTTP and OAuth on this computer." : undefined}
        >
          {status.httpUrl && <Action onClick={() => void copyText(status.httpUrl ?? "")}>Copy URL</Action>}
        </Row>
        <Row title="Stdio command" subtitle={status.command}>
          <Action onClick={() => void copyText(status.command)}>Copy command</Action>
        </Row>
      </Group>
      {status.pending.length > 0 && (
        <Group title="Waiting for you">
          {status.pending.map((item) => (
            <Row key={item.id} title={`${item.clientName} wants to run ${item.tool}`} subtitle={item.summary}>
              <Action danger onClick={() => void answerOutsideCall(item.id, false)}>
                Decline
              </Action>
              <Action prominent onClick={() => void answerOutsideCall(item.id, true)}>
                Allow
              </Action>
            </Row>
          ))}
        </Group>
      )}
      <Group
        title="Connected apps"
        footer="Read-only apps never ask. Above that, each change is put to you here first unless you turn that off for the app."
      >
        {status.grants.length === 0 ? (
          <Row
            title="No app is connected"
            subtitle="Connect one below, then paste the config it gives you."
          />
        ) : (
          status.grants.map((grant) => <GrantRow key={grant.id} grant={grant} />)
        )}
      </Group>
      <ConnectGroup
        full={status.grants.length >= status.maxGrants}
        disabled={!status.available}
        created={created}
        onCreated={setCreated}
      />
      <Group
        title="Recent activity"
        footer="Who called which tool and how it ended. Arguments and answers are not kept."
      >
        {status.recent.length === 0 ? (
          <Row title="Nothing yet" subtitle="Calls from connected apps appear here." />
        ) : (
          status.recent
            .slice(0, 12)
            .map((call, index) => (
              <Row
                key={`${call.at}-${index}`}
                title={`${call.clientName} · ${call.tool}`}
                subtitle={`${new Date(call.at).toLocaleString()}${call.detail ? ` · ${call.detail}` : ""}`}
                badge={
                  <Chip tone={call.outcome === "ok" ? "ok" : call.outcome === "denied" ? "bad" : "warn"}>
                    {outcomeLabel(call.outcome)}
                  </Chip>
                }
              />
            ))
        )}
      </Group>
    </>
  );
}

function GrantRow({ grant }: { grant: OutsideGrant }) {
  const [now] = useState(() => Date.now());
  const used = lastUsedLabel(grant.lastUsedAt, now);
  return (
    <Row
      title={grant.name}
      subtitle={`${PLATFORM_LABEL[grant.platform]} · ${used}${grant.lastTool ? ` (${grant.lastTool})` : ""}`}
      status={SCOPE_HELP[grant.scope]}
      stacked
    >
      <div className="flex flex-wrap items-center gap-3">
        <Select
          label={`${grant.name} access`}
          value={grant.scope}
          options={SCOPE_OPTIONS}
          onChange={(scope) => void updateOutsideApp(grant.id, { scope: scope as OutsideScope })}
        />
        {grant.scope !== "read_only" && (
          <span className="flex items-center gap-2 text-callout text-fg-muted">
            Ask me first
            <Switch
              label={`Ask before ${grant.name} changes anything`}
              checked={grant.ask}
              onChange={(ask) => void updateOutsideApp(grant.id, { ask })}
            />
          </span>
        )}
        <Action
          danger
          onClick={() =>
            askConfirm({
              title: `Revoke ${grant.name}?`,
              message: `${grant.name} loses access at once: its next call is refused, and calls waiting for you are declined. Threads it already started keep running. To use it again, connect it again with a new token.`,
              action: "Revoke",
              destructive: true,
              run: () => void revokeOutsideApp(grant.id, grant.name),
            })
          }
        >
          Revoke
        </Action>
      </div>
    </Row>
  );
}

function ConnectGroup({
  full,
  disabled,
  created,
  onCreated,
}: {
  full: boolean;
  disabled: boolean;
  created: OutsideCreated | null;
  onCreated: (created: OutsideCreated | null) => void;
}) {
  const [name, setName] = useState("");
  const [platform, setPlatform] = useState<OutsidePlatform>("claude-desktop");
  const [scope, setScope] = useState<OutsideScope>("read_only");
  const [ask, setAsk] = useState(true);
  const [busy, setBusy] = useState(false);

  const connect = async () => {
    setBusy(true);
    const made = await connectOutsideApp({
      name: name.trim() || PLATFORM_LABEL[platform],
      platform,
      scope,
      ask,
    });
    setBusy(false);
    if (made) {
      onCreated(made);
      setName("");
    }
  };

  return (
    <Group
      title="Connect an app"
      footer="Pick the app and what it may do. RustCode makes a token and the config to paste into that app; the token is shown once."
    >
      <Row title="App" subtitle="Which app will connect.">
        <Select
          label="App"
          value={platform}
          options={PLATFORM_OPTIONS}
          onChange={(value) => setPlatform(value as OutsidePlatform)}
        />
      </Row>
      <Row title="Name" subtitle="How you recognise it here and in the activity list.">
        <input
          aria-label="Name"
          className={cx(fieldClass, "w-56 min-w-0")}
          placeholder={PLATFORM_LABEL[platform]}
          value={name}
          maxLength={60}
          onChange={(event) => setName(event.target.value)}
        />
      </Row>
      <Row title="Access" subtitle={SCOPE_HELP[scope]}>
        <Select
          label="Access"
          value={scope}
          options={SCOPE_OPTIONS}
          onChange={(value) => {
            setScope(value as OutsideScope);
            setAsk(value !== "read_only");
          }}
        />
      </Row>
      {scope !== "read_only" && (
        <Row title="Ask me before it changes anything" subtitle="Each start or message waits for your Allow.">
          <Switch label="Ask before changes" checked={ask} onChange={setAsk} />
        </Row>
      )}
      <Row title="Connect" subtitle={full ? "The most apps are connected; revoke one first." : undefined}>
        <Action prominent disabled={disabled || full || busy} onClick={() => void connect()}>
          <Plug size={14} />
          {busy ? "Connecting…" : "Connect app"}
        </Action>
      </Row>
      {created && <CreatedRow created={created} onDone={() => onCreated(null)} />}
    </Group>
  );
}

function CreatedRow({ created, onDone }: { created: OutsideCreated; onDone: () => void }) {
  return (
    <div className="flex flex-col gap-3 px-4 py-3" data-setting="Config to paste">
      <div>
        <div className="font-medium text-body text-fg">
          {created.grant.name} is connected as {SCOPE_LABEL[created.grant.scope]}
        </div>
        <div className="text-callout text-fg-muted">
          Paste this into {created.snippet.where}, then restart that app. It contains the token, which is
          shown only now: copy it before you close this.
        </div>
      </div>
      <pre className="selectable max-h-64 overflow-auto rounded-lg bg-fill p-3 font-mono text-caption text-fg">
        {created.snippet.text}
      </pre>
      <div className="flex items-center gap-2">
        <Action prominent onClick={() => void copyText(created.snippet.text)}>
          Copy config
        </Action>
        <Action onClick={onDone}>Done</Action>
      </div>
    </div>
  );
}
