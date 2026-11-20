// Settings › Outside agents (docs/mcp-outside-agents.md): other apps that use
// RustCode's MCP tools with an access level of their own. Hosted agents
// (ChatGPT, Claude) connect to a machine's public address and sign in on its
// page; apps on this computer (Claude Desktop, Cursor, Codex) get a token.
//
// The engine owns the grants and checks the level on every call, so lowering
// an app's access or revoking it applies to its next call. Each machine has
// its own: the picker at the top shows any connected machine, so a sign-in
// on a server is approved from this window.

import { useEffect, useState } from "react";
import {
  lastUsedLabel,
  OUTSIDE_PLATFORMS,
  OUTSIDE_SCOPES,
  type OutsideCode,
  type OutsideCreated,
  type OutsideGrant,
  type OutsidePlatform,
  type OutsideScope,
  type OutsideStatus,
  outcomeLabel,
  type PasskeyLink,
  PLATFORM_LABEL,
  SCOPE_HELP,
  SCOPE_LABEL,
  type WebSession,
} from "../acp/outside";
import { hubSession, signOutOfHub } from "../acp/transport";
import { openExternal } from "../lib/native";
import {
  answerOutsideCall,
  connectOutsideApp,
  makeOutsideCode,
  makePasskeyLink,
  OUTSIDE_POLL_MS,
  refreshOutside,
  removePasskey,
  revokeOutsideApp,
  signOutWebSession,
  updateOutsideApp,
  useOutside,
} from "../state/outside";
import { useScopeMachines } from "../state/settingsScope";
import { askConfirm, useApp } from "../state/store";
import { LOCAL_HOST } from "../state/threadKeys";
import { HarnessIcon } from "./HarnessIcon";
import { Copy, ExternalLink, Globe, KeyRound, Plug } from "./icons";
import { AgentTile, LevelPoints, SignInCard } from "./OutsideSignIn";
import { copyText, cx, Switch } from "./primitives";
import { Action, Chip, fieldClass, Group, Loading, Row, Select } from "./settingsKit";

const SCOPE_OPTIONS = OUTSIDE_SCOPES.map((scope) => ({ value: scope, label: SCOPE_LABEL[scope] }));
const PLATFORM_OPTIONS = OUTSIDE_PLATFORMS.map((platform) => ({
  value: platform,
  label: PLATFORM_LABEL[platform],
}));

/** A hosted agent the Connect grid knows, and how its connector is added. */
interface HostedApp {
  id: string;
  name: string;
  /** The `HarnessIcon` mark, or none for a plug. */
  mark: string;
  /** The button: what it does with the URL. */
  action: string;
  run: (url: string) => void;
  steps: string[];
}

const encoded = encodeURIComponent;

export const HOSTED_APPS: HostedApp[] = [
  {
    id: "claude",
    name: "Claude",
    mark: "claude",
    action: "Open in Claude",
    // Claude's add-connector form, filled in.
    run: (url) =>
      openExternal(
        `https://claude.ai/customize/connectors?modal=add-custom-connector&connectorName=RustCode&connectorUrl=${encoded(url)}`,
      ),
    steps: [
      "Claude opens its Add custom connector form with RustCode filled in. Choose Add.",
      "Choose Connect. The RustCode sign-in page opens.",
      "Pick what Claude may do and choose Approve in RustCode, then approve the card that appears here.",
    ],
  },
  {
    id: "chatgpt",
    name: "ChatGPT",
    mark: "openai",
    action: "Copy URL and open ChatGPT",
    run: (url) => {
      void copyText(url);
      openExternal("https://chatgpt.com/#settings/Connectors");
    },
    steps: [
      "In ChatGPT's Apps & Connectors settings, turn on Developer mode under Advanced if it is off.",
      "Choose Create, name it RustCode, paste the URL and pick OAuth.",
      "The RustCode sign-in page opens: choose Approve in RustCode, then approve the card that appears here.",
    ],
  },
  {
    id: "other",
    name: "Another agent",
    mark: "",
    action: "Copy URL",
    run: (url) => void copyText(url),
    steps: [
      "Add a remote MCP server (Streamable HTTP) with OAuth, and paste the URL.",
      "When its sign-in page opens, choose Approve in RustCode, or enter a sign-in code from below.",
    ],
  },
];

