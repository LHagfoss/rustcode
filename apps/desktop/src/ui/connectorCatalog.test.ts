import { describe, expect, test } from "bun:test";
import { CONNECTORS, configuredAs, connectorName, matchesConnector } from "./connectorCatalog";

const linear = CONNECTORS.find((c) => c.id === "linear")!;

describe("connector catalog", () => {
  test("every connector has a unique id and an https URL", () => {
    expect(new Set(CONNECTORS.map((c) => c.id)).size).toBe(CONNECTORS.length);
    for (const c of CONNECTORS) expect(c.url.startsWith("https://")).toBe(true);
  });

  test("a configured server matches by URL, not name", () => {
    const servers = [{ name: "my-linear", target: "https://mcp.linear.app/mcp/" }];
    expect(configuredAs(linear, servers)?.name).toBe("my-linear");
    expect(configuredAs(linear, [{ name: "linear", target: "npx linear-mcp" }])).toBeUndefined();
  });

  test("a taken name gets a number", () => {
    expect(connectorName(linear, [])).toBe("linear");
    expect(connectorName(linear, ["linear", "linear-2"])).toBe("linear-3");
  });

  test("search reads the name and description", () => {
    expect(matchesConnector(linear, "issues")).toBe(true);
    expect(matchesConnector(linear, "postgres")).toBe(false);
    expect(matchesConnector(linear, "  ")).toBe(true);
  });
});
