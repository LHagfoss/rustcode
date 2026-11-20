import { bool, type JsonObject, num, obj, str } from "../acp/jsonrpc";
import { RemoteMethod } from "../acp/remote";
import { shellCall, shellCallWithin } from "../lib/shell";
import { type Machine, PAIR_DEADLINE_MS, refreshMachines, type SshVia, useMachines } from "./machines";
import { errorText, request, toast } from "./store";

// ---------------------------------------------------------------- connect codes

/**
 * A connect code this computer shows for another computer to pair with it: no request and no
 * accept step, valid ten minutes, once (docs/remote.md › Connect codes). The engine creates it on
 * this computer's gateway; the other computer redeems it with `machines_connect_code` (or
 * `rc machines connect`).
 */
export interface ConnectOffer {
  /** The short form, `K7MQ-29XH-TP4W-NRD6`. */
  code: string;
  /** The long form, `rustcode://connect/…`: every address and the gateway's id. */
  link: string;
  /** The address the short code carries, or the one to type next to it. */
  address: string;
  addressInCode: boolean;
  hostName: string | null;
  loopbackOnly: boolean;
  serial: number;
  expiresAt: number;
  /** The computer that paired with it, once one has. */
  pairedWith: string | null;
  /** Its wrong tries ran out. */
  usedUp: boolean;
}

/** Read the engine's `connectCode` answer. */
export function parseConnectOffer(value: unknown, now = Date.now()): ConnectOffer | null {
  const offer = obj(value);
  const code = str(offer?.code);
  const link = str(offer?.link);
  if (!offer || !code || !link) return null;
  return {
    code,
    link,
    address: str(offer.address) ?? "",
    addressInCode: bool(offer.addressInCode) ?? false,
    hostName: str(offer.hostName) ?? null,
    loopbackOnly: bool(offer.loopbackOnly) ?? false,
    serial: num(offer.serial) ?? 0,
    expiresAt: now + (num(offer.expiresInSecs) ?? 600) * 1000,
    pairedWith: null,
    usedUp: false,
  };
}

/** Create a connect code on this computer. Remote access is switched on first if it was off. */
export async function createConnectCode(): Promise<void> {
  useMachines.setState({ connect: { status: "running" } });
  try {
    const answer = obj(await request<JsonObject>(RemoteMethod.connectCode, {}, 20_000));
    const offer = parseConnectOffer(answer?.connectCode);
    const error = str(answer?.connectCodeError);
    useMachines.setState({
      connectOffer: offer,
      connect: offer
        ? { status: "idle" }
        : { status: "failed", error: error ?? "The gateway did not hand out a connect code." },
    });
  } catch (error) {
    useMachines.setState({ connect: { status: "failed", error: errorText(error) } });
  }
}

/** Withdraw the open connect code, or take a spent one off the screen. */
export async function cancelConnectCode(): Promise<void> {
  const offer = useMachines.getState().connectOffer;
  useMachines.setState((state) => ({
    connectOffer: null,
    connect: state.connect.status === "running" ? state.connect : { status: "idle" },
  }));
  if (!offer || offer.pairedWith || offer.usedUp || offer.expiresAt <= Date.now()) return;
  try {
    await request(RemoteMethod.connectCode, { cancel: true }, 15_000);
  } catch (error) {
    toast(errorText(error), "error");
  }
}

/**
 * Follow the open connect code: the gateway says when a computer paired with it, or its tries ran
 * out. The page asks while the code is on screen.
 */
export async function followConnectCode(): Promise<void> {
  const offer = useMachines.getState().connectOffer;
  if (!offer || offer.pairedWith || offer.usedUp) return;
  try {
    const status = obj(await request<JsonObject>(RemoteMethod.status, {}, 10_000));
    const ended = obj(obj(obj(status?.gateway)?.machineOffer)?.lastEnded);
    if (!ended || num(ended.serial) !== offer.serial) return;
    const current = useMachines.getState().connectOffer;
    if (current?.serial !== offer.serial) return;
    useMachines.setState({
      connectOffer:
        str(ended.end) === "paired"
          ? { ...current, pairedWith: str(ended.deviceName) ?? "A computer" }
          : { ...current, usedUp: true },
    });
  } catch {
    // The next tick asks again; the countdown says when it no longer matters.
  }
}

/**
 * Pair with a connect code created on another computer. `address` is where to reach it when the
 * code carries none.
 */
