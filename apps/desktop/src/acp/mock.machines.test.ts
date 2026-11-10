// The mock's machines, which is what Settings › Machines reads under
// `VITE_TRANSPORT=mock`: the four `?machines=` states, and the ACP one saved
// machine answers in process (`MockMachine`), since the mock has no shell to
// dial a real machine through.

import { describe, expect, test } from "bun:test";
import type { Json, JsonObject } from "./jsonrpc";
import { MockMachine, type MockOptions, MockTransport } from "./mock";

async function machineList(option: MockOptions["machines"] = undefined): Promise<JsonObject[]> {
  const transport = new MockTransport({ machines: option });
  return (await transport.call("machines_list", {})) as unknown as JsonObject[];
}

async function call(transport: MockTransport, command: string, args: Record<string, unknown> = {}) {
  return (await transport.call(command, args)) as Json;
}

describe("mock machines", () => {
  test("the default is one paired machine, connected, with three threads", async () => {
    const machines = await machineList();
    expect(machines.length).toBe(1);
    const machine = machines[0]!;
    expect(machine.name).toBe("Lars's Linux box");
    expect(machine.platform).toBe("linux");
    expect(machine.state).toBe("connected");
    expect(machine.routes).toEqual([{ kind: "lan", address: "192.168.1.31:17879", encrypted: false }]);
    // The app opens its channel and reads `session/list`: three threads.
    await expect(call(new MockTransport(), "machine_connect", { id: machine.id })).resolves.toEqual({
      connected: true,
    });
    const machine2 = new MockMachine("Lars's Linux box");
    const answers = machine2.receive(JSON.stringify({ jsonrpc: "2.0", id: 2, method: "session/list" }));
    const answer = JSON.parse(answers[0]!) as JsonObject;
    expect((answer.result as JsonObject).sessions as JsonObject[]).toHaveLength(3);
  });

  test("?machines=offline shows a machine being retried, and opens no channel", async () => {
    const machines = await machineList("offline");
    expect(machines[0]!.state).toBe("offline");
    expect(machines[0]!.retryInSecs).toBe(4);
    const transport = new MockTransport({ machines: "offline" });
    await expect(call(transport, "machine_connect", { id: machines[0]!.id })).resolves.toEqual({
      connected: false,
    });
  });

  test("?machines=revoked shows a pairing the machine ended", async () => {
    const machines = await machineList("revoked");
    expect(machines[0]!.state).toBe("needsPairing");
    expect(String(machines[0]!.detail)).toContain("Pair it again");
  });

  test("?machines=paired saves the machine just now", async () => {
    const machines = await machineList("paired");
    expect(machines[0]!.state).toBe("connected");
    expect(Date.now() - Date.parse(String(machines[0]!.addedAt))).toBeLessThan(5_000);
  });

  test("?machines=connect starts with one saved machine on an older RustCode, its server not installed", async () => {
    const transport = new MockTransport({ machines: "connect" });
    const machines = await machineList("connect");
    expect(machines.map((machine) => machine.name)).toEqual(["Lars's Linux box"]);
    expect(machines[0]!.version).toBe("0.96.0");
    // The NetBird address its handshake reported, and why it does not answer.
    expect(machines[0]!.routes).toContainEqual({
      kind: "vpn",
      address: "100.90.16.237:17879",
      provider: "netbird",
      encrypted: true,
    });
    expect(String(machines[0]!.hint)).toContain("NetBird");
    // The background server is the other half of the state: nothing is installed.
    const server = (await call(transport, "server_service_status")) as JsonObject;
    expect(server.installed).toBe(false);
    expect(((await call(transport, "server_service_install")) as JsonObject).running).toBe(true);
  });

  test("asking a machine then typing its code saves it", async () => {
    const transport = new MockTransport({ machines: "connect" });
    const ask = (await call(transport, "machines_ask", {
      askId: "ask-mock-1",
      address: "192.168.1.20:17879",
    })) as JsonObject;
    expect(ask.codeLength).toBe(8);
    // The ask id is the page's, echoed back so one id names the request throughout.
    expect(ask.askId).toBe("ask-mock-1");
    const paired = (await call(transport, "machines_pair", {
      askId: ask.askId,
      code: "48219034",
    })) as JsonObject;
    expect(paired.name).toBe("Lars's Linux box");
    // The list the page reads next has it once, and the app can reach it.
    const saved = (await call(transport, "machines_list")) as JsonObject[];
    expect(saved.map((machine) => machine.name)).toEqual(["Lars's Linux box"]);
    await expect(call(transport, "machine_connect", { id: paired.id })).resolves.toEqual({ connected: true });
  });

  test("a short code is refused before it costs a pairing attempt on the machine", async () => {
    const transport = new MockTransport();
    await expect(call(transport, "machines_pair", { askId: "x", code: "4821" })).rejects.toThrow(
      "exactly 8 digits",
    );
  });

  test("a public address is refused, with the reason the client gives", async () => {
    const transport = new MockTransport();
    await expect(call(transport, "machines_ask", { address: "example.com:17879" })).rejects.toThrow(
      "no TLS listener",
    );
  });

  test("a LAN, overlay or tailnet address is not refused", async () => {
    const transport = new MockTransport();
    for (const address of ["192.168.1.20:17879", "10.0.0.4", "100.92.13.44:17879", "mac.tail1234.ts.net"]) {
      const ask = (await call(transport, "machines_ask", { address })) as JsonObject;
      expect(ask.codeLength).toBe(8);
    }
  });

  test("discovery offers a machine already saved, one that is not, and the saved one's overlay address", async () => {
    const transport = new MockTransport();
    const found = ((await call(transport, "machines_discover")) as JsonObject).rows as JsonObject[];
    // The page merges the two rows for one machine by host id (`mergeDiscovered`).
    expect(found.map((machine) => [machine.name, machine.saved, machine.source])).toEqual([
      ["Lars's Linux box", true, "bonjour"],
      ["Studio", false, "bonjour"],
      ["Lars's Linux box", true, "overlay"],
    ]);
    expect(found[1]!.address).toBe("192.168.1.20:17879");
    expect(found[2]!.address).toBe("100.90.16.237:17879");
    expect(found[2]!.routeKind).toBe("netbird");
  });

  test("removing a machine takes it out of the list, and main moves to the one asked for", async () => {
    const transport = new MockTransport();
    await call(transport, "machines_set_main", { id: "gw-7c1d9e4f2a8b6d30" });
    let saved = (await call(transport, "machines_list")) as JsonObject[];
    expect(saved[0]!.main).toBe(true);
    await call(transport, "machines_remove", { id: "gw-7c1d9e4f2a8b6d30" });
    saved = (await call(transport, "machines_list")) as JsonObject[];
    expect(saved).toEqual([]);
  });
});
