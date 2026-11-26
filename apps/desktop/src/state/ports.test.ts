import { describe, expect, test } from "bun:test";
import {
  formatPortLabel,
  formatPortsSummary,
  type ListeningPort,
  parseListeningPort,
  parsePortList,
} from "./ports";

describe("listening ports parsing and formatting", () => {
  test("parses sorted listening ports with optional labels and skips malformed rows", () => {
    const parsed = parsePortList({
      list: {
        cwd: "/code/worktrees/wt-a",
        ports: [
          {
            port: 5173,
            pid: 4201,
            address: "127.0.0.1",
            processName: "vite",
            url: "http://localhost:5173",
          },
          {
            port: 3000,
            pid: 4200,
            address: "0.0.0.0",
            processName: "node",
            label: "Web",
            url: "http://localhost:3000",
          },
          {
            port: 0,
            pid: 99,
            address: "127.0.0.1",
            processName: "invalid",
          },
        ],
      },
    });

    expect(parsed.error).toBeUndefined();
    expect(parsed.cwd).toBe("/code/worktrees/wt-a");
    expect(parsed.ports).toEqual([
      {
        port: 3000,
        pid: 4200,
        address: "0.0.0.0",
        processName: "node",
        label: "Web",
        url: "http://localhost:3000",
      },
      {
        port: 5173,
        pid: 4201,
        address: "127.0.0.1",
        processName: "vite",
        url: "http://localhost:5173",
      },
    ]);
  });

  test("returns structured error message when engine reports an error", () => {
    const parsed = parsePortList({
      error: { code: "not_found", message: "No listening process on :3000" },
    });
    expect(parsed.ports).toEqual([]);
    expect(parsed.error).toBe("No listening process on :3000");
  });

  test("rejects malformed port entries", () => {
    expect(parseListeningPort(undefined)).toBeNull();
    expect(parseListeningPort({ port: 70000, pid: 10 })).toBeNull();
    expect(parseListeningPort({ port: 3000 })).toBeNull();
  });

  test("formats single and multiple active port summaries", () => {
    const web: ListeningPort = {
      port: 3000,
      pid: 100,
      address: "127.0.0.1",
      processName: "node",
      label: "Web",
      url: "http://localhost:3000",
    };
    const api: ListeningPort = {
      port: 8787,
      pid: 101,
      address: "127.0.0.1",
      processName: "bun",
      url: "http://localhost:8787",
    };

    expect(formatPortLabel(web)).toBe("Web :3000");
    expect(formatPortLabel(api)).toBe(":8787");
    expect(formatPortsSummary([])).toBe("");
    expect(formatPortsSummary([web])).toBe("Web :3000");
    expect(formatPortsSummary([api, web])).toBe(":8787 +1");
  });
});
