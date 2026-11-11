// A device row without a window: the glyph, the name, the state, and the words
// under it, including an offline machine's reason and its one action.

import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { Machine } from "../state/machines";
import { DeviceRow, MachineCard } from "./Devices";

const NOW = Date.parse("2026-11-08T12:00:00Z");

function machine(overrides: Partial<Machine> = {}): Machine {
  return {
    id: "gw-8e1f4b7a5c2d4e6f",
    name: "Studio",
    platform: "macos",
    version: "1.28.0",
    main: false,
    routes: [{ kind: "lan", address: "192.168.1.20:17879", encrypted: false }],
    route: { kind: "lan", address: "192.168.1.20:17879", encrypted: false },
    state: "connected",
    detail: null,
    hint: null,
    retryInSecs: null,
    addedAt: "2026-11-02T09:00:00Z",
    lastConnectedAt: "2026-11-08T09:00:00Z",
    ...overrides,
  };
}

function render(value: Machine, counts?: { running: number; total: number }): string {
  return renderToStaticMarkup(
    <DeviceRow machine={value} counts={counts} now={NOW} onOpen={() => undefined} />,
  );
}

describe("a device row", () => {
  test("says its platform, name, state, route once, and how many threads run", () => {
    const html = render(machine(), { running: 1, total: 3 });
    expect(html).toContain("macOS");
    expect(html).toContain("Studio");
    expect(html).toContain("Connected");
    // The route's name to the eye, its address on hover, and each once.
    const text = html.replace(/<[^>]*>/g, "");
    expect(text.split("LAN").length - 1).toBe(1);
    expect(html.split("192.168.1.20:17879").length - 1).toBe(1);
    expect(text).toContain("1 of 3 threads running");
    expect(html).not.toContain("Reconnect");
  });

  test("an offline row says why, when it last answered, and offers the action", () => {
    const html = render(
      machine({
        state: "offline",
        platform: "windows",
        lastConnectedAt: "2026-11-08T09:00:00Z",
        detail: "failed to connect: Connection timed out (os error 60)",
      }),
      { running: 0, total: 2 },
    );
    expect(html).toContain("Windows");
    expect(html).toContain("Offline");
    expect(html).toContain("last seen 3 hours ago");
    expect(html).toContain("No answer on port 17879, a firewall may block it.");
    expect(html).toContain("Reconnect");
  });

  test("a machine's card says what it runs and offers its three actions", () => {
    const html = renderToStaticMarkup(<MachineCard machine={machine()} now={NOW} />);
    expect(html).toContain("Studio");
    expect(html).toContain("Mac");
    expect(html).toContain("RustCode 1.28.0");
    expect(html).toContain("Connected");
    expect(html).toContain("LAN · 192.168.1.20:17879");
    expect(html).toContain("New thread");
    expect(html).toContain("Reconnect");
    expect(html).toContain("Machine settings");
  });

  test("a pairing the machine ended offers pairing again instead", () => {
    const html = render(machine({ state: "needsPairing" }));
    expect(html).toContain("Needs pairing");
    expect(html).toContain("That computer no longer accepts this pairing. Pair it again.");
    expect(html).toContain("Pair again");
    expect(html).not.toContain(">Reconnect<");
  });
});
