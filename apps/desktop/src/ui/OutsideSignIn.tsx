// A hosted agent (ChatGPT, Claude) asking to connect through a machine's
// public sign-in page (docs/mcp-outside-agents.md › Hosted agents). The page
// and this card show the same two words, so the owner approves the sign-in
// they started and not one somebody else started at the same time. The owner
// picks the level here; the page's pick is only the starting point.

import { useEffect, useState } from "react";
import {
  OUTSIDE_SCOPES,
  type OutsideLevel,
  type OutsideScope,
  type OutsideSignIn,
  SCOPE_LABEL,
} from "../acp/outside";
import { answerOutsideSignIn } from "../state/outside";
import { HarnessIcon } from "./HarnessIcon";
import { Check, X } from "./icons";
import { cx, Segments, Switch } from "./primitives";
import { Action } from "./settingsKit";

/** The mark a hosted agent's name points to, or none for its initial. */
export function agentMark(name: string): "openai" | "claude" | "gemini" | null {
  const key = name.toLowerCase();
  if (key.includes("chatgpt") || key.includes("openai")) return "openai";
  if (key.includes("claude") || key.includes("anthropic")) return "claude";
  if (key.includes("gemini")) return "gemini";
  return null;
}

/** The asking agent's mark in a rounded tile, or its initial. */
export function AgentTile({ name, size = 40 }: { name: string; size?: number }) {
  const mark = agentMark(name);
  return (
    <span
      aria-hidden
      style={{ width: size, height: size }}
      className={cx(
        "flex shrink-0 items-center justify-center rounded-xl font-semibold",
        mark ? "bg-fill text-fg" : "bg-accent text-accent-fg",
      )}
    >
      {mark ? (
        <HarnessIcon id={mark} size={Math.round(size * 0.55)} />
      ) : (
        <span style={{ fontSize: size * 0.45 }}>{name.trim().charAt(0).toUpperCase() || "?"}</span>
      )}
    </span>
  );
}

const SHORT_LABEL: Record<OutsideScope, string> = {
  read_only: "Read",
  delegate: "Threads",
  manage: "Manage",
  admin: "Full",
};

/** What `scope` allows and does not, as ticks and crosses. */
export function LevelPoints({ level }: { level: OutsideLevel | undefined }) {
  if (!level) return null;
  return (
    <ul className="flex flex-col gap-1 text-callout">
      {level.can.map((text) => (
        <li key={`y${text}`} className="flex items-start gap-2 text-fg-muted">
          <Check size={14} className="mt-[3px] shrink-0 text-success" />
          {text}
        </li>
      ))}
      {level.cannot.map((text) => (
        <li key={`n${text}`} className="flex items-start gap-2 text-fg-muted">
          <X size={14} className="mt-[3px] shrink-0 text-danger" />
          {text}
        </li>
      ))}
    </ul>
  );
}

export function SignInCard({
  host,
  machine,
  signIn,
  levels,
  floating = false,
}: {
  host: string;
  /** The machine's name, shown when the card is not on that machine's page. */
  machine: string | null;
  signIn: OutsideSignIn;
  levels: OutsideLevel[];
  /** In the window's corner, over whatever is open. */
  floating?: boolean;
}) {
  const [scope, setScope] = useState<OutsideScope>(signIn.scope);
  const [ask, setAsk] = useState(false);
  const [busy, setBusy] = useState(false);
  const [left, setLeft] = useState(signIn.expiresIn);
  useEffect(() => {
    setLeft(signIn.expiresIn);
    const timer = setInterval(() => setLeft((n) => Math.max(0, n - 1)), 1000);
    return () => clearInterval(timer);
  }, [signIn.expiresIn]);

  const answer = async (allow: boolean) => {
    setBusy(true);
    await answerOutsideSignIn(host, signIn.id, allow ? { allow: true, scope, ask } : { allow: false });
    setBusy(false);
  };
  const level = levels.find((l) => l.scope === scope);
  const minutes = Math.floor(left / 60);
  const seconds = String(left % 60).padStart(2, "0");

  return (
    <div
      role="alertdialog"
      aria-label={`${signIn.clientName} wants to connect`}
      className={cx(
        "flex flex-col gap-3 rounded-2xl p-4",
        floating ? "elevated pointer-events-auto rc-in" : "rounded-none",
      )}
    >
      <div className="flex items-center gap-3">
        <AgentTile name={signIn.clientName} />
        <span aria-hidden className="flex gap-1">
          <i className="size-1 rounded-full bg-fg-faint" />
          <i className="size-1 rounded-full bg-fg-faint" />
          <i className="size-1 rounded-full bg-fg-faint" />
        </span>
        <span className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-fill">
          <HarnessIcon id="rustcode" size={24} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate font-semibold text-body text-fg">{signIn.clientName} wants to connect</div>
          <div className="truncate text-caption text-fg-muted">
            {machine ? `To RustCode on ${machine}` : "To RustCode on this machine"}
            {signIn.destination ? ` · access goes to ${signIn.destination}` : ""}
          </div>
        </div>
      </div>
      <div className="flex items-center justify-between gap-3 rounded-xl bg-fill px-3 py-2">
        <span className="text-caption text-fg-muted">The sign-in page shows</span>
        <span className="selectable font-semibold text-body text-fg uppercase tracking-wide">
          {signIn.words}
        </span>
      </div>
      <Segments
        label="Access"
        fill
        value={scope}
        options={OUTSIDE_SCOPES.map((value) => ({ value, label: SHORT_LABEL[value] }))}
        onChange={setScope}
      />
      <div className="flex flex-col gap-1">
        <div className="text-caption text-fg-faint">
          {SCOPE_LABEL[scope]}
          {level?.summary ? `: ${level.summary}` : ""}
        </div>
        <LevelPoints level={level} />
      </div>
      {scope !== "read_only" && (
        <div className="flex items-center justify-between gap-3 text-callout text-fg-muted">
          Ask me before each change
          <Switch
            label={`Ask before ${signIn.clientName} changes anything`}
            checked={ask}
            onChange={setAsk}
          />
        </div>
      )}
      <div className="flex items-center gap-2">
        <span className="flex-1 text-caption text-fg-faint tabular-nums">
          {left > 0 ? `Waits ${minutes}:${seconds}` : "Expired"}
        </span>
        <Action danger disabled={busy} onClick={() => void answer(false)}>
          Decline
        </Action>
        <Action prominent disabled={busy || left === 0} onClick={() => void answer(true)}>
          Approve
        </Action>
      </div>
    </div>
  );
}