export function OutsideAgentsPage() {
  const engine = useApp((s) => s.engine.phase);
  const machines = useScopeMachines().filter((m) => m.online);
  const byHost = useOutside((s) => s.byHost);
  const errors = useOutside((s) => s.errors);
  const [picked, setPicked] = useState<string | null>(null);
  const [created, setCreated] = useState<OutsideCreated | null>(null);

  // Until the owner picks, show the first machine with a public address: that is where
  // hosted agents connect.
  const withPublic = machines.find((m) => byHost[m.host]?.publicUrl);
  const host = picked ?? withPublic?.host ?? LOCAL_HOST;
  const status = byHost[host] ?? null;
  const error = errors[host] ?? null;
  const remote = host !== LOCAL_HOST;
  const machineName =
    machines.find((m) => m.host === host && !m.local)?.name ?? (remote ? status?.machineName : null) ?? null;

  useEffect(() => {
    if (engine !== "ready") return;
    void refreshOutside(host);
    const timer = setInterval(() => void refreshOutside(host), OUTSIDE_POLL_MS);
    return () => clearInterval(timer);
  }, [engine, host]);

  // The same bar as the other machine pages (SettingsScope), so "which machine" sits in one place.
  const picker =
    machines.length > 1 ? (
      <div className="mb-6 flex flex-wrap items-center gap-3 rounded-xl bg-surface px-4 py-3">
        <div className="min-w-0 flex-1 font-medium text-body text-fg">Showing</div>
        <Select
          label="Showing machine"
          value={host}
          options={machines.map((m) => ({ value: m.host, label: m.local ? "This machine" : m.name }))}
          onChange={(next) => {
            setPicked(next);
            setCreated(null);
          }}
        />
      </div>
    ) : null;

  if (!status)
    return (
      <>
        {picker}
        <Loading error={error} onRetry={() => void refreshOutside(host)} />
      </>
    );

  const where = remote ? (machineName ?? "that machine") : "this computer";
  return (
    <>
      {picker}
      <p className="mb-6 max-w-[70ch] px-4 text-callout text-fg-muted">
        Let other AI apps read, start or message RustCode threads on {where}. Each gets its own access level,
        and you can change or revoke it here.
      </p>
      {!status.available && (
        <Group title="Not available here">
          <Row
            title="Outside agents need macOS or Linux"
            subtitle="The connection runs over a local socket this system does not have."
          />
        </Group>
      )}
      {(status.signIns.length > 0 || status.pending.length > 0) && (
        <Group title="Waiting for you">
          {status.signIns.map((signIn) => (
            <SignInCard
              key={signIn.id}
              host={host}
              machine={remote ? machineName : null}
              signIn={signIn}
              levels={status.levels}
            />
          ))}
          {status.pending.map((item) => (
            <Row key={item.id} title={`${item.clientName} wants to run ${item.tool}`} subtitle={item.summary}>
              <Action danger onClick={() => void answerOutsideCall(item.id, false, host)}>
                Decline
              </Action>
              <Action prominent onClick={() => void answerOutsideCall(item.id, true, host)}>
                Allow
              </Action>
            </Row>
          ))}
        </Group>
      )}
      <HostedGroup host={host} status={status} where={where} />
      <Group
        title="Connected apps"
        hint="Read-only apps never ask. Above that, each change is put to you first unless you turn that off for the app."
      >
        {status.grants.length === 0 ? (
          <Row title="No app is connected" subtitle="Connect one above or below." />
        ) : (
          status.grants.map((grant) => <GrantRow key={grant.id} grant={grant} host={host} />)
        )}
      </Group>
      {!remote && (
        <>
          <ConnectGroup
            full={status.grants.length >= status.maxGrants}
            disabled={!status.available}
            created={created}
            onCreated={setCreated}
          />
          <Group title="Connection details" hint="For an app on this computer you set up by hand.">
            <Row
              title="HTTP connection URL"
              subtitle={status.httpUrl ?? "HTTP endpoint is not running. Restart RustCode to try again."}
              status={
                status.httpUrl ? "For apps using Streamable HTTP and OAuth on this computer." : undefined
              }
            >
              {status.httpUrl && (
                <Action onClick={() => void copyText(status.httpUrl ?? "")}>Copy URL</Action>
              )}
            </Row>
            <Row title="Stdio command" subtitle={status.command}>
              <Action onClick={() => void copyText(status.command)}>Copy command</Action>
            </Row>
          </Group>
        </>
      )}
      <Group
        title="Recent activity"
        hint="Who called which tool and how it ended. Arguments and answers are not kept."
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

/** ChatGPT, Claude and other hosted agents: the machine's public address and how to add it. */
function HostedGroup({ host, status, where }: { host: string; status: OutsideStatus; where: string }) {
  const [chosen, setChosen] = useState("claude");
  const url = status.publicUrl;
  if (!url) {
    return (
      <Group title="Connect ChatGPT, Claude and other hosted agents">
        <Row
          title="No public address"
          subtitle={`Hosted agents reach RustCode over the internet, so ${where} needs an https address first.`}
          status={
            status.publicError ??
            "Run a tunnel (cloudflared) to 127.0.0.1:43871 and set [outside] public_url in config.toml; docs/mcp-outside-agents.md › Hosted agents has the steps."
          }
        >
          <Globe size={18} className="text-fg-faint" />
        </Row>
      </Group>
    );
  }
  const app = HOSTED_APPS.find((a) => a.id === chosen) ?? HOSTED_APPS[0];
  return (
    <Group
      title="Connect ChatGPT, Claude and other hosted agents"
      hint="The agent signs in on this machine's page. Approve it there with RustCode, or with a sign-in code."
    >
      <Row title="Connector URL" subtitle={url}>
        <Action onClick={() => void copyText(url)}>
          <Copy size={14} />
          Copy
        </Action>
      </Row>
      {/* One block: the pick, its steps and its action, with no rule between them. */}
      <div className="flex flex-col gap-4 px-4 py-4">
        <div className="grid grid-cols-3 gap-2">
          {HOSTED_APPS.map((item) => (
            <button
              key={item.id}
              type="button"
              aria-pressed={item.id === chosen}
              onClick={() => setChosen(item.id)}
              className={cx(
                "flex flex-col items-center gap-2 rounded-lg px-3 py-3 text-callout",
                item.id === chosen
                  ? "bg-fill-strong text-fg"
                  : "bg-fill-subtle text-fg-muted hover:bg-fill hover:text-fg",
              )}
            >
              <span className="flex size-10 items-center justify-center rounded-md bg-fill text-fg-muted">
                {item.mark ? <HarnessIcon id={item.mark} size={24} /> : <Plug size={18} />}
              </span>
              <span className="font-medium">{item.name}</span>
            </button>
          ))}
        </div>
        {app && (
          <>
            <ol className="flex flex-col gap-2 text-callout text-fg-muted">
              {app.steps.map((step, index) => (
                <li key={step} className="flex items-start gap-2">
                  <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-fill font-semibold text-caption text-fg">
                    {index + 1}
                  </span>
                  <span className="pt-px">{step}</span>
                </li>
              ))}
            </ol>
            <div>
              <Action prominent onClick={() => app.run(url)}>
                {app.id === "claude" ? <ExternalLink size={14} /> : <Copy size={14} />}
                {app.action}
              </Action>
            </div>
          </>
        )}
      </div>
      <CodeRow host={host} status={status} />
      <PasskeyRows host={host} status={status} />
      <WebAppRows host={host} status={status} url={url} />
    </Group>
  );
}

/** The owner's passkeys: each approves a sign-in on the page with no code. */
function PasskeyRows({ host, status }: { host: string; status: OutsideStatus }) {
  const [name, setName] = useState("");
  const [link, setLink] = useState<(PasskeyLink & { until: number }) | null>(null);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!link) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [link]);
  const left = link ? Math.max(0, Math.round((link.until - now) / 1000)) : 0;
  return (
    <>
      <Row
        title="Passkeys"
        subtitle="Approve a sign-in on the page with your passkey, from any device, with no code. Add one by opening a link on the device that keeps it."
        stacked
      >
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center gap-2">
            <input
              className={fieldClass}
              value={name}
              placeholder="Name, such as My phone"
              aria-label="Passkey name"
              onChange={(event) => setName(event.target.value)}
            />
            <Action
              onClick={async () => {
                const made = await makePasskeyLink(host, name);
                if (made) {
                  setNow(Date.now());
                  setLink({ ...made, until: Date.now() + made.minutes * 60_000 });
                }
              }}
            >
              <KeyRound size={14} />
              Add a passkey
            </Action>
          </div>
          {link && left > 0 && (
            <div className="flex flex-col gap-2 rounded-xl bg-fill px-4 py-3">
              <span className="selectable break-all font-mono text-caption text-fg">{link.link}</span>
              <span className="flex items-center gap-2">
                <span className="flex-1 text-caption text-fg-faint tabular-nums">
                  Works once · {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")} left · Do not
                  share it
                </span>
                <Action onClick={() => void copyText(link.link)}>
                  <Copy size={14} />
                  Copy
                </Action>
                <Action onClick={() => void openExternal(link.link)}>
                  <ExternalLink size={14} />
                  Open
                </Action>
              </span>
            </div>
          )}
        </div>
      </Row>
      {status.passkeys.map((passkey) => (
        <Row
          key={passkey.id}
          title={passkey.name}
          subtitle={`${passkey.host} · ${lastUsedLabel(passkey.lastUsedAt)}`}
        >
          <Action
            danger
            onClick={() =>
              askConfirm({
                title: `Remove ${passkey.name}?`,
                message: `${passkey.name} stops approving sign-ins at once. Agents it approved stay connected; revoke them below.`,
                action: "Remove",
                destructive: true,
                run: () => void removePasskey(host, passkey.id, passkey.name),
              })
            }
          >
            Remove
          </Action>
        </Row>
      ))}
    </>
  );
}

/** The app served at the public address, and the browsers signed in to it with a passkey. */
function WebAppRows({ host, status, url }: { host: string; status: OutsideStatus; url: string }) {
  const origin = url.replace(/\/mcp$/, "");
  const signIn = status.passkeys.length > 0;
  return (
    <>
      <Row
        title="Web app"
        subtitle={`Open ${origin} in any browser and sign in with your passkey to use RustCode there. Each browser stays signed in for 30 days.`}
        status={
          !status.webInstalled
            ? "This machine has no web build yet: run `bun run build:hub` in apps/desktop and copy dist-hub to ~/.config/rustcode/web."
            : !signIn
              ? "Add a passkey above first; it is how you sign in."
              : undefined
        }
      >
        {status.webInstalled && signIn && (
          <Action onClick={() => void openExternal(origin)}>
            <ExternalLink size={14} />
            Open
          </Action>
        )}
      </Row>
      {status.webSessions.map((session) => (
        <WebSessionRow key={session.id} host={host} session={session} />
      ))}
    </>
  );
}

function WebSessionRow({ host, session }: { host: string; session: WebSession }) {
  const here = session.id === hubSession();
  return (
    <Row
      title={here ? `${session.name} (this browser)` : session.name}
      subtitle={`Signed in to the web app · ${lastUsedLabel(session.lastUsedAt ?? session.createdAt)}`}
    >
      <Action
        danger
        onClick={() =>
          here
            ? signOutOfHub()
            : askConfirm({
                title: `Sign out ${session.name}?`,
                message: `${session.name} is signed out of the web app at once and has to sign in with a passkey again.`,
                action: "Sign out",
                destructive: true,
                run: () => void signOutWebSession(host, session.id, session.name),
              })
        }
      >
        Sign out
      </Action>
    </Row>
  );
}

/** A one-time code for the sign-in page, for when no RustCode window is at hand to approve. */
function CodeRow({ host, status }: { host: string; status: OutsideStatus }) {
  const [scope, setScope] = useState<OutsideScope>("delegate");
  const [code, setCode] = useState<(OutsideCode & { until: number }) | null>(null);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!code) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [code]);
  const left = code ? Math.max(0, Math.round((code.until - now) / 1000)) : 0;
  const level = status.levels.find((l) => l.scope === scope);
  return (
    <Row
      title="Sign-in code"
      subtitle="Type it on the sign-in page instead of approving here. It works once, for 10 minutes."
      stacked
    >
      <div className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <Select
            label="Most it may do"
            value={scope}
            options={SCOPE_OPTIONS}
            onChange={(value) => setScope(value as OutsideScope)}
          />
          <Action
            onClick={async () => {
              const made = await makeOutsideCode(host, scope, false);
              if (made) {
                setNow(Date.now());
                setCode({ ...made, until: Date.now() + made.minutes * 60_000 });
              }
            }}
          >
            <KeyRound size={14} />
            Get a sign-in code
          </Action>
        </div>
        <LevelPoints level={level} />
        {code && left > 0 && (
          <div className="flex items-center gap-3 rounded-xl bg-fill px-4 py-3">
            <span className="selectable font-mono font-semibold text-fg text-title2 tracking-[0.18em]">
              {code.code}
            </span>
            <span className="flex-1 text-caption text-fg-faint tabular-nums">
              {SCOPE_LABEL[code.scope]} · {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")} left
            </span>
            <Action onClick={() => void copyText(code.code)}>Copy</Action>
          </div>
        )}
      </div>
    </Row>
  );
}

