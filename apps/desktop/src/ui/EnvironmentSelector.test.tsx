import { describe, expect, test } from "bun:test";
import { Tooltip } from "radix-ui";
import { renderToStaticMarkup } from "react-dom/server";
import type { Machine } from "../state/machines";
import { EnvironmentSelectorView } from "./EnvironmentSelector";

const LAN = { kind: "lan", address: "192.168.1.31:17879", encrypted: false };
const STUDIO: Machine = {
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
};
const LOCAL = { displayName: "My Mac", icon: null, tint: null };

function render(machines: Machine[], host: string): string {
  return renderToStaticMarkup(
    <Tooltip.Provider>
      <EnvironmentSelectorView machines={machines} local={LOCAL} host={host} />
    </Tooltip.Provider>,
  );
}

describe("the environment selector", () => {
  test("is hidden until a second computer is paired", () => {
    expect(render([], "")).toBe("");
  });

  test("names this computer when the next thread runs here", () => {
    const html = render([STUDIO], "");
    expect(html).toContain('aria-label="Run on: My Mac"');
    expect(html).toContain("My Mac");
  });

  test("names the environment the next thread runs on", () => {
    expect(render([STUDIO], "gw-1")).toContain('aria-label="Run on: Studio"');
    expect(render([{ ...STUDIO, displayName: "Office Mac" }], "gw-1")).toContain(
      'aria-label="Run on: Office Mac"',
    );
  });
});
