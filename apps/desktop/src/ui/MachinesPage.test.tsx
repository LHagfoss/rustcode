import { describe, expect, test } from "bun:test";
import { Tooltip } from "radix-ui";
import { renderToStaticMarkup } from "react-dom/server";
import type { Machine } from "../state/machines";
import { MachineRow } from "./MachinesPage";

const HINT =
  "Not reachable over the VPN ([fdfd::1]:17879). On that machine, check that the VPN allows inbound connections.";

const MACHINE: Machine = {
  id: "gw-7c1d9e4f2a8b6d30",
  name: "Studio",
  platform: "macos",
  version: "1.29.1",
  routes: [{ kind: "lan", address: "192.168.1.31:17879", encrypted: false }],
  route: { kind: "lan", address: "192.168.1.31:17879", encrypted: false },
  state: "connected",
  detail: null,
  hint: HINT,
  retryInSecs: null,
  addedAt: "2026-11-02T09:00:00+00:00",
  lastConnectedAt: "2026-11-08T09:00:00+00:00",
};

function render(machine: Machine, local = "1.29.1"): string {
  return renderToStaticMarkup(
    <Tooltip.Provider>
      <MachineRow machine={machine} count={2} now={Date.now()} local={local} />
    </Tooltip.Provider>,
  );
}

describe("a machine row", () => {
  test("keeps route diagnostics in settings", () => {
    const html = render(MACHINE);
    expect(html).not.toContain(HINT);
    expect(html).toContain("LAN · Unencrypted");
  });

  test("an online one shows its status, address, version and actions", () => {
    const html = render(MACHINE);
    expect(html).toContain("Online");
    expect(html).toContain('data-status="online"');
    expect(html).toContain("192.168.1.31:17879 · LAN · Unencrypted");
    expect(html).toContain("RustCode 1.29.1");
    expect(html).toContain("2 threads");
    expect(html).not.toContain("Rename");
    expect(html).toContain("Settings");
    expect(html).not.toContain("Remove");
    expect(html).not.toContain("Reconnect");
  });

  test("an offline one offers Reconnect and still says where it is tried", () => {
    const html = render({ ...MACHINE, state: "offline", detail: "No route answered.", hint: null });
    expect(html).toContain("Offline");
    expect(html).toContain("Reconnect");
    expect(html).not.toContain("No route answered.");
    expect(html).toContain("192.168.1.31:17879 · LAN · Unencrypted");
  });

  test("one that reconnects by itself still offers a manual retry", () => {
    const html = render({ ...MACHINE, state: "offline", retryInSecs: 4, hint: null });
    expect(html).toContain("Reconnecting");
    expect(html).toContain("Retry now</button>");
    expect(html).not.toContain("Reconnect</button>");
  });

  test("an outdated one says how to update it there", () => {
    const html = render(
      { ...MACHINE, platform: "linux", version: "1.44.0", state: "offline", hint: null },
      "1.45.0",
    );
    expect(html).toContain("Outdated");
    expect(html).toContain("<code");
    expect(html).toContain("rc --update");
  });

  test("route latency shows only while connected", () => {
    const probe = { latencyMs: 0 } as NonNullable<NonNullable<Machine["route"]>["probe"]>;
    const route = { ...MACHINE.route!, probe };
    expect(render({ ...MACHINE, route })).toContain("Route latency 0 ms");
    expect(render({ ...MACHINE, route, state: "offline", hint: null })).not.toContain("Route latency");
  });

  test("one that ended the pairing offers Pair again", () => {
    const html = render({ ...MACHINE, state: "needsPairing", hint: null });
    expect(html).toContain("Needs pairing");
    expect(html).toContain("Pair again");
  });

  test("one that was never connected this run offers Connect", () => {
    const html = render({ ...MACHINE, state: "stopped", hint: null });
    expect(html).toContain("Not connected");
    expect(html).toContain("Connect");
  });
});
