import { describe, expect, test } from "bun:test";
import type { TranscriptItem } from "./thread";
import { cleanToolPath, formatToolRow } from "./toolRowFormat";

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

const tool = (overrides: Partial<ToolItem>): ToolItem => ({
  kind: "tool",
  id: "t1",
  title: "Tool",
  status: "completed",
  diffs: [],
  ...overrides,
});

describe("cleanToolPath", () => {
  const cwd = "/home/lucas/code/worktrees/rustcode-polish/apps/desktop";

  test("resolves parent traversal outside cwd and shortens home", () => {
    expect(cleanToolPath("../../../../rustcode/AGENTS.md", cwd)).toBe("~/code/rustcode/AGENTS.md");
    expect(cleanToolPath("../../../../../.gemini/skills/impeccable/SKILL.md", cwd)).toBe(
      "~/.gemini/skills/impeccable/SKILL.md",
    );
  });

  test("strips cwd prefix for files inside the working directory", () => {
    expect(
      cleanToolPath("/home/lucas/code/worktrees/rustcode-polish/apps/desktop/src/ui/Rows.tsx", cwd),
    ).toBe("src/ui/Rows.tsx");
    expect(cleanToolPath("src/state/store.ts", cwd)).toBe("src/state/store.ts");
  });
});

describe("formatToolRow", () => {
  const cwd = "/home/lucas/code/rustcode";

  test("normalizes Antigravity read calls with relative paths and line ranges", () => {
    const row = formatToolRow(
      tool({
        title: "File ../../../../.gemini/skills/impeccable/SKILL.md",
        toolKind: "read",
        input: { Path: "/home/lucas/.gemini/skills/impeccable/SKILL.md", StartLine: 1, EndLine: 120 },
      }),
      "read",
      "/home/lucas/code/worktrees/rustcode-polish/apps/desktop",
    );
    expect(row).toEqual({
      title: "Read",
      pathDir: "~/.gemini/skills/impeccable/",
      pathFile: "SKILL.md",
      lineRange: "L1–120",
      summary: undefined,
      detailMono: true,
    });
  });

  test("distinguishes Antigravity edit vs write calls", () => {
    const edited = formatToolRow(
      tool({
        title: "File apps/desktop/src/ui/Rows.tsx",
        toolKind: "edit",
        input: {
          Path: "/home/lucas/code/rustcode/apps/desktop/src/ui/Rows.tsx",
          Target: "a",
          Replacement: "b",
        },
      }),
      "edit",
      cwd,
    );
    expect(edited.title).toBe("Edit");
    expect(edited.pathDir).toBe("apps/desktop/src/ui/");
    expect(edited.pathFile).toBe("Rows.tsx");

    const written = formatToolRow(
      tool({
        title: "File apps/desktop/src/state/toolRowFormat.ts",
        toolKind: "edit",
        input: {
          Path: "/home/lucas/code/rustcode/apps/desktop/src/state/toolRowFormat.ts",
          Content: "export {}",
        },
      }),
      "edit",
      cwd,
    );
    expect(written.title).toBe("Write");
    expect(written.pathFile).toBe("toolRowFormat.ts");
  });

  test("uses Antigravity toolSummary for commands and strips redundant cd", () => {
    const row = formatToolRow(
      tool({
        title: "cd /home/lucas/code/rustcode && git status --short",
        toolKind: "execute",
        input: {
          CommandLine: "cd /home/lucas/code/rustcode && git status --short",
          WaitMsBeforeAsync: 5000,
          toolSummary: "Git status check",
        },
      }),
      "execute",
      cwd,
    );
    expect(row).toEqual({
      title: "Git status check",
      detail: "git status --short",
      detailMono: true,
    });
  });

  test("keeps plain shell commands clean when no toolSummary is sent", () => {
    const row = formatToolRow(
      tool({
        title: "npm test",
        toolKind: "execute",
        input: { command: "npm test", description: "Run the unit tests" },
      }),
      "execute",
      cwd,
    );
    expect(row).toEqual({
      title: "npm test",
      titleMono: true,
      summary: undefined,
      detail: undefined,
      detailMono: true,
    });
  });

  test("formats Antigravity flattened MCP calls and web searches cleanly", () => {
    const mcp = formatToolRow(
      tool({
        title: "Run mcp_rustcode_list_threads",
        toolKind: "other",
        input: { toolSummary: "List project threads" },
      }),
      "list_thread",
      cwd,
    );
    expect(mcp.title).toBe("Listed threads");

    const web = formatToolRow(
      tool({
        title: '"Sentry pricing" on Web',
        toolKind: "search",
        input: { query: "Sentry pricing" },
      }),
      "search",
      cwd,
    );
    expect(web.title).toBe("Searched web");
    expect(web.summary).toBe("Sentry pricing");
    expect(web.detail).toBeUndefined();
  });
});
