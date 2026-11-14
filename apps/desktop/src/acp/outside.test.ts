import { describe, expect, test } from "bun:test";
import {
  createParams,
  lastUsedLabel,
  OutsideMethod,
  outcomeLabel,
  parseCreated,
  parseOutsideStatus,
} from "./outside";

describe("outside agents", () => {
  test("reads the engine's list, with defaults for what is missing", () => {
    const status = parseOutsideStatus({
      grants: [
        {
          id: "ab12",
          name: "Cursor",
          platform: "cursor",
          scope: "delegate",
          ask: true,
          createdAt: "2026-11-13T10:00:00.000Z",
          lastUsedAt: "2026-11-13T10:05:00.000Z",
          lastTool: "list_sessions",
        },
        { name: "no id is dropped" },
        { id: "cd34", name: "Odd", platform: "vim", scope: "root" },
      ],
      pending: [{ id: "p1", clientId: "ab12", clientName: "Cursor", tool: "start_session", summary: "go" }],
      recent: [
        { at: "2026-11-13T10:05:00.000Z", clientName: "Cursor", tool: "list_sessions", outcome: "ok" },
      ],
      command: "/bin/rustcode",
      httpUrl: "http://127.0.0.1:43871/mcp",
      maxGrants: 20,
      available: true,
    });
    expect(status.httpUrl).toBe("http://127.0.0.1:43871/mcp");
    expect(parseOutsideStatus({}).httpUrl).toBeNull();
    expect(parseOutsideStatus({ httpUrl: null }).httpUrl).toBeNull();
    expect(status.grants.map((grant) => grant.id)).toEqual(["ab12", "cd34"]);
    // An unknown level reads as the safest one, never as more access.
    expect(status.grants[1]?.scope).toBe("read_only");
    expect(status.grants[1]?.platform).toBe("other");
    expect(status.grants[0]?.lastTool).toBe("list_sessions");
    expect(status.pending[0]?.tool).toBe("start_session");
    expect(status.recent[0]?.durationMs).toBeNull();
    expect(parseOutsideStatus({}).grants).toEqual([]);
    expect(parseOutsideStatus({}).available).toBe(true);
  });

  test("a created grant carries the token and config only when all are there", () => {
    const created = parseCreated({
      grant: { id: "ab12", name: "Codex", platform: "codex", scope: "read_only" },
      token: "rcx_ab12_secret",
      snippet: { format: "toml", where: "~/.codex/config.toml", text: "[mcp_servers.rustcode]" },
    });
    expect(created?.snippet.format).toBe("toml");
    expect(created?.token).toBe("rcx_ab12_secret");
    expect(parseCreated({ grant: { id: "x", name: "y" }, snippet: {} })).toBeNull();
  });

  test("the request names the app, its platform, its level and whether it asks", () => {
    expect(OutsideMethod.create).toBe("_rustcode/outside/create");
    expect(createParams({ name: "  Cursor ", platform: "cursor", scope: "delegate", ask: true })).toEqual({
      name: "Cursor",
      platform: "cursor",
      scope: "delegate",
      ask: true,
    });
  });

  test("says when an app last called", () => {
    const now = Date.parse("2026-11-13T12:00:00.000Z");
    expect(lastUsedLabel(null, now)).toBe("Not used yet");
    expect(lastUsedLabel("2026-11-13T11:59:50.000Z", now)).toBe("Used just now");
    expect(lastUsedLabel("2026-11-13T11:30:00.000Z", now)).toBe("Used 30 min ago");
    expect(lastUsedLabel("2026-11-13T09:00:00.000Z", now)).toBe("Used 3 h ago");
    expect(lastUsedLabel("2026-11-10T12:00:00.000Z", now)).toBe("Used 3 d ago");
    expect(outcomeLabel("denied")).toBe("Refused (access)");
  });
});
