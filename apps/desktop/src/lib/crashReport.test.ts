import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import type { Transport } from "../acp/transport";
import {
  CRASH_REPORTS_DEFAULT,
  CrashRateLimiter,
  crashReportNoticeSeen,
  crashReportsEnabled,
  markCrashReportNoticeSeen,
  reportWebError,
  resetCrashRateLimiterForTest,
  scrubCrashText,
  setCrashReportsEnabled,
} from "./crashReport";
import { setShellTransport } from "./shell";

const store = new Map<string, string>();
const scope = globalThis as unknown as Record<string, unknown>;

beforeEach(() => {
  store.clear();
  scope.localStorage = {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => {
      store.set(key, value);
    },
    removeItem: (key: string) => {
      store.delete(key);
    },
  };
  resetCrashRateLimiterForTest();
  setShellTransport(null);
});

afterEach(() => {
  delete scope.localStorage;
  setShellTransport(null);
});

describe("scrubCrashText", () => {
  test("removes home paths, prompt text, tokens and env assignments", () => {
    const raw = [
      "at /custom/me/src/App.tsx:12",
      "at /Users/alice/projects/app/index.ts:8",
      "at /home/bob/work/main.rs:4",
      "at C:\\Users\\carol\\repo\\file.ts:1",
      'OPENAI_API_KEY=sk-proj-1234567890abcdef HOME=/home/bob MY_TOKEN="secret value"',
      "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig",
      "token ghp_1234567890abcdefghij",
      '{"prompt":"private user prompt","messages":[{"content":"secret"}],"status":500}',
    ].join(" ");

    const clean = scrubCrashText(raw, "/custom/me");

    expect(clean).not.toContain("/custom/me");
    expect(clean).not.toContain("alice");
    expect(clean).not.toContain("bob");
    expect(clean).not.toContain("carol");
    expect(clean).toContain("~/src/App.tsx:12");
    expect(clean).toContain("~/projects/app/index.ts:8");
    expect(clean).toContain("~/work/main.rs:4");
    expect(clean).toContain("~\\repo\\file.ts:1");
    expect(clean).not.toContain("sk-proj-1234567890abcdef");
    expect(clean).not.toContain("secret value");
    expect(clean).not.toContain("eyJhbGciOiJIUzI1NiJ9");
    expect(clean).not.toContain("ghp_1234567890abcdefghij");
    expect(clean).not.toContain("private user prompt");
    expect(clean).toContain("OPENAI_API_KEY=[REDACTED]");
    expect(clean).toContain("MY_TOKEN=[REDACTED]");
    expect(clean).toContain("Bearer [REDACTED]");
    expect(clean).toContain('"prompt":"[REDACTED]"');
    expect(clean).toContain('"messages":"[REDACTED]"');
    expect(clean).toContain('"status":500');
  });
});

describe("crash reporting preference and forwarding", () => {
  test("defaults to enabled and toggles immediately without restart", () => {
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    const transport: Transport = {
      name: "mock",
      start: async () => {},
      send: async () => {},
      restart: async () => {},
      stop: async () => {},
      call: async (command, args) => {
        calls.push({ command, args });
        return { enabled: Boolean(args.enabled), configured: true };
      },
    };
    setShellTransport(transport);

    expect(crashReportsEnabled()).toBe(CRASH_REPORTS_DEFAULT);
    expect(crashReportsEnabled()).toBe(true);
    expect(crashReportNoticeSeen()).toBe(false);

    expect(reportWebError(new Error("boom at /Users/alice/app.ts"), { now: 1_000 })).toBe(true);
    expect(calls.some((c) => c.command === "crash_report_send")).toBe(true);
    const sent = calls.find((c) => c.command === "crash_report_send")!;
    expect(sent.args.message).toBe("boom at ~/app.ts");

    setCrashReportsEnabled(false);
    expect(crashReportsEnabled()).toBe(false);
    expect(crashReportNoticeSeen()).toBe(true);
    expect(calls.some((c) => c.command === "crash_report_set_enabled" && c.args.enabled === false)).toBe(
      true,
    );

    expect(reportWebError(new Error("ignored while off"), { now: 2_000 })).toBe(false);

    setCrashReportsEnabled(true);
    expect(reportWebError(new Error("sent after re-enabling"), { now: 3_000 })).toBe(true);
  });

  test("marks first-run notice seen independently", () => {
    expect(crashReportNoticeSeen()).toBe(false);
    markCrashReportNoticeSeen();
    expect(crashReportNoticeSeen()).toBe(true);
  });
});

describe("CrashRateLimiter", () => {
  test("deduplicates identical errors and caps bursts", () => {
    const limiter = new CrashRateLimiter();
    expect(limiter.allow("same", 0)).toBe(true);
    expect(limiter.allow("same", 10_000)).toBe(false);
    expect(limiter.allow("same", 61_000)).toBe(true);

    const burst = new CrashRateLimiter();
    for (let i = 0; i < 10; i++) {
      expect(burst.allow(`err-${i}`, 1_000)).toBe(true);
    }
    expect(burst.allow("err-overflow", 2_000)).toBe(false);
    expect(burst.allow("err-overflow", 5 * 60_000 + 2_000)).toBe(true);
  });
});
