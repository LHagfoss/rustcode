import { describe, expect, test } from "bun:test";
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

function render(machine: Machine): string {
  return renderToStaticMarkup(<MachineRow machine={machine} count={2} now={Date.now()} local="1.29.1" />);
}

describe("a machine row", () => {
  test("says an unreachable route once", () => {
    const html = render(MACHINE);
    expect(html.split("Not reachable over the VPN").length - 1).toBe(1);
    expect(html).toContain("LAN · Unencrypted");
  });
});
