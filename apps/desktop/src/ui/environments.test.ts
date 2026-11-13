import { describe, expect, test } from "bun:test";
import type { Machine, SshStep } from "../state/machines";
import {
  addFailure,
  addressSteps,
  codeSteps,
  environmentAddress,
  environmentChoices,
  environmentIds,
  environmentOutdated,
  environmentRecovery,
  environmentRoute,
  environmentStatus,
  knownEnvironment,
  newEnvironment,
  recoveryLabel,
  sshProgress,
} from "./environments";

const LAN = { kind: "lan", address: "192.168.1.31:17879", encrypted: false };
const TAILNET = { kind: "tailscale", address: "100.64.0.7:17879", encrypted: true };

function machine(patch: Partial<Machine> = {}): Machine {
  return {
    id: "gw-1",
    name: "Studio",
    platform: "macos",
    version: "1.32.0",
    routes: [LAN],
    route: LAN,
    state: "connected",
    detail: null,
    hint: null,
    retryInSecs: null,
    addedAt: "2026-11-02T09:00:00+00:00",
    lastConnectedAt: null,
    ...patch,
  };
}

describe("environment status", () => {
  test("names the four states people tell apart", () => {
    expect(environmentStatus(machine())).toEqual({ kind: "online", label: "Online", tone: "ok" });
    expect(environmentStatus(machine({ state: "connecting" })).kind).toBe("connecting");
    expect(environmentStatus(machine({ state: "offline" }))).toEqual({
      kind: "offline",
      label: "Offline",
      tone: "warn",
    });
    expect(environmentStatus(machine({ state: "needsPairing" })).kind).toBe("attention");
  });

  test("an offline one on its retry clock is connecting, not failed", () => {
    expect(environmentStatus(machine({ state: "offline", retryInSecs: 8 }))).toEqual({
      kind: "connecting",
      label: "Reconnecting",
      tone: "idle",
    });
  });

  test("one that refused says why", () => {
    const status = environmentStatus(machine({ state: "unavailable", reason: "version_mismatch" }));
    expect(status.kind).toBe("attention");
    expect(status.label).toBe("Version mismatch");
  });
});

describe("environment route", () => {
  test("the address leads, then the network and whether it is encrypted", () => {
    expect(environmentRoute(machine())).toBe("192.168.1.31:17879 · LAN · Unencrypted");
    expect(environmentRoute(machine({ route: TAILNET, routes: [TAILNET] }))).toBe(
      "100.64.0.7:17879 · Tailscale · Encrypted",
    );
  });

  test("falls back to the first saved route, and says when there is none", () => {
    expect(environmentAddress(machine({ route: null }))).toBe("192.168.1.31:17879");
    expect(environmentRoute(machine({ route: null, routes: [] }))).toBe("No route saved");
    expect(environmentAddress(machine({ route: null, routes: [] }))).toBeNull();
  });
});

describe("recovery", () => {
  test("each state offers the one action that brings it back", () => {
    expect(environmentRecovery(machine())).toBeNull();
    expect(environmentRecovery(machine({ state: "connecting" }))).toBeNull();
    expect(environmentRecovery(machine({ state: "stopped" }))).toBe("connect");
    expect(environmentRecovery(machine({ state: "offline" }))).toBe("reconnect");
    expect(environmentRecovery(machine({ state: "unavailable" }))).toBe("reconnect");
    expect(environmentRecovery(machine({ state: "needsPairing" }))).toBe("pair");
    expect(recoveryLabel("pair")).toBe("Pair again");
    expect(recoveryLabel("reconnect")).toBe("Reconnect");
    expect(recoveryLabel("connect")).toBe("Connect");
  });
});

describe("the new-thread selector's choices", () => {
  const nameOf = (entry: Machine) => entry.displayName ?? entry.name;

  test("this computer first, then each environment", () => {
    const choices = environmentChoices(
      [machine(), machine({ id: "gw-2", name: "Box" })],
      { name: "My Mac" },
      nameOf,
    );
    expect(choices.map((choice) => choice.name)).toEqual(["My Mac", "Studio", "Box"]);
    expect(choices[0]).toMatchObject({ id: "", available: true, detail: "This computer" });
    expect(choices[1]).toMatchObject({ id: "gw-1", available: true });
    expect(choices[1]?.detail).toBe("192.168.1.31:17879 · LAN · Unencrypted");
  });

  test("one that is away is listed with why, and cannot be chosen", () => {
    const [, away, paused] = environmentChoices(
      [machine({ state: "offline" }), machine({ id: "gw-2", paused: true })],
      { name: "My Mac" },
      nameOf,
    );
    expect(away).toMatchObject({ available: false, detail: "Offline" });
    expect(paused).toMatchObject({ available: false, detail: "Paused for new threads" });
  });

  test("uses the name the user gave it", () => {
    const [, named] = environmentChoices(
      [machine({ displayName: "Office Mac" })],
      { name: "My Mac" },
      nameOf,
    );
    expect(named?.name).toBe("Office Mac");
  });
});

