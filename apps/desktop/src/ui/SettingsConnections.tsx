import { useState } from "react";
import { Method } from "../acp/types";
import { openExternal } from "../lib/native";
import {
  accountDetail,
  type Connections,
  type DeviceCode,
  type GitHubAccount,
  githubDetail,
  type ModelAccount,
  parseConnections,
  parseDeviceCode,
  providerLabel,
  providerMark,
} from "../state/connections";
import { useScope } from "../state/settingsScope";
import { askConfirm, errorText, request, toast } from "../state/store";
import { HarnessIcon } from "./HarnessIcon";
import { ExternalLink, Github, LogOut, RefreshCw } from "./icons";
import { CopyButton } from "./Markdown";
import { cx, Spinner, Tooltip } from "./primitives";
import { SettingsScopeBar } from "./SettingsScope";
import { Action, Chip, Group, Literal, Loading, Row, useEngineData, withCode } from "./settingsKit";

/** How long the page waits for someone to approve the GitHub code before it gives up. */
const GITHUB_WAIT_MS = 15 * 60_000;

type GitHubSignIn =
  | { phase: "idle" }
  | { phase: "starting" }
  | { phase: "code"; code: DeviceCode }
  | { phase: "failed"; message: string };

/**
 * Settings › Connections: the GitHub CLI's sign-ins and the saved model accounts on this
 * computer. GitHub signs in here with the device code `gh auth login --web` prints; model
 * accounts sign in with `/login` in a thread and sign out or refresh their models here.
 */
export function ConnectionsPage() {
  const { data, error, setData } = useEngineData(Method.connections, parseConnections);
  const { scope, machines } = useScope();
  const remote =
    scope.mode === "machine"
      ? machines.find((machine) => machine.host === scope.host && !machine.local)
      : undefined;
  if (remote)
    return (
      <>
        <SettingsScopeBar machineOnly />
        <Group title="Credentials and accounts">
          <Row
            title={`Sign in on ${remote.name} itself`}
            subtitle="Sign in on that computer. Credentials never leave their machine."
          />
        </Group>
      </>
    );
  if (!data)
    return (
      <>
        <SettingsScopeBar machineOnly />
        <Loading error={error} />
      </>
    );
  return (
    <>
      <SettingsScopeBar machineOnly />
      <GitHubGroup data={data} onChange={setData} />
      <ModelAccountsGroup accounts={data.accounts} onChange={setData} />
    </>
  );
}

function Avatar({ children, dim }: { children: React.ReactNode; dim?: boolean }) {
  return (
    <span
      aria-hidden
      className={cx(
        "flex size-8 shrink-0 items-center justify-center rounded-full bg-fill text-fg",
        dim && "text-fg-faint",
      )}
    >
      {children}
    </span>
  );
}

function AccountTitle({ avatar, name }: { avatar: React.ReactNode; name: string }) {
  return (
    <span className="flex min-w-0 items-center gap-3">
      {avatar}
      <span className="min-w-0 truncate">{name}</span>
    </span>
  );
}

