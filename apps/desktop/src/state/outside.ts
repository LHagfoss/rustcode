// What Settings › Outside agents and the approval cards show: each machine's
// answer to `_rustcode/outside/list`, asked again when that machine says
// something changed (`_rustcode/outside/changed`) and polled while a card or
// the page is open. The engine decides everything; this holds the last answer
// per machine and sends the owner's choices to the machine they are for.

import { create } from "zustand";
import {
  createParams,
  type OutsideCode,
  type OutsideCreated,
  OutsideMethod,
  type OutsidePlatform,
  type OutsideScope,
  type OutsideStatus,
  type PasskeyLink,
  parseCode,
  parseCreated,
  parseOutsideStatus,
  parsePasskeyLink,
} from "../acp/outside";
import { machineRequest } from "./settingsScope";
import { errorText, toast } from "./store";
import { LOCAL_HOST } from "./threadKeys";

interface OutsideState {
  /** Each machine's last answer, keyed by host (`""` is this computer). */
  byHost: Record<string, OutsideStatus>;
  errors: Record<string, string>;
}

export const useOutside = create<OutsideState>(() => ({ byHost: {}, errors: {} }));

/** How often an open page or card asks again: a call waits at most two minutes. */
export const OUTSIDE_POLL_MS = 3000;

const inFlight = new Map<string, Promise<OutsideStatus | null>>();

export function refreshOutside(host: string = LOCAL_HOST): Promise<OutsideStatus | null> {
  const running = inFlight.get(host);
  if (running) return running;
  const read = (async () => {
    try {
      const status = parseOutsideStatus(await machineRequest(host, OutsideMethod.list, {}, 10_000));
      useOutside.setState((s) => {
        const errors = { ...s.errors };
        delete errors[host];
        return { byHost: { ...s.byHost, [host]: status }, errors };
      });
      return status;
    } catch (error) {
      useOutside.setState((s) => ({ errors: { ...s.errors, [host]: errorText(error) } }));
      return useOutside.getState().byHost[host] ?? null;
    } finally {
      inFlight.delete(host);
    }
  })();
  inFlight.set(host, read);
  return read;
}

/** A machine went away: its cards go with it. */
export function forgetOutside(host: string) {
  useOutside.setState((s) => {
    if (!(host in s.byHost)) return s;
    const byHost = { ...s.byHost };
    delete byHost[host];
    return { byHost };
  });
}

/** Connect an app. The token and config come back once, in the answer. */
export async function connectOutsideApp(
  input: {
    name: string;
    platform: OutsidePlatform;
    scope: OutsideScope;
    ask: boolean;
  },
  host: string = LOCAL_HOST,
): Promise<OutsideCreated | null> {
  try {
    const created = parseCreated(
      await machineRequest(host, OutsideMethod.create, createParams(input), 15_000),
    );
    if (!created) throw new Error("RustCode did not return a token");
    await refreshOutside(host);
    return created;
  } catch (error) {
    toast(errorText(error), "error");
    return null;
  }
}

export async function updateOutsideApp(
  id: string,
  change: { scope?: OutsideScope; ask?: boolean },
  host: string = LOCAL_HOST,
) {
  try {
    await machineRequest(host, OutsideMethod.update, { id, ...change }, 10_000);
  } catch (error) {
    toast(errorText(error), "error");
  }
  await refreshOutside(host);
}

export async function revokeOutsideApp(id: string, name: string, host: string = LOCAL_HOST) {
  try {
    await machineRequest(host, OutsideMethod.revoke, { id }, 10_000);
    toast(`${name} can no longer connect.`, "success");
  } catch (error) {
    toast(errorText(error), "error");
  }
  await refreshOutside(host);
}

export async function answerOutsideCall(id: string, allow: boolean, host: string = LOCAL_HOST) {
  try {
    await machineRequest(host, OutsideMethod.answer, { id, allow }, 10_000);
  } catch (error) {
    toast(errorText(error), "error");
  }
  await refreshOutside(host);
}

/** Approve a hosted agent's sign-in at `scope`, or decline it. */
export async function answerOutsideSignIn(
  host: string,
  id: string,
  answer: { allow: false } | { allow: true; scope: OutsideScope; ask: boolean },
) {
  try {
    const result = await machineRequest<{ answered?: boolean }>(
      host,
      OutsideMethod.signIn,
      answer.allow ? { id, allow: true, scope: answer.scope, ask: answer.ask } : { id, allow: false },
      10_000,
    );
    if (result && result.answered === false) toast("That sign-in is no longer waiting.", "info");
  } catch (error) {
    toast(errorText(error), "error");
  }
  await refreshOutside(host);
}

/** A one-time code for the machine's sign-in page, at most at `scope`. */
export async function makeOutsideCode(
  host: string,
  scope: OutsideScope,
  ask: boolean,
): Promise<OutsideCode | null> {
  try {
    const code = parseCode(await machineRequest(host, OutsideMethod.code, { scope, ask }, 10_000));
    if (!code) throw new Error("RustCode did not return a code");
    return code;
  } catch (error) {
    toast(errorText(error), "error");
    return null;
  }
}

/** A link that adds an owner passkey on the machine's public sign-in page. */
export async function makePasskeyLink(host: string, name: string): Promise<PasskeyLink | null> {
  try {
    const link = parsePasskeyLink(
      await machineRequest(host, OutsideMethod.passkeyAdd, { name: name.trim() || "Passkey" }, 10_000),
    );
    if (!link) throw new Error("RustCode did not return a link");
    return link;
  } catch (error) {
    toast(errorText(error), "error");
    return null;
  }
}

/** Sign out a browser signed in to the web app; its connection closes. */
export async function signOutWebSession(host: string, id: string, name: string) {
  try {
    await machineRequest(host, OutsideMethod.webSignOut, { id }, 10_000);
    toast(`${name} is signed out.`, "success");
  } catch (error) {
    toast(errorText(error), "error");
  }
  await refreshOutside(host);
}

/** Remove an owner passkey; it stops approving sign-ins at once. */
export async function removePasskey(host: string, id: string, name: string) {
  try {
    await machineRequest(host, OutsideMethod.passkeyRemove, { id }, 10_000);
    toast(`The passkey ${name} no longer approves sign-ins.`, "success");
  } catch (error) {
    toast(errorText(error), "error");
  }
  await refreshOutside(host);
}