describe("add environment progress", () => {
  test("a connect code runs, then fails on its first step", () => {
    expect(codeSteps(true, false)[0]?.state).toBe("running");
    expect(codeSteps(false, true)[0]?.state).toBe("failed");
    expect(codeSteps(false, false).every((step) => step.state === "todo")).toBe(true);
  });

  test("an address moves through ask, accept, code and pair", () => {
    const states = (stage: "waiting" | "code" | "pairing" | null, failed = false) =>
      addressSteps(stage, failed, "Studio").map((step) => step.state);
    expect(states(null)).toEqual(["todo", "todo", "todo", "todo"]);
    expect(states("waiting")).toEqual(["done", "running", "todo", "todo"]);
    expect(states("code")).toEqual(["done", "done", "running", "todo"]);
    expect(states("pairing")).toEqual(["done", "done", "done", "running"]);
    expect(states("pairing", true)).toEqual(["done", "done", "done", "failed"]);
    expect(states(null, true)).toEqual(["failed", "todo", "todo", "todo"]);
    expect(addressSteps("waiting", false, "Studio")[1]?.label).toBe("Accept the request on Studio");
  });

  test("ssh lists every step while it runs and stops at the one that failed", () => {
    const steps: SshStep[] = [
      { step: "sign_in", state: "done", label: "Signed in" },
      { step: "check", state: "failed", label: "RustCode is missing", detail: "not found" },
    ];
    const running = sshProgress([steps[0]!], true);
    expect(running.map((step) => step.key)).toEqual(["sign_in", "check", "install", "start", "pair"]);
    expect(running[1]?.state).toBe("todo");
    const failed = sshProgress(steps, false);
    expect(failed.map((step) => step.state)).toEqual(["done", "failed"]);
    expect(failed[1]?.detail).toBeNull();
    expect(sshProgress([], false)).toEqual([]);
  });

  test("ssh lists the pair back step only for mutual access, or once it has run", () => {
    const first: SshStep[] = [{ step: "sign_in", state: "done", label: "Signed in" }];
    expect(sshProgress(first, true).map((step) => step.key)).not.toContain("pair_back");
    const mutual = sshProgress(first, true, true).map((step) => step.key);
    expect(mutual.at(-1)).toBe("pair_back");
    const ran: SshStep[] = [
      { step: "pair", state: "done", label: "Paired" },
      { step: "pair_back", state: "failed", label: "Letting box reach this computer", detail: "no route" },
    ];
    const failed = sshProgress(ran, false);
    expect(failed.map((step) => [step.key, step.state])).toEqual([
      ["pair", "done"],
      ["pair_back", "failed"],
    ]);
  });
});

describe("failures", () => {
  test("say what failed, the plain error and the next thing to try, per method", () => {
    const code = addFailure("code", "The code was refused.\n  Ask for a new one.");
    expect(code.title).toBe("Could not connect with that code");
    expect(code.detail).toBe("The code was refused. Ask for a new one.");
    expect(addFailure("ssh", "x").hint).toContain("ssh-add");
    expect(addFailure("address", "x").title).toBe("Could not reach that address");
  });
});

describe("a new environment", () => {
  test("is the one that was not there when the dialog opened", () => {
    const before = environmentIds([machine()]);
    expect(newEnvironment([machine()], before)).toBeNull();
    const fresh = machine({ id: "gw-2", name: "Box" });
    expect(newEnvironment([machine(), fresh], before)?.id).toBe("gw-2");
  });

  test("is none when several appeared at once", () => {
    const before = environmentIds([]);
    expect(newEnvironment([machine(), machine({ id: "gw-2" })], before)).toBeNull();
  });
});

describe("an outdated environment", () => {
  test("is older than this app; a newer, equal, unknown or development version is not", () => {
    expect(environmentOutdated("1.32.0", "1.33.0")).toBe(true);
    expect(environmentOutdated("1.33.0", "1.33.0")).toBe(false);
    expect(environmentOutdated("1.34.0", "1.33.0")).toBe(false);
    expect(environmentOutdated(null, "1.33.0")).toBe(false);
    expect(environmentOutdated("1.32.0", null)).toBe(false);
    // A `tauri dev` build reports 0.1.0, which would make every environment look old.
    expect(environmentOutdated("1.32.0", "0.1.0")).toBe(false);
  });
});

describe("an environment known through another", () => {
  test("shows what is known of it and who told us, and is outdated by the same rule", () => {
    const now = Date.parse("2026-11-13T13:00:00Z");
    const row = knownEnvironment(
      {
        hostId: "gw-pi",
        name: "Raspberry Pi",
        nameUpdatedAt: null,
        platform: "linux",
        version: "1.26.0",
        lastSeenAt: "2026-11-13T10:00:00Z",
        via: "gw-studio",
      },
      "Studio",
      "1.33.0",
      now,
    );
    expect(row.name).toBe("Raspberry Pi");
    expect(row.facts).toEqual(["Linux", "RustCode 1.26.0", "Last seen 3 hours ago"]);
    expect(row.via).toBe("Studio");
    expect(row.outdated).toBe(true);
  });

  test("says nothing it does not know", () => {
    const row = knownEnvironment(
      {
        hostId: "gw-x",
        name: "Box",
        nameUpdatedAt: null,
        platform: "unknown",
        version: null,
        lastSeenAt: null,
        via: null,
      },
      null,
      "1.33.0",
      Date.now(),
    );
    expect(row.facts).toEqual([]);
    expect(row.outdated).toBe(false);
  });
});
