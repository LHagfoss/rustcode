// Harness cards and the default-harness form, shared by first-run
// onboarding and Settings › Agents (docs/harnesses.md). RustCode never
// signs in for a harness: "Sign in" shows the harness's own login command.

import { Check, ChevronRight, Minus, Plus, RefreshCw, SquareTerminal, X } from "lucide-react";
import { Popover } from "radix-ui";
import { useEffect, useMemo, useState } from "react";
import { effortLabel, type Harness, type HarnessAccount, type HarnessSnapshot } from "../acp/types";
import { canOpenTerminal, openInTerminal } from "../lib/native";
import {
  accountLoginCommand,
  accountSignIn,
  capabilityRows,
  commandLine,
  harnessStatus,
  normalizeDefault,
  orderHarnesses,
  selectOption,
  unavailableReason,
  validateDefault,
} from "../state/harness";
import { accountToSignIn } from "../state/logins";
import {
  accountSlug,
  addHarnessAccount,
  askConfirm,
  checkHarnessAccounts,
  ensureHarnessModels,
  fetchHarnessModels,
  type NewThreadChoice,
  refreshHarnesses,
  removeHarness,
  removeHarnessAccount,
  setHarnessEnabled,
  useApp,
} from "../state/store";
import { HarnessIcon } from "./HarnessIcon";
import { CopyButton } from "./Markdown";
import { cx, Skeleton, Spinner, Switch, Tooltip } from "./primitives";
import { Action, Select, type SelectOption } from "./settingsKit";

function StatusDot({ tone }: { tone: "ok" | "warn" | "off" }) {
  return (
    <span
      className={cx(
        "inline-block size-[7px] shrink-0 rounded-full",
        tone === "ok" ? "bg-success" : tone === "warn" ? "bg-warning" : "bg-fg-faint",
      )}
    />
  );
}