function GitHubGroup({ data, onChange }: { data: Connections; onChange: (next: Connections) => void }) {
  const github = data.github;
  const [signIn, setSignIn] = useState<GitHubSignIn>(() =>
    github?.pending ? { phase: "code", code: github.pending } : { phase: "idle" },
  );
  const [signingOut, setSigningOut] = useState<string | null>(null);
  const busy = signIn.phase === "starting" || signIn.phase === "code";

  const start = async () => {
    setSignIn({ phase: "starting" });
    try {
      const code = parseDeviceCode(await request(Method.githubLogin, {}, 30_000));
      if (!code) throw new Error("GitHub did not give a sign-in code.");
      setSignIn({ phase: "code", code });
      const next = parseConnections(await request(Method.githubWait, {}, GITHUB_WAIT_MS));
      onChange(next);
      setSignIn({ phase: "idle" });
      const login = next.github?.accounts.find((a) => a.active)?.login;
      toast(login ? `Signed in to GitHub as ${login}` : "Signed in to GitHub", "success");
    } catch (e) {
      setSignIn((now) => (now.phase === "idle" ? now : { phase: "failed", message: errorText(e) }));
    }
  };
  const cancel = () => {
    setSignIn({ phase: "idle" });
    void request(Method.githubCancel, {}).catch(() => undefined);
  };
  const signOut = (account: GitHubAccount) =>
    askConfirm({
      title: `Sign ${account.login} out of GitHub?`,
      message:
        "The GitHub CLI forgets this account on this computer. Pull requests stop working until you sign in again.",
      action: "Sign out",
      destructive: true,
      run: () => {
        const key = `${account.host}/${account.login}`;
        setSigningOut(key);
        void request(Method.githubLogout, { host: account.host, login: account.login }, 30_000)
          .then((v) => {
            onChange(parseConnections(v));
            toast(`Signed ${account.login} out of GitHub`, "success");
          })
          .catch((e) => toast(`Could not sign out: ${errorText(e)}`, "error"))
          .finally(() => setSigningOut(null));
      },
    });

  const accounts = github?.accounts ?? [];
  return (
    <Group
      title="GitHub"
      footer="Pull requests, checks and merges use this sign-in, through the GitHub CLI."
      action={
        github?.cliInstalled && accounts.length > 0 && !busy ? (
          <Action size="sm" onClick={() => void start()}>
            Add account
          </Action>
        ) : undefined
      }
    >
      {!github?.cliInstalled ? (
        <Row
          title={
            <AccountTitle
              avatar={
                <Avatar dim>
                  <Github size={16} />
                </Avatar>
              }
              name="GitHub CLI not installed"
            />
          }
          subtitle="Install it, then come back here to sign in."
          status={
            github?.installHint ? (
              <span className="flex items-center gap-1">
                <Literal>{github.installHint}</Literal>
                <CopyButton text={github.installHint} iconOnly label="Copy install command" />
              </span>
            ) : undefined
          }
        >
          <Action onClick={() => openExternal("https://cli.github.com")}>
            <ExternalLink size={12} />
            cli.github.com
          </Action>
        </Row>
      ) : (
        <>
          {accounts.map((a) => {
            const key = `${a.host}/${a.login}`;
            return (
              <Row
                key={`${key}:${a.active}:${a.valid}`}
                title={
                  <AccountTitle
                    avatar={
                      <Avatar>
                        <Github size={16} />
                      </Avatar>
                    }
                    name={a.login}
                  />
                }
                subtitle={<span className="pl-11">{githubDetail(a)}</span>}
                badge={
                  a.valid ? (
                    <Chip tone="ok">{a.active ? "Signed in" : "Usable"}</Chip>
                  ) : (
                    <Chip tone="warn">Token invalid</Chip>
                  )
                }
              >
                {!a.valid && (
                  <Action size="sm" disabled={busy} onClick={() => void start()}>
                    Sign in again
                  </Action>
                )}
                <Tooltip label="Sign out">
                  <Action
                    size="sm"
                    quiet
                    aria-label={`Sign ${a.login} out`}
                    disabled={signingOut === key}
                    onClick={() => signOut(a)}
                  >
                    {signingOut === key ? <Spinner size={12} /> : <LogOut size={13} />}
                  </Action>
                </Tooltip>
              </Row>
            );
          })}
          {accounts.length === 0 && signIn.phase !== "code" && (
            <Row
              title={
                <AccountTitle
                  avatar={
                    <Avatar dim>
                      <Github size={16} />
                    </Avatar>
                  }
                  name="Not signed in"
                />
              }
              subtitle={<span className="pl-11">Sign in so threads can open and merge pull requests.</span>}
            >
              <Action prominent disabled={signIn.phase === "starting"} onClick={() => void start()}>
                {signIn.phase === "starting" ? <Spinner size={12} /> : <Github size={13} />}
                Sign in with GitHub
              </Action>
            </Row>
          )}
          {signIn.phase === "code" && <DeviceCodePanel code={signIn.code} onCancel={cancel} />}
          {signIn.phase === "failed" && (
            <Row title="Sign-in did not finish" subtitle={signIn.message}>
              <Action size="sm" onClick={() => void start()}>
                Try again
              </Action>
            </Row>
          )}
        </>
      )}
    </Group>
  );
}

