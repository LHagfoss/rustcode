// What Settings changes on which machine (docs/desktop.md › Settings on one
// machine or all): the registry's kinds, the Mixed rule, the write targets
// under All machines, and copying an MCP definition without its secrets.

import { describe, expect, test } from "bun:test";
import {
  failureSummary,
  mcpCopy,
  missingSettingValues,
  pageHasServerSettings,
  pageIsMachineOnly,
  rowKind,
  runOnMachines,
  type ScopedMachine,
  scopeBreakdown,
  scopedMachines,
  scopeValuesMixed,
  writableMachines,
} from "./settingsRegistry";

const LOCAL: ScopedMachine = { host: "", name: "This machine", online: true, local: true };
const LINUX: ScopedMachine = { host: "gw-linux", name: "Lars's Linux box", online: true, local: false };
const STUDIO: ScopedMachine = { host: "gw-studio", name: "Studio", online: false, local: false };

describe("setting kinds", () => {
  test("app preferences, server settings and machine-only rows are told apart", () => {
    expect(rowKind("appearance", "Theme")).toBe("app");
    expect(rowKind("keyboard", "Keyboard shortcuts")).toBe("app");
    expect(rowKind("general", "System notifications")).toBe("app");
    expect(rowKind("general", "Default access")).toBe("server");
    expect(rowKind("agents", "Harnesses")).toBe("server");
    expect(rowKind("capabilities", "MCP servers")).toBe("server");
    expect(rowKind("connections", "GitHub")).toBe("machine");
    expect(rowKind("general", "Default project folder")).toBe("machine");
    expect(rowKind("remote", "Gateway address")).toBe("machine");
  });

  test("only pages with server rows show the Applying to selector", () => {
    expect(pageHasServerSettings("general")).toBe(true);
    expect(pageHasServerSettings("agents")).toBe(true);
    expect(pageHasServerSettings("capabilities")).toBe(true);
    expect(pageHasServerSettings("appearance")).toBe(false);
    expect(pageHasServerSettings("chat")).toBe(false);
    expect(pageHasServerSettings("connections")).toBe(false);
    expect(pageIsMachineOnly("connections")).toBe(true);
    expect(pageIsMachineOnly("remote")).toBe(true);
    expect(pageIsMachineOnly("machines")).toBe(true);
    expect(pageIsMachineOnly("general")).toBe(false);
  });
});

describe("Mixed and All machines", () => {
  test("the connected machines disagreeing is Mixed", () => {
    expect(
      scopeValuesMixed([
        { ...LOCAL, value: "ask" },
        { ...LINUX, value: "edits" },
      ]),
    ).toBe(true);
    expect(
      scopeValuesMixed([
        { ...LOCAL, value: "ask" },
        { ...LINUX, value: "ask" },
      ]),
    ).toBe(false);
  });

  test("an offline machine keeps its value and is not part of Mixed", () => {
    const entries = [
      { ...LOCAL, value: true },
      { ...STUDIO, value: false },
      { ...LINUX, value: true },
    ];
    expect(scopeValuesMixed(entries)).toBe(false);
    const breakdown = scopeBreakdown(entries, (value) => (value ? "On" : "Off"));
    expect(breakdown.find((entry) => entry.host === STUDIO.host)?.label).toBe(
      "Off — offline, keeps its value",
    );
  });

  test("a value that has not arrived yet does not make a settled row Mixed", () => {
    expect(
      scopeValuesMixed([
        { ...LOCAL, value: "ask" },
        { ...LINUX, value: undefined },
      ]),
    ).toBe(false);
  });

  test("this machine, one machine, and all machines", () => {
    const machines = [LOCAL, LINUX, STUDIO];
    expect(scopedMachines({ mode: "this" }, machines)).toEqual([LOCAL]);
    expect(scopedMachines({ mode: "machine", host: LINUX.host }, machines)).toEqual([LINUX]);
    expect(scopedMachines({ mode: "all" }, machines)).toEqual(machines);
  });

  test("All machines writes only to the connected machines", () => {
    const machines = [LOCAL, LINUX, STUDIO];
    expect(writableMachines({ mode: "all" }, machines).map((machine) => machine.host)).toEqual([
      LOCAL.host,
      LINUX.host,
    ]);
    expect(writableMachines({ mode: "machine", host: STUDIO.host }, machines)).toEqual([]);
  });
});

describe("copying an MCP server", () => {
  test("keeps the definition and blanks every value", () => {
    const copy = mcpCopy({
      name: "github",
      transport: "stdio",
      copyTarget: ["npx", "-y", "github-mcp"],
      envKeys: ["GITHUB_TOKEN", "NODE_ENV"],
      headerKeys: ["Authorization"],
    });
    expect(copy).not.toBeNull();
    expect(copy?.target).toEqual(["npx", "-y", "github-mcp"]);
    expect(copy?.env).toEqual(["GITHUB_TOKEN=", "NODE_ENV="]);
    expect(copy?.headers).toEqual(["Authorization: "]);
    expect(copy?.secretKeys).toEqual(["GITHUB_TOKEN", "NODE_ENV", "Authorization"]);
    expect(copy?.env.every((value) => value.endsWith("="))).toBe(true);
  });

  test("a target with a masked secret is not copied", () => {
    expect(
      mcpCopy({
        name: "private",
        transport: "stdio",
        copyTarget: null,
        envKeys: [],
        headerKeys: [],
      }),
    ).toBeNull();
  });
});

describe("a fan-out", () => {
  test("reaches every machine and names the one that failed", async () => {
    const machines = [LOCAL, LINUX, STUDIO];
    const visited: string[] = [];
    const result = await runOnMachines(machines, async (machine) => {
      visited.push(machine.host);
      if (machine.host === LINUX.host) throw new Error("That machine is not connected.");
      return machine.host;
    });

    expect(visited).toEqual([LOCAL.host, LINUX.host, STUDIO.host]);
    expect(result.attempted).toBe(3);
    expect(result.succeeded.map((machine) => machine.host)).toEqual([LOCAL.host, STUDIO.host]);
    expect(result.failures).toEqual([
      { host: LINUX.host, name: LINUX.name, message: "That machine is not connected." },
    ]);
    expect(failureSummary(result)).toBe("Lars's Linux box: That machine is not connected.");
  });

  test("a later machine is still written after one fails", async () => {
    const writes: string[] = [];
    await runOnMachines([LOCAL, LINUX, STUDIO], async (machine) => {
      writes.push(machine.name);
      if (machine.host === LOCAL.host) throw new Error("old server");
    });
    expect(writes).toEqual(["This machine", "Lars's Linux box", "Studio"]);
  });
});

describe("seeding a machine that has no values yet", () => {
  test("only the keys the server does not already hold are written", () => {
    const missing = missingSettingValues(["default-permission-mode", "orchestration"], {
      "default-permission-mode": "full",
      orchestration: false,
      "resume-after-restart": true,
    });
    expect(missing).toEqual({ "resume-after-restart": true });
  });
});
