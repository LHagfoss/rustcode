/**
 * Fleet-wide jobs: choosing any machine in the paired network, resolving hub fallbacks, and
 * opening run threads on the machine that ran them.
 */

import { describe, expect, test } from "bun:test";
import { RpcError } from "../acp/jsonrpc";
import {
  isLoopsUnsupported,
  jobMachineTargets,
  loopKey,
  loopMachineLabel,
  remoteLoopError,
  runThreadKey,
  targetKeyOf,
} from "./fleetLoops";

const VPS = {
  id: "gw-vps",
  name: "lagosvps",
  displayName: "LagosVPS",
  state: "connected" as const,
};

const STUDIO = {
  id: "gw-studio",
  name: "studio",
  displayName: "Studio",
  state: "offline" as const,
};

describe("jobMachineTargets", () => {
  test("lists only this computer when there are no paired machines or hub members", () => {
    const targets = jobMachineTargets({
      localName: "Main Mac",
      machines: [],
    });
    expect(targets).toEqual([
      {
        key: "host:",
        host: "",
        label: "This computer",
        machineName: "Main Mac",
        online: true,
      },
    ]);
  });

  test("lets a non-hub machine pick connected paired machines directly without requiring a hub", () => {
    const targets = jobMachineTargets({
      localName: "Main Mac",
      localHostId: "gw-mac",
      machines: [VPS],
      localHub: {
        hub: false,
        member: { machine: "gw-vps", name: "lagosvps", access: "admin", ask: false },
        members: null,
      },
    });
    expect(targets.map((t) => ({ key: t.key, host: t.host, member: t.member, label: t.label }))).toEqual([
      { key: "host:", host: "", member: undefined, label: "Main Mac (this computer)" },
      { key: "host:gw-vps", host: "gw-vps", member: undefined, label: "LagosVPS" },
    ]);
  });

  test("includes unpaired hub members reported by a connected hub and deduplicates this computer", () => {
    const targets = jobMachineTargets({
      localName: "Main Mac",
      localHostId: "gw-mac",
      machines: [VPS],
      remoteHubMembers: {
        "gw-vps": [
          { hostId: "gw-mac", name: "Main Mac", online: true },
          { hostId: "gw-arch", name: "arch-pc", online: true },
          { hostId: "gw-pi", name: "home-pi", online: false },
        ],
      },
    });
    expect(targets.map((t) => ({ key: t.key, host: t.host, member: t.member, label: t.label }))).toEqual([
      { key: "host:", host: "", member: undefined, label: "Main Mac (this computer)" },
      { key: "host:gw-vps", host: "gw-vps", member: undefined, label: "LagosVPS" },
      { key: "hub:gw-vps:arch-pc", host: "gw-vps", member: "arch-pc", label: "arch-pc (via LagosVPS)" },
      {
        key: "hub:gw-vps:home-pi",
        host: "gw-vps",
        member: "home-pi",
        label: "home-pi (via LagosVPS, offline)",
      },
    ]);
  });

  test("falls back to a hub for an offline paired machine when a hub knows it, else marks it disabled", () => {
    const withHub = jobMachineTargets({
      localName: "Main Mac",
      machines: [VPS, STUDIO],
      remoteHubMembers: {
        "gw-vps": [{ hostId: "gw-studio", name: "studio", online: false }],
      },
    });
    const studioViaHub = withHub.find((t) => t.machineName === "Studio");
    expect(studioViaHub).toEqual({
      key: "hub:gw-vps:studio",
      host: "gw-vps",
      member: "studio",
      label: "Studio (offline · via LagosVPS)",
      machineName: "Studio",
      viaHubName: "LagosVPS",
      online: false,
    });

    const withoutHub = jobMachineTargets({
      localName: "Main Mac",
      machines: [STUDIO],
    });
    expect(withoutHub[1]).toEqual({
      key: "host:gw-studio",
      host: "gw-studio",
      label: "Studio (offline)",
      machineName: "Studio",
      online: false,
      disabled: true,
    });
  });

  test("marks an online paired machine needing an update when remote loops are unsupported", () => {
    const targets = jobMachineTargets({
      localName: "Main Mac",
      machines: [VPS],
      unsupportedHosts: new Set(["gw-vps"]),
    });
    expect(targets[1]).toEqual({
      key: "host:gw-vps",
      host: "gw-vps",
      label: "LagosVPS (update required)",
      machineName: "LagosVPS",
      online: true,
      disabled: false,
    });
  });

  test("preserves the current target when editing a job whose hub member is no longer listed", () => {
    const targets = jobMachineTargets({
      localName: "Main Mac",
      machines: [VPS],
      current: { host: "gw-vps", member: "old-box" },
    });
    expect(targets.at(-1)).toEqual({
      key: "hub:gw-vps:old-box",
      host: "gw-vps",
      member: "old-box",
      label: "old-box (not a member)",
      machineName: "old-box",
      viaHubName: "LagosVPS",
      online: false,
    });
  });
});

describe("fleet loop keys and labels", () => {
  test("keys jobs and targets uniquely across hosts", () => {
    expect(loopKey({ host: "", id: "l-1" })).toBe("l-1");
    expect(loopKey({ host: "gw-vps", id: "l-1" })).toBe("gw-vps/l-1");
    expect(targetKeyOf("")).toBe("host:");
    expect(targetKeyOf("gw-vps")).toBe("host:gw-vps");
    expect(targetKeyOf("gw-vps", " arch-pc ")).toBe("hub:gw-vps:arch-pc");
  });

  test("formats the machine name for direct and hub-dispatched jobs", () => {
    expect(loopMachineLabel({ host: "" }, [VPS], "Main Mac")).toBe("Main Mac");
    expect(loopMachineLabel({ host: "gw-vps" }, [VPS], "Main Mac")).toBe("LagosVPS");
    expect(loopMachineLabel({ host: "gw-vps", machine: "arch-pc" }, [VPS], "Main Mac")).toBe("arch-pc");
  });

  test("resolves run thread keys for local, paired, and hub-dispatched runs", () => {
    expect(runThreadKey({ host: "" }, { threadId: "t-1" }, [VPS], "Main Mac")).toBe("t-1");
    expect(runThreadKey({ host: "gw-vps" }, { threadId: "t-1" }, [VPS], "Main Mac")).toBe("gw-vps/t-1");
    expect(
      runThreadKey({ host: "gw-vps" }, { threadId: "t-1", machine: "Main Mac" }, [VPS], "Main Mac"),
    ).toBe("t-1");
    expect(runThreadKey({ host: "" }, { threadId: "t-1", machine: "LagosVPS" }, [VPS], "Main Mac")).toBe(
      "gw-vps/t-1",
    );
    expect(
      runThreadKey({ host: "gw-vps" }, { threadId: "t-1", machine: "arch-pc" }, [VPS], "Main Mac"),
    ).toBeUndefined();
  });

  test("turns a remote method-not-found error into an update prompt", () => {
    const err = new RpcError(-32601, "method is not permitted over remote ACP");
    expect(isLoopsUnsupported(err)).toBe(true);
    expect(remoteLoopError(err, "LagosVPS")).toBe(
      "Update RustCode on LagosVPS to manage its jobs from here.",
    );
    expect(remoteLoopError(new Error("boom"), "LagosVPS")).toBe("boom");
  });
});