/** The one-time code, large enough to read across, with the two things to do with it. */
function DeviceCodePanel({ code, onCancel }: { code: DeviceCode; onCancel: () => void }) {
  return (
    <div className="rc-expand flex flex-col gap-3 px-4 py-4" data-testid="github-device-code">
      <div className="text-callout text-fg-muted">Enter this code on GitHub to finish signing in.</div>
      <div className="flex flex-wrap items-center gap-3">
        <span className="selectable rounded-lg bg-fill px-3 py-1.5 font-mono font-semibold text-fg text-title tracking-[0.2em]">
          {code.code}
        </span>
        <CopyButton text={code.code} label="Copy code" size="md" />
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Action
          prominent
          onClick={() => {
            void navigator.clipboard?.writeText(code.code).catch(() => undefined);
            openExternal(code.url);
          }}
        >
          <ExternalLink size={12} />
          Copy code and open GitHub
        </Action>
        <Action quiet onClick={onCancel}>
          Cancel
        </Action>
        <span className="ml-auto flex items-center gap-2 text-caption text-fg-muted">
          <Spinner size={12} />
          Waiting for GitHub…
        </span>
      </div>
    </div>
  );
}

function ModelAccountsGroup({
  accounts,
  onChange,
}: {
  accounts: ModelAccount[];
  onChange: (next: Connections) => void;
}) {
  const [working, setWorking] = useState<string | null>(null);
  const keyOf = (a: ModelAccount) => `${a.provider}/${a.account}`;
  const run = async (a: ModelAccount, method: string, done: string) => {
    setWorking(keyOf(a));
    try {
      onChange(parseConnections(await request(method, { provider: a.provider, account: a.account }, 60_000)));
      toast(done, "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setWorking(null);
    }
  };
  const signOut = (a: ModelAccount) =>
    askConfirm({
      title: `Sign ${a.display || providerLabel(a.provider)} out?`,
      message: "Threads on this account stop until it signs in again with /login.",
      action: "Sign out",
      destructive: true,
      run: () => void run(a, Method.accountLogout, `Signed ${a.display || providerLabel(a.provider)} out`),
    });
  return (
    <Group title="Model accounts" footer={withCode("Add an account with `/login` in any thread.")}>
      {accounts.length === 0 && (
        <Row
          title={
            <AccountTitle
              avatar={
                <Avatar dim>
                  <RefreshCw size={14} />
                </Avatar>
              }
              name="No model accounts yet"
            />
          }
          subtitle={<span className="pl-11">{withCode("Type `/login` in a thread to add one.")}</span>}
        />
      )}
      {accounts.map((a) => {
        const mark = providerMark(a.provider);
        const key = keyOf(a);
        return (
          <Row
            key={`${key}:${a.active}:${a.needsSignIn}`}
            title={
              <AccountTitle
                avatar={
                  <Avatar>
                    {mark ? (
                      <HarnessIcon id={mark} size={16} />
                    ) : a.provider === "github_copilot" ? (
                      <Github size={16} />
                    ) : (
                      <span className="font-semibold text-caption uppercase">
                        {providerLabel(a.provider).slice(0, 2)}
                      </span>
                    )}
                  </Avatar>
                }
                name={a.display || providerLabel(a.provider)}
              />
            }
            subtitle={<span className="pl-11">{accountDetail(a)}</span>}
            badge={
              a.needsSignIn ? (
                <Chip tone="warn">Sign in again</Chip>
              ) : a.active ? (
                <Chip tone="ok">Active</Chip>
              ) : (
                <Chip>Idle</Chip>
              )
            }
          >
            <Tooltip label="Refresh its models">
              <Action
                size="sm"
                quiet
                aria-label={`Refresh models for ${a.display || a.provider}`}
                disabled={working === key || a.needsSignIn}
                onClick={() => void run(a, Method.accountRefresh, "Models refreshed")}
              >
                {working === key ? <Spinner size={12} /> : <RefreshCw size={13} />}
              </Action>
            </Tooltip>
            <Tooltip label="Sign out">
              <Action
                size="sm"
                quiet
                aria-label={`Sign ${a.display || a.provider} out`}
                disabled={working === key}
                onClick={() => signOut(a)}
              >
                <LogOut size={13} />
              </Action>
            </Tooltip>
          </Row>
        );
      })}
    </Group>
  );
}
