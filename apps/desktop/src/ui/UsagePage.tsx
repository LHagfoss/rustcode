// Settings › Usage: every subscription this machine is signed in to, with its
// plan windows (`_rustcode/usage/plans`), whatever thread is open. Claude's
// numbers come from Claude Code's sign-in in the macOS Keychain, so they are
// read only after you ask once (macOS may ask to allow it); the choice is kept.

import { RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { arr, type Json, num, obj, str } from "../acp/jsonrpc";
import { resetLabel, type UsageWindow } from "../acp/rustcode";
import { Method } from "../acp/types";
import { request, useApp } from "../state/store";
import { relativeAge } from "../state/thread";
import { PlanWindow } from "./ComposerUsage";
import { Pill, Spinner } from "./primitives";
import { Group, Row } from "./settingsKit";

const CLAUDE_KEY = "rustcode.claudeUsage";

export interface PlanEntry {
  id: string;
  name: string;
  via?: string;
  status: "ok" | "signedOut" | "notInstalled" | "unavailable" | "notAsked";
  plan?: string;
  error?: string;
  windows: (UsageWindow & { label: string })[];
}

export function parsePlans(result: Json | undefined): PlanEntry[] {
  return arr(obj(result)?.plans).flatMap((p) => {
    const o = obj(p);
    const id = str(o?.id);
    const status = str(o?.status);
    if (!o || !id || !status) return [];
    const windows = arr(o.windows).flatMap((w) => {
      const x = obj(w);
      const used = num(x?.usedPercent);
      if (used === undefined) return [];
      const resets = num(x?.resetsAt);
      return [
        {
          label: str(x?.label) ?? "Usage",
          usedPercent: used,
          minutes: num(x?.windowMinutes),
          resetsAt: resets ? resets * 1000 : undefined,
        },
      ];
    });
    return [
      {
        id,
        name: str(o.name) ?? id,
        via: str(o.via),
        status: status as PlanEntry["status"],
        plan: str(o.plan),
        error: str(o.error),
        windows,
      },
    ];
  });
}

function claudeAllowed(): boolean {
  try {
    return localStorage.getItem(CLAUDE_KEY) === "1";
  } catch {
    return false;
  }
}

export function UsagePage() {
  const status = useApp((s) => s.status);
  const [claude, setClaude] = useState(claudeAllowed);
  const [plans, setPlans] = useState<PlanEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [readAt, setReadAt] = useState<number | null>(null);
  const load = useCallback(async (withClaude: boolean) => {
    setLoading(true);
    try {
      setPlans(parsePlans(await request(Method.usagePlans, { claude: withClaude }, 20_000)));
      setReadAt(Date.now());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    if (status === "ready") void load(claude);
  }, [status, claude, load]);
  const allowClaude = () => {
    try {
      localStorage.setItem(CLAUDE_KEY, "1");
    } catch {
      // Asked again next time in a private window.
    }
    setClaude(true);
  };
  if (!plans) {
    return error ? (
      <p className="text-callout text-warning">{error}</p>
    ) : (
      <div className="flex items-center gap-2 text-callout text-fg-muted">
        <Spinner size={12} /> Reading your plans…
      </div>
    );
  }
  return (
    <>
      {plans.map((p) => (
        <Group
          key={p.id}
          title={
            <span>
              {p.name}
              {p.plan && <span className="ml-1.5 font-normal text-fg-faint capitalize">{p.plan}</span>}
            </span>
          }
          action={p.via && <span className="text-caption text-fg-faint">from {p.via}</span>}
        >
          {p.status === "ok" && p.windows.length > 0 ? (
            <div className="flex flex-col gap-4 px-3.5 py-3.5">
              {p.windows.map((w) => (
                <PlanWindow key={w.label} window={w} title={w.label} />
              ))}
            </div>
          ) : p.status === "notAsked" ? (
            <Row
              title="Show Claude plan usage"
              subtitle="Reads Claude Code's sign-in to ask Anthropic for your 5-hour and weekly windows. macOS may ask once to allow access to it in the Keychain. Only reads; never signs in or out."
            >
              <Pill prominent onClick={allowClaude}>
                Show
              </Pill>
            </Row>
          ) : (
            <Row
              title={
                p.status === "signedOut"
                  ? `Not signed in to ${p.via ?? p.name}`
                  : p.status === "notInstalled"
                    ? `${p.via ?? p.name} not found`
                    : p.status === "ok"
                      ? "No plan windows reported"
                      : "Could not read the usage"
              }
              subtitle={
                p.error ??
                (p.status === "signedOut"
                  ? `Sign in with ${p.via ?? p.name} on this Mac to see this plan here.`
                  : p.status === "notInstalled"
                    ? `Install ${p.via ?? p.name} and sign in to see this plan here.`
                    : undefined)
              }
            />
          )}
        </Group>
      ))}
      <div className="flex items-center gap-2 px-1 text-caption text-fg-faint">
        <span className="flex-1">
          {error ? (
            <span className="text-warning">Could not refresh: {error}</span>
          ) : readAt ? (
            `Read ${Date.now() - readAt < 60_000 ? "just now" : relativeAge(readAt)}. ${soonestReset(plans) ?? ""}`
          ) : null}
        </span>
        <Pill onClick={() => void load(claude)} disabled={loading}>
          {loading ? <Spinner size={11} /> : <RefreshCw size={12} />} Refresh
        </Pill>
      </div>
    </>
  );
}

function soonestReset(plans: PlanEntry[]): string | undefined {
  const next = plans
    .flatMap((p) => p.windows)
    .filter((w) => w.resetsAt)
    .sort((a, b) => a.resetsAt! - b.resetsAt!)[0];
  return next ? `The soonest window ${resetLabel(next)}.` : undefined;
}