export async function connectWithCode(code: string, address?: string): Promise<boolean> {
  const typed = code.trim();
  if (!typed) {
    useMachines.setState({ connect: { status: "failed", error: "Paste or type the connect code first." } });
    return false;
  }
  useMachines.setState({ connect: { status: "running" } });
  try {
    const machine = await shellCallWithin<Machine>(PAIR_DEADLINE_MS, "machines_connect_code", {
      code: typed,
      address: address?.trim() || null,
      name: null,
    });
    useMachines.setState((state) => ({
      machines: [...state.machines.filter((saved) => saved.id !== machine.id), machine],
    }));
    toast(`Connected to ${machine.name}.`, "success");
    await refreshMachines();
    useMachines.setState({ connect: { status: "idle" } });
    return true;
  } catch (error) {
    useMachines.setState({ connect: { status: "failed", error: errorText(error) } });
    return false;
  }
}

/** One step of a connection over SSH (`rustcode_client::ssh::Progress`). */
export interface SshStep {
  step: "sign_in" | "check" | "install" | "start" | "pair" | "pair_back";
  state: "running" | "done" | "skipped" | "failed";
  label: string;
  detail?: string | null;
}

export interface SshRun {
  running: boolean;
  steps: SshStep[];
}

/** SSH deadline: a ten-minute install on the other computer, plus sign-in, start and pairing. */
export const SSH_DEADLINE_MS = 15 * 60_000;

/**
 * Connect over SSH: install RustCode on `target` at this version when allowed and needed, start
 * its server and pair with it. The steps are polled while it runs.
 */
export async function connectOverSsh(
  target: string,
  allowInstall: boolean,
  mutual = false,
  via: SshVia = "auto",
): Promise<boolean> {
  const typed = target.trim();
  if (!typed) {
    useMachines.setState({
      ssh: {
        status: "failed",
        error: "Type the host first, like lars@box or an alias from ~/.ssh/config.",
        data: useMachines.getState().ssh.data,
      },
    });
    return false;
  }
  useMachines.setState({ ssh: { status: "running", data: { steps: [] } } });
  const poll = setInterval(() => {
    void shellCall<SshRun>("machines_ssh_progress").then(
      (run) => {
        if (useMachines.getState().ssh.status === "running")
          useMachines.setState({ ssh: { status: "running", data: { steps: run.steps } } });
      },
      () => {},
    );
  }, 400);
  let failure: string | null = null;
  let notice: string | undefined;
  try {
    const machine = await shellCallWithin<Machine>(SSH_DEADLINE_MS, "machines_connect_ssh", {
      target: typed,
      port: null,
      identity: null,
      allowInstall,
      name: null,
      via,
    });
    useMachines.setState((state) => ({
      machines: [...state.machines.filter((saved) => saved.id !== machine.id), machine],
    }));
    if (mutual) {
      const failure = await pairBack(typed);
      if (failure) notice = pairBackNotice(machine.name, failure);
    }
    toast(`Connected to ${machine.name}.`, "success");
    await refreshMachines();
    return true;
  } catch (error) {
    failure = errorText(error);
    return false;
  } finally {
    clearInterval(poll);
    const last = await shellCall<SshRun>("machines_ssh_progress").catch(() => null);
    useMachines.setState((state) => ({
      ssh: {
        ...(failure ? { status: "failed" as const, error: failure } : { status: "idle" as const }),
        data: { steps: last?.steps ?? state.ssh.data?.steps ?? [], notice },
      },
    }));
  }
}

/**
 * The second direction of mutual access: create a connect code on this computer and have the
 * computer just connected redeem it over SSH. Returns why it did not work, or null. The one-way
 * pairing is saved either way, and a code that was not used is withdrawn.
 */
async function pairBack(target: string): Promise<string | null> {
  let link: string | null = null;
  try {
    const answer = obj(await request<JsonObject>(RemoteMethod.connectCode, {}, 20_000));
    const offer = parseConnectOffer(answer?.connectCode);
    if (!offer) return str(answer?.connectCodeError) ?? "This computer could not create a connect code.";
    if (offer.loopbackOnly) {
      return "Remote access on this computer listens here only (remote.bind), so no other computer can reach it. Set remote.bind to auto and try again.";
    }
    link = offer.link;
    await shellCallWithin<null>(PAIR_BACK_DEADLINE_MS, "machines_pair_back", {
      target,
      port: null,
      identity: null,
      link,
    });
    return null;
  } catch (error) {
    if (link) await request(RemoteMethod.connectCode, { cancel: true }, 15_000).catch(() => {});
    return errorText(error);
  }
}

/** How long the page waits for the other computer to redeem the code: the shell bounds it at two minutes. */
const PAIR_BACK_DEADLINE_MS = 3 * 60_000;

/** The notice for a connection that is one way because pairing back failed. */
export function pairBackNotice(name: string, reason: string): string {
  return `Connected to ${name}, one way: this computer reaches it, but it cannot reach this computer. ${reason.trim()}`;
}

/** Clear the steps of a finished connection over SSH. */
export function dismissSsh(): void {
  useMachines.setState({ ssh: { status: "idle" } });
}
