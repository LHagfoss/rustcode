import { describe, expect, test } from "bun:test";
import type { McpServer } from "./Capabilities";
import { cardState, firstLine } from "./Connectors";

const server = (over: Partial<McpServer>): McpServer => ({
  name: "vercel",
  transport: "http",
  target: "https://mcp.vercel.com",
  enabled: true,
  scope: "user",
  connection: "connecting",
  tools: [],
  toolCount: 0,
  envKeys: [],
  headerKeys: [],
  copyTarget: null,
  ...over,
});

describe("connector card state", () => {
  test("an app not set up is available", () => {
    expect(cardState(undefined)).toBe("available");
  });

  test("a failed connection offers a retry instead of a dead chip", () => {
    expect(cardState(server({ connection: "failed", error: "client registration failed" }))).toBe("failed");
  });

  test("a signed-out server asks for a sign-in even with an error", () => {
    expect(cardState(server({ auth: "signed_out", error: "sign-in required" }))).toBe("signed-out");
  });

  test("connected and off win over everything else", () => {
    expect(cardState(server({ connection: "connected", auth: "signed_in" }))).toBe("connected");
    expect(cardState(server({ enabled: false, error: "x" }))).toBe("off");
  });
});

test("a card error drops the engine's server prefix and keeps the first line", () => {
  expect(firstLine("MCP OAuth for 'vercel': token request failed (HTTP 401)\nmore")).toBe(
    "token request failed (HTTP 401)",
  );
});
