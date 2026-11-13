// What Settings › Outside agents and the approval cards show: the engine's
// answer to `_rustcode/outside/list`, polled while a card or the page is open.
// The engine decides everything; this holds the last answer and sends the
// owner's choices.

import { create } from "zustand";
import {
  createParams,
  type OutsideCreated,
  OutsideMethod,
  type OutsidePlatform,
  type OutsideScope,
  type OutsideStatus,
  parseCreated,
  parseOutsideStatus,
} from "../acp/outside";
import { errorText, request, toast } from "./store";

interface OutsideState {
  status: OutsideStatus | null;
  error: string | null;
}

export const useOutside = create<OutsideState>(() => ({ status: null, error: null }));

/** How often an open page or card asks again: a call waits at most two minutes. */
export const OUTSIDE_POLL_MS = 3000;

export async function refreshOutside(): Promise<OutsideStatus | null> {
  try {
    const status = parseOutsideStatus(await request(OutsideMethod.list, {}, 10_000));
    useOutside.setState({ status, error: null });
    return status;
  } catch (error) {
    useOutside.setState({ error: errorText(error) });
    return useOutside.getState().status;
  }
}

/** Connect an app. The token and config come back once, in the answer. */
export async function connectOutsideApp(input: {
  name: string;
  platform: OutsidePlatform;
  scope: OutsideScope;
  ask: boolean;
}): Promise<OutsideCreated | null> {
  try {
    const created = parseCreated(await request(OutsideMethod.create, createParams(input), 15_000));
    if (!created) throw new Error("RustCode did not return a token");
    await refreshOutside();
    return created;
  } catch (error) {
    toast(errorText(error), "error");
    return null;
  }
}

export async function updateOutsideApp(id: string, change: { scope?: OutsideScope; ask?: boolean }) {
  try {
    await request(OutsideMethod.update, { id, ...change }, 10_000);
  } catch (error) {
    toast(errorText(error), "error");
  }
  await refreshOutside();
}

export async function revokeOutsideApp(id: string, name: string) {
  try {
    await request(OutsideMethod.revoke, { id }, 10_000);
    toast(`${name} can no longer connect.`, "success");
  } catch (error) {
    toast(errorText(error), "error");
  }
  await refreshOutside();
}

export async function answerOutsideCall(id: string, allow: boolean) {
  try {
    await request(OutsideMethod.answer, { id, allow }, 10_000);
  } catch (error) {
    toast(errorText(error), "error");
  }
  await refreshOutside();
}