function GrantRow({ grant, host }: { grant: OutsideGrant; host: string }) {
  const [now] = useState(() => Date.now());
  const used = lastUsedLabel(grant.lastUsedAt, now);
  return (
    <Row
      title={
        <span className="flex items-center gap-2">
          {grant.hosted && <AgentTile name={grant.name} size={24} />}
          {grant.name}
        </span>
      }
      subtitle={`${grant.hosted ? signedInLabel(grant.signedInUntil) : PLATFORM_LABEL[grant.platform]} · ${used}${grant.lastTool ? ` (${grant.lastTool})` : ""}`}
      status={SCOPE_HELP[grant.scope]}
      stacked
    >
      <div className="flex flex-wrap items-center gap-3">
        <Select
          label={`${grant.name} access`}
          value={grant.scope}
          options={SCOPE_OPTIONS}
          onChange={(scope) => void updateOutsideApp(grant.id, { scope: scope as OutsideScope }, host)}
        />
        {grant.scope !== "read_only" && (
          <span className="flex items-center gap-2 text-callout text-fg-muted">
            Ask me first
            <Switch
              label={`Ask before ${grant.name} changes anything`}
              checked={grant.ask}
              onChange={(ask) => void updateOutsideApp(grant.id, { ask }, host)}
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
              run: () => void revokeOutsideApp(grant.id, grant.name, host),
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
      title="Connect an app on this computer"
      hint="Claude Desktop, Cursor, Codex or a script: pick it and what it may do. RustCode makes a token and the config to paste into that app; the token is shown once."
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
      <Row
        title="Create its token and config"
        subtitle={
          full ? "The most apps are connected; revoke one first." : "Shown once, to paste into that app."
        }
      >
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

/** "Signed in from the web until 17 Dec", for a hosted agent. */
function signedInLabel(until: number | null): string {
  if (!until) return "Signed in from the web";
  const date = new Date(until * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" });
  return `Signed in from the web until ${date}`;
}