function SignIn({ harness }: { harness: Harness }) {
  const command = harness.login_command;
  if (!command?.length) return null;
  const line = commandLine(command);
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <Action prominent={harness.auth === "needs_login"} disabled={!harness.installed}>
          Sign in
        </Action>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          className="elevated z-[60] flex w-[340px] flex-col gap-2.5 p-3 rc-in"
        >
          <p className="text-callout text-fg-muted">
            {harness.name} signs in with its own login. Run this in a terminal; RustCode never sees the
            credentials.
          </p>
          <div className="flex items-center gap-2 rounded-md bg-fill-subtle py-1 pr-1 pl-2.5">
            <code className="selectable flex-1 truncate font-mono text-callout">{line}</code>
            <CopyButton text={line} />
          </div>
          {canOpenTerminal() && (
            <Action onClick={() => void openInTerminal(command)} className="self-start">
              <SquareTerminal size={13} />
              Open in Terminal
            </Action>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/**
 * Whether the account is the harness's own login (`~/.claude`, `~/.codex`),
 * which runs with no `CLAUDE_CONFIG_DIR`/`CODEX_HOME` at all: macOS keys
 * Claude Code's Keychain item on the variable, so setting one would look signed
 * out. The engine decides it on the resolved path, so an account written as an
 * absolute path is still the own login.
 */
export function isOwnLoginDir(account: Pick<HarnessAccount, "own_dir">): boolean {
  return account.own_dir === true;
}

/** What the engine last read about an account's sign-in. */
function accountStatus(account: HarnessAccount): string {
  if (account.auth === "ok") return "Signed in";
  // A look found no credentials file, Keychain item or status command behind it.
  if (account.auth === "needs_login") return "Not signed in";
  // Not looked for yet: only a Refresh probes.
  return "Not checked";
}

/** Signs the account in: the harness's own command, under the account's own directory. */
function signAccountIn(
  harness: Pick<Harness, "account_env" | "account_login_command">,
  account: Pick<HarnessAccount, "config_dir" | "own_dir">,
): Promise<boolean> {
  const signIn = accountSignIn(harness);
  if (!signIn) return Promise.resolve(false);
  return openInTerminal(signIn.command, isOwnLoginDir(account) ? [] : [[signIn.env, account.config_dir]]);
}

/** How long an added account's sign-in is waited for, three seconds at a time. */
const SIGN_IN_WAIT_MS = 5 * 60_000;

/**
 * A harness's named logins: each its own config directory, added and removed
 * by hand (`docs/harnesses.md`). RustCode never reads or copies their tokens;
 * "Sign in" is the harness's own login command, run in a terminal.
 */
export function AccountsList({ harness }: { harness: Harness }) {
  const accounts = harness.accounts ?? [];
  const discovered = harness.discovered_accounts ?? [];
  // Threads the list already knows about: no request, just a count for the
  // remove dialog.
  const threads = useApp((s) => s.sessions);
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  // The account whose sign-in the app is waiting for, and one it gave up on.
  const [waiting, setWaiting] = useState<string | null>(null);
  const [gaveUp, setGaveUp] = useState<string | null>(null);
  // One real look when the section opens: an account reads `unknown` until
  // the Keychain is asked. It is the existence probe only: forcing a plan read
  // here would clear the pauses a denied Keychain prompt earned and ask again.
  useEffect(() => {
    void checkHarnessAccounts(false);
  }, []);
  // The waiting row asks again every three seconds, for at most five minutes
  // while this section is mounted.
  useEffect(() => {
    if (!waiting) return;
    const deadline = Date.now() + SIGN_IN_WAIT_MS;
    const timer = setInterval(() => {
      if (Date.now() > deadline) {
        setWaiting(null);
        setGaveUp(waiting);
        return;
      }
      void checkHarnessAccounts(false).then((snapshot) => {
        const found = snapshot?.harnesses
          .find((h) => h.id === harness.id)
          ?.accounts?.find((a) => a.id === waiting);
        if (found?.auth === "ok") {
          setWaiting(null);
          setGaveUp(null);
        }
      });
    }, 3000);
    return () => clearInterval(timer);
  }, [waiting, harness.id]);
  const signIn = async (account: HarnessAccount) => {
    setGaveUp(null);
    await signAccountIn(harness, account);
    setWaiting(account.id);
  };
  const refresh = async () => {
    setBusy(true);
    await checkHarnessAccounts();
    setBusy(false);
  };
  const add = async () => {
    const id = accountSlug(name);
    if (!id) return;
    setBusy(true);
    const snapshot = await addHarnessAccount(harness.id, id, name.trim());
    setBusy(false);
    if (!snapshot) return;
    setAdding(false);
    setName("");
    const added = snapshot.harnesses.find((h) => h.id === harness.id)?.accounts?.find((a) => a.id === id);
    if (!added) return;
    if (canOpenTerminal()) await signIn(added);
    else setGaveUp(id);
  };
  const addFound = async (configDir: string, suggested: string) => {
    const id = accountSlug(suggested) || accountSlug(configDir.replace(/^.*[./]/, "")) || "account";
    setBusy(true);
    await addHarnessAccount(harness.id, id, suggested, configDir);
    // `accounts/add` answers without probing, so its row alone reads `unknown`:
    // ask what the account's sign-in is before opening a Terminal for it.
    const checked = await checkHarnessAccounts(false);
    setBusy(false);
    const added = accountToSignIn(checked, harness.id, configDir);
    if (added && canOpenTerminal()) await signIn(added);
  };
  const remove = (account: HarnessAccount) => {
    const used = threads.filter(
      (thread) => thread.harness === harness.id && thread.account === account.id,
    ).length;
    askConfirm({
      title: `Remove the account “${account.name}”?`,
      message:
        `RustCode forgets the account. Its directory, ${account.config_dir}, stays on disk with the sign-in and conversations in it. ` +
        `Threads on this account stay readable, but they need another account to continue.` +
        (used ? ` ${used} thread${used === 1 ? "" : "s"} use${used === 1 ? "s" : ""} it now.` : ""),
      action: "Remove account",
      destructive: true,
      run: () => void removeHarnessAccount(harness.id, account.id),
    });
  };
  return (
    <div className="ml-11 flex flex-col gap-2 rounded-lg border border-border/70 px-3 py-2.5">
      <div className="flex items-center gap-2">
        <span className="text-caption text-fg-muted">Accounts</span>
        <span className="flex-1" />
        <Action size="sm" quiet onClick={() => void refresh()} disabled={busy}>
          {busy ? <Spinner size={11} /> : <RefreshCw size={12} />} Refresh
        </Action>
      </div>
      {accounts.length ? (
        <ul className="flex flex-col gap-2">
          {accounts.map((account) => (
            <li key={account.id} className="flex flex-col gap-1">
              <div className="flex items-center gap-2">
                <span className="min-w-0 flex-1 truncate text-body">{account.name}</span>
                {waiting === account.id ? (
                  <span className="flex shrink-0 items-center gap-1 text-caption text-fg-muted">
                    <Spinner size={10} />
                    Waiting for sign-in…
                  </span>
                ) : (
                  <span
                    className={cx(
                      "shrink-0 text-caption",
                      account.auth === "ok"
                        ? "text-success"
                        : account.auth === "needs_login" || gaveUp === account.id
                          ? "text-warning"
                          : "text-fg-faint",
                    )}
                  >
                    {gaveUp === account.id ? "Not signed in" : accountStatus(account)}
                  </span>
                )}
                {account.auth !== "ok" && waiting !== account.id && (
                  <Tooltip label={canOpenTerminal() ? undefined : "Only the app can open Terminal"}>
                    <span>
                      <Action
                        size="sm"
                        disabled={!canOpenTerminal() || busy}
                        onClick={() => void signIn(account)}
                      >
                        Sign in
                      </Action>
                    </span>
                  </Tooltip>
                )}
                <Tooltip label="Remove from RustCode; the directory stays">
                  <button
                    type="button"
                    aria-label={`Remove the ${account.name} account`}
                    onClick={() => remove(account)}
                    className="text-fg-faint hover:text-danger"
                  >
                    <X size={12} />
                  </button>
                </Tooltip>
              </div>
              {account.auth !== "ok" && (
                <div className="flex items-center gap-2 rounded-md bg-fill-subtle py-1 pr-1 pl-2.5">
                  <code className="selectable flex-1 truncate font-mono text-caption text-fg-muted">
                    {accountLoginCommand(harness, account)}
                  </code>
                  <CopyButton text={accountLoginCommand(harness, account)} />
                </div>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-caption text-fg-muted">
          Runs on {harness.name}'s own sign-in. Add an account to choose between subscriptions.
        </p>
      )}
      {adding ? (
        <div className="flex items-center gap-1.5">
          <input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void add();
              if (e.key === "Escape") setAdding(false);
            }}
            placeholder="Name, e.g. Work"
            aria-label="Account name"
            className="h-7 min-w-0 flex-1 rounded-md border border-border bg-transparent px-2 text-callout outline-none focus:border-accent"
          />
          <Action size="sm" onClick={() => void add()} disabled={busy || !accountSlug(name)}>
            Add and sign in
          </Action>
          <Action size="sm" quiet onClick={() => setAdding(false)}>
            Cancel
          </Action>
        </div>
      ) : (
        <button
          type="button"
          onClick={() => setAdding(true)}
          className="flex items-center gap-1 self-start text-caption text-accent hover:underline"
        >
          <Plus size={11} />
          Add account
        </button>
      )}
      {discovered.length > 0 && (
        <div className="flex flex-col gap-1.5 border-border/70 border-t pt-2">
          <span className="text-caption text-fg-muted">Found on this Mac</span>
          {discovered.map((found) => (
            <div key={found.config_dir} className="flex items-center gap-2">
              <span className="min-w-0 flex-1 truncate font-mono text-caption text-fg-muted">
                {found.config_dir}
              </span>
              <span className="shrink-0 text-caption text-fg-faint">{found.suggested_name}</span>
              <Action
                size="sm"
                quiet
                disabled={busy}
                onClick={() => void addFound(found.config_dir, found.suggested_name)}
              >
                Add
              </Action>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** The card's check: lists the models, which is also the cheapest proof the harness runs. */
function useCheck(harness: Harness) {
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const check = async () => {
    setChecking(true);
    setResult(null);
    try {
      await fetchHarnessModels(harness.id);
      const models = useApp.getState().harnesses?.harnesses.find((h) => h.id === harness.id)?.models ?? [];
      setResult({
        ok: true,
        text: models.length ? `Works · ${models.length} model${models.length === 1 ? "" : "s"}` : "Works",
      });
    } catch (error) {
      setResult({ ok: false, text: error instanceof Error ? error.message : String(error) });
    } finally {
      setChecking(false);
    }
  };
  /** Detects the harness again: its install or sign-in may have changed outside the app. */
  const recheck = async () => {
    setChecking(true);
    setResult(null);
    try {
      await refreshHarnesses();
    } finally {
      setChecking(false);
    }
  };
  return { checking, result, check, recheck };
}

export function HarnessCard({ harness }: { harness: Harness }) {
  const { checking, result, check, recheck } = useCheck(harness);
  const [open, setOpen] = useState(false);
  const status = harnessStatus(harness);
  const local = harness.builtin;
  // RustCode turns off only once another harness starts new threads: the engine refuses otherwise.
  const isDefault = useApp((s) => s.harnesses?.default.harness === harness.id);
  const pinned = local && isDefault && harness.enabled;
  const custom = !local && !["codex", "claude-code", "gemini", "opencode", "pi"].includes(harness.id);
  // Sign in stays out of the way once the harness is known to be signed in.
  // A harness with named accounts signs in per account, in the Accounts list;
  // its own sign-in button would be the wrong one.
  const signIn =
    !local && harness.installed && harness.auth !== "ok" && !result?.ok && !harness.accounts?.length;
  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      <div className="flex items-center gap-3">
        <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-fill">
          <HarnessIcon id={harness.id} name={harness.name} size={17} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="font-semibold text-body">{harness.name}</span>
            {local && (
              <span className="rounded-full bg-fill px-1.5 text-caption text-fg-muted leading-[16px]">
                Local
              </span>
            )}
          </div>
          <div className="flex items-center gap-1.5 text-caption text-fg-muted">
            <StatusDot tone={status.tone} />
            <span className="truncate">
              {local ? `${status.label} · for the TUI, testing and local models` : status.label}
            </span>
          </div>
        </div>
        {signIn && <SignIn harness={harness} />}
        {harness.installed && harness.enabled && (
          <Action onClick={() => void check()} disabled={checking || !!unavailableReason(harness)}>
            {checking && <Spinner size={10} />}
            Check
          </Action>
        )}
        <Tooltip
          label={
            !harness.installed
              ? "Install it first"
              : pinned
                ? "Choose another default harness first"
                : undefined
          }
        >
          <span>
            <Switch
              label={`Use ${harness.name}`}
              checked={harness.enabled}
              disabled={!harness.installed || pinned}
              onChange={(v) =>
                // Turning a harness on checks it at once, so its models are ready to pick.
                void setHarnessEnabled(harness.id, v).then((snap) => {
                  if (v && snap) void check();
                })
              }
            />
          </span>
        </Tooltip>
      </div>
      {!harness.installed && harness.install_hint && (
        <div className="ml-11 flex items-center gap-2 rounded-md bg-fill-subtle py-1 pr-1 pl-2.5">
          <span className="text-caption text-fg-muted">Install</span>
          <code className="selectable flex-1 truncate font-mono text-callout">{harness.install_hint}</code>
          <CopyButton text={harness.install_hint} />
        </div>
      )}
      {accountSignIn(harness) && <AccountsList harness={harness} />}
      {(result || harness.error) && (
        <div
          className={cx(
            "selectable ml-11 flex items-start gap-1.5 text-callout",
            result?.ok ? "text-success" : "text-warning",
          )}
        >
          {result?.ok && <Check size={13} className="mt-[3px] shrink-0" />}
          <span className="min-w-0 flex-1">{result ? result.text : harness.error}</span>
          {!result?.ok && (
            <Action size="sm" quiet disabled={checking} onClick={() => void recheck()}>
              {checking ? <Spinner size={11} /> : <RefreshCw size={12} />} Check again
            </Action>
          )}
        </div>
      )}
      <div className="ml-11">
        <button
          type="button"
          onClick={() => setOpen(!open)}
          aria-expanded={open}
          className="flex items-center gap-1 text-caption text-fg-muted hover:text-fg"
        >
          <ChevronRight size={11} className={cx("transition-transform", open && "rotate-90")} />
          Details
        </button>
        {open && (
          <div className="mt-2 flex flex-col gap-2.5">
            {!local && harness.command.length > 0 && (
              <div className="flex items-center gap-2 text-caption">
                <span className="text-fg-muted">Runs</span>
                <code className="selectable truncate font-mono text-fg-muted">
                  {commandLine(harness.command)}
                </code>
              </div>
            )}
            <ul className="grid grid-cols-2 gap-x-4 gap-y-1">
              {capabilityRows(harness.capabilities).map((row) => (
                <li
                  key={row.key}
                  className={cx("flex items-start gap-1.5 text-caption", !row.available && "text-fg-faint")}
                >
                  {row.available ? (
                    <Check size={11} className="mt-px text-success" />
                  ) : (
                    <Minus size={11} className="mt-px" />
                  )}
                  <span>
                    {row.label}
                    {row.detail && <span className="block text-fg-faint">{row.detail}</span>}
                  </span>
                </li>
              ))}
              {!local && (
                <li className="col-span-2 text-caption text-fg-faint">
                  Runs with its own tools and permission rules; RustCode's sandbox does not apply.
                </li>
              )}
            </ul>
            {custom && (
              <button
                type="button"
                onClick={() => void removeHarness(harness.id)}
                className="self-start text-caption text-danger hover:underline"
              >
                Remove
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

/** Where the cards will be: each one's icon, name and status line, and its switch. */
export function HarnessListSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <div role="status" aria-label="Loading" className="divide-y divide-border">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-center gap-3 px-4 py-3">
          <Skeleton className="size-8 shrink-0 rounded-lg" />
          <div className="flex min-w-0 flex-1 flex-col gap-1.5">
            <Skeleton className="h-3" style={{ width: `${[34, 46, 30][i % 3]}%` }} />
            <Skeleton className="h-2 w-2/5" />
          </div>
          <Skeleton className="h-5 w-9 shrink-0 rounded-full" />
        </div>
      ))}
    </div>
  );
}

export function HarnessList({ snapshot }: { snapshot: HarnessSnapshot }) {
  return (
    <div className="divide-y divide-border">
      {orderHarnesses(snapshot.harnesses).map((h) => (
        <HarnessCard key={h.id} harness={h} />
      ))}
    </div>
  );
}

function SelectRow({
  label,
  ...select
}: {
  label: string;
  value: string;
  options: SelectOption[];
  onChange: (v: string) => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex items-center justify-between gap-4 px-4 py-3">
      <span className="text-body">{label}</span>
      <Select label={label} {...select} />
    </div>
  );
}

/** Default harness, model and effort for new threads. */
export function DefaultsForm({
  snapshot,
  value,
  onChange,
}: {
  snapshot: HarnessSnapshot;
  value: NewThreadChoice;
  onChange: (v: NewThreadChoice) => void;
}) {
  const threads = useApp((s) => s.threads);
  const harness = snapshot.harnesses.find((h) => h.id === value.harness);
  const [listing, setListing] = useState(false);
  // An external harness lists its models once asked; ask as soon as it is picked.
  useEffect(() => {
    if (!harness || harness.builtin || harness.models.length || !harness.installed) return;
    let live = true;
    setListing(true);
    void ensureHarnessModels(harness.id).finally(() => live && setListing(false));
    return () => {
      live = false;
    };
  }, [harness]);
  // The built-in harness lists its models through a thread's config options.
  const known = useMemo(
    () => Object.values(threads).find((t) => t.configOptions.length)?.configOptions ?? [],
    [threads],
  );
  const models = harness?.models.length
    ? harness.models.map((m) => ({ value: m.id, label: m.name, efforts: m.efforts }))
    : harness?.builtin
      ? (selectOption(known, "model")?.choices ?? []).map((c) => ({
          value: c.value,
          label: c.name,
          efforts: selectOption(known, "effort")?.choices.map((e) => e.value) ?? [],
        }))
      : [];
  const efforts = models.find((m) => m.value === value.model)?.efforts ?? models[0]?.efforts ?? [];
  const problem = validateDefault(snapshot, value);
  const set = (next: Partial<NewThreadChoice>) => {
    const merged = { ...value, ...next };
    onChange(harness?.models.length || next.harness ? normalizeDefault(snapshot, merged) : merged);
  };
  return (
    <div className="flex flex-col">
      <div className="divide-y divide-border">
        <SelectRow
          label="Harness"
          value={value.harness}
          onChange={(harnessId) => set({ harness: harnessId, model: null, effort: null })}
          options={orderHarnesses(snapshot.harnesses).map((h) => {
            const reason = unavailableReason(h);
            const name = h.builtin ? `${h.name} (local)` : h.name;
            return { value: h.id, label: reason ? `${name} — ${reason}` : name, disabled: !!reason };
          })}
        />
        <SelectRow
          label="Model"
          value={value.model ?? ""}
          disabled={!models.length}
          onChange={(model) => set({ model: model || null })}
          options={[
            {
              value: "",
              label: models.length
                ? "Harness default"
                : listing
                  ? "Loading models…"
                  : harness?.builtin
                    ? "Open a thread to list models"
                    : "Harness default",
            },
            ...models.map((m) => ({ value: m.value, label: m.label })),
          ]}
        />
        <div className="flex items-center justify-between gap-4 px-4 py-3">
          <span className="text-body">Effort</span>
          {harness && !harness.capabilities.effort ? (
            <span className="text-callout text-fg-faint">{harness.name} has no effort setting</span>
          ) : efforts.length ? (
            <Select
              label="Effort"
              value={value.effort ?? ""}
              onChange={(effort) => set({ effort: effort || null })}
              options={[
                ...(value.effort ? [] : [{ value: "", label: "Default" }]),
                ...efforts.map((e) => ({ value: e, label: effortLabel(e) })),
              ]}
            />
          ) : (
            <span className="text-callout text-fg-faint">Model default</span>
          )}
        </div>
      </div>
      {problem && <p className="mt-2 px-3 text-caption text-warning sm:px-4">{problem}</p>}
    </div>
  );
}
