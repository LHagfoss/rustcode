import { create } from "zustand";
import { type JsonObject, obj, str } from "../acp/jsonrpc";
import { RemoteMethod } from "../acp/remote";
import { Method } from "../acp/types";
import { parseConnectOffer } from "./machineConnect";
import { type Machine, machineName } from "./machines";
import { refreshRemote } from "./remoteAccess";
import { errorText, request, toast } from "./store";
import { connectionFor } from "./threadMachines";

/** How long a just-paired machine gets to come online before pairing back gives up. */
const CONNECT_WAIT_MS = 20_000;
/** `rc machines connect` there: it dials this computer's gateway and stores the token. */
const EXEC_SECS = 60;

interface AccessState {
  /** New pairings go both ways unless the person chose one way in Add machine. */
  bothWays: boolean;
  /** Machine id → pairing back in progress, or why it last failed. */
  byMachine: Record<string, { busy: boolean; error?: string }>;
}

export const useMachineAccess = create<AccessState>(() => ({ bothWays: true, byMachine: {} }));

export function setBothWays(bothWays: boolean): void {
  useMachineAccess.setState({ bothWays });
}

function put(id: string, entry: { busy: boolean; error?: string }) {
  useMachineAccess.setState((s) => ({ byMachine: { ...s.byMachine, [id]: entry } }));
}

async function waitForConnection(id: string) {
  const until = Date.now() + CONNECT_WAIT_MS;
  for (;;) {
    const connection = connectionFor(id);
    if (connection || Date.now() > until) return connection;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

/** The plain reason a failed `rc machines connect` gives, from its output. */
function connectFailure(name: string, output: string): string {
  if (/unrecognized subcommand/i.test(output))
    return `RustCode on ${name} is too old to connect back. Update it there, then try again.`;
  const last = output.trim().split("\n").filter(Boolean).pop();
  return last
    ? `${name} could not connect back: ${last}`
    : `${name} could not connect back to this computer.`;
}

/**
 * Let `machine` reach this computer: a fresh single-use connect code here, redeemed there over the
 * connection this computer already has. Returns why it did not work, or null.
 */
export async function allowMachineBack(
  machine: Pick<Machine, "id" | "name" | "displayName">,
): Promise<string | null> {
  const name = machineName(machine);
  if (useMachineAccess.getState().byMachine[machine.id]?.busy) return null;
  put(machine.id, { busy: true });
  let link: string | null = null;
  let failure: string | null = null;
  try {
    const connection = await waitForConnection(machine.id);
    if (!connection) throw new Error(`${name} is not connected. Connect to it first, then try again.`);
    const answer = obj(await request<JsonObject>(RemoteMethod.connectCode, {}, 20_000));
    const offer = parseConnectOffer(answer?.connectCode);
    if (!offer)
      throw new Error(str(answer?.connectCodeError) ?? "This computer could not create a connect code.");
    if (offer.loopbackOnly)
      throw new Error(
        "Remote access on this computer listens here only (remote.bind), so no other computer can reach it. Set remote.bind to auto and try again.",
      );
    link = offer.link;
    const result = obj(
      await connection.request(
        Method.machineExec,
        { command: `rc machines connect "${link}"`, timeoutSecs: EXEC_SECS },
        (EXEC_SECS + 15) * 1000,
      ),
    );
    if (result?.success !== true) throw new Error(connectFailure(name, str(result?.output) ?? ""));
    link = null;
    await refreshRemote().catch(() => null);
  } catch (error) {
    failure = errorText(error);
  } finally {
    if (link) await request(RemoteMethod.connectCode, { cancel: true }, 15_000).catch(() => {});
  }
  put(machine.id, failure ? { busy: false, error: failure } : { busy: false });
  return failure;
}

/** After a pairing by code or by address: pair back when "Both ways" is chosen, and say how it went. */
export async function afterPaired(machine: Pick<Machine, "id" | "name" | "displayName">): Promise<void> {
  if (!useMachineAccess.getState().bothWays) return;
  const failure = await allowMachineBack(machine);
  const name = machineName(machine);
  if (failure) toast(`Connected to ${name}, one way only. ${failure}`, "error");
  else toast(`${name} can reach this computer too.`, "success");
}
