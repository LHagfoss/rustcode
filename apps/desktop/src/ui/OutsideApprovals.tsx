// What outside apps put to the owner, answered from anywhere in the app
// (docs/mcp-outside-agents.md): a hosted agent's sign-in waiting on a
// machine's public page, and a connected app's call that waits for Allow.
// Every connected machine is asked: it says when something starts or stops
// waiting (`_rustcode/outside/changed`), and a slow poll catches what a
// dropped notification missed. One card per item in the window's
// bottom-right corner, like a device asking to connect.

import { useEffect, useMemo } from "react";
import {
  answerOutsideCall,
  forgetOutside,
  OUTSIDE_POLL_MS,
  refreshOutside,
  useOutside,
} from "../state/outside";
import { useScopeMachines } from "../state/settingsScope";
import { useApp } from "../state/store";
import { LOCAL_HOST } from "../state/threadKeys";
import { SignInCard } from "./OutsideSignIn";
import { Action } from "./settingsKit";

/** At most this many cards at once. */
const SHOWN = 3;

/** Without anything waiting, machines are asked rarely: they say when something starts. */
const IDLE_POLL_MS = 30_000;

export function OutsideApprovals() {
  const engine = useApp((s) => s.engine.phase);
  const machines = useScopeMachines();
  const byHost = useOutside((s) => s.byHost);
  // Settings › Outside agents shows the same items in place; the corner would repeat them.
  const onPage = useApp((s) => s.settings === "outside");
  const online = useMemo(() => machines.filter((m) => m.online), [machines]);
  const hosts = online.map((m) => m.host).join("\u0000");
  const waiting = Object.values(byHost).some((s) => s.pending.length > 0 || s.signIns.length > 0);

  useEffect(() => {
    if (engine !== "ready") return;
    const list = hosts.split("\u0000");
    for (const host of Object.keys(useOutside.getState().byHost)) {
      if (!list.includes(host)) forgetOutside(host);
    }
    const ask = () => {
      for (const host of list) void refreshOutside(host);
    };
    ask();
    const timer = setInterval(ask, waiting ? OUTSIDE_POLL_MS : IDLE_POLL_MS);
    return () => clearInterval(timer);
  }, [engine, hosts, waiting]);

  const names = new Map(online.map((m) => [m.host, m.name]));
  const cards = Object.entries(byHost).flatMap(([host, status]) => {
    const machine = host === LOCAL_HOST ? null : (names.get(host) ?? status.machineName);
    return [
      ...status.signIns.map((signIn) => ({ kind: "signIn" as const, host, machine, status, signIn })),
      ...status.pending.map((item) => ({ kind: "call" as const, host, machine, item })),
    ];
  });
  if (cards.length === 0 || onPage) return null;
  return (
    <div className="pointer-events-none fixed right-4 bottom-4 z-[80] flex w-[360px] flex-col gap-2">
      {cards.slice(0, SHOWN).map((card) =>
        card.kind === "signIn" ? (
          <SignInCard
            key={`${card.host}/${card.signIn.id}`}
            host={card.host}
            machine={card.machine}
            signIn={card.signIn}
            levels={card.status.levels}
            floating
          />
        ) : (
          <div
            key={`${card.host}/${card.item.id}`}
            role="alertdialog"
            aria-label={`${card.item.clientName} asks to run ${card.item.tool}`}
            className="elevated pointer-events-auto flex flex-col gap-2 rounded-2xl p-4 rc-in"
          >
            <div className="font-medium text-body text-fg">
              {card.item.clientName} wants to run {card.item.tool}
            </div>
            {card.machine && <div className="text-caption text-fg-faint">On {card.machine}</div>}
            <div className="selectable break-words text-callout text-fg-muted">{card.item.summary}</div>
            <div className="text-caption text-fg-faint">
              An outside app is not one of your threads, so their access, Full access included, does not cover
              it.
            </div>
            <div className="flex justify-end gap-2">
              <Action danger onClick={() => void answerOutsideCall(card.item.id, false, card.host)}>
                Decline
              </Action>
              <Action prominent onClick={() => void answerOutsideCall(card.item.id, true, card.host)}>
                Allow
              </Action>
            </div>
          </div>
        ),
      )}
    </div>
  );
}
