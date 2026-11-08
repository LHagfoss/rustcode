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

  test("?machines=connect starts with none, and asking then pairing saves one", async () => {
    const transport = new MockTransport({ machines: "connect" });
    expect(await machineList("connect")).toEqual([]);
    const ask = (await call(transport, "machines_ask", { address: "192.168.1.20:17879" })) as JsonObject;
    expect(ask.codeLength).toBe(8);
    expect(typeof ask.requestId).toBe("string");
    const paired = (await call(transport, "machines_pair", {
      requestId: ask.requestId,
      code: "48219034",
    })) as JsonObject;
    expect(paired.name).toBe("Lars's Linux box");
    // The list the page reads next has it, and the app can reach it.
    expect(await machineList("connect")).toEqual([]);
    const saved = (await call(transport, "machines_list")) as JsonObject[];
    expect(saved.map((machine) => machine.name)).toEqual(["Lars's Linux box"]);
    await expect(call(transport, "machine_connect", { id: paired.id })).resolves.toEqual({ connected: true });
  });

  test("a short code is refused before it costs a pairing attempt on the machine", async () => {
    const transport = new MockTransport();
    await expect(call(transport, "machines_pair", { requestId: "x", code: "4821" })).rejects.toThrow(
      "exactly 8 digits",
    );
  });

  test("a public address is refused, with the reason the client gives", async () => {
    const transport = new MockTransport();
    await expect(call(transport, "machines_ask", { address: "example.com:17879" })).rejects.toThrow(
      "no TLS listener",
    );
  });

  test("discovery offers a machine already saved and one that is not", async () => {
    const transport = new MockTransport();
    const found = (await call(transport, "machines_discover")) as JsonObject[];
    expect(found.map((machine) => [machine.name, machine.saved])).toEqual([
      ["Lars's Linux box", true],
      ["Studio", false],
    ]);
    expect(found[1]!.address).toBe("192.168.1.20:17879");
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
