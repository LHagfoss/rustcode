import { describe, expect, test } from "bun:test";
import type { AgentChoice, ThreadChoice } from "./compose";
import { type MentionNode, mentionNodes, QUERY_ROWS, SHORT_ROWS } from "./mentionDrawer";

const agent = (harness: string, name: string, harnessName = harness): AgentChoice => ({
  harness,
  harnessName,
  name,
  token: `${harness}/${name}`,
});

const thread = (id: string, harness?: string): ThreadChoice => ({
  id,
  title: `Thread ${id}`,
  harness,
  state: "open",
});

/** What a node holds, for comparing a whole listing to what it should read as. */
function shape(nodes: MentionNode[]): string[] {
  return nodes.map((node) => {
    switch (node.kind) {
      case "heading":
        return `# ${node.label}`;
      case "harness":
        return `## ${node.name}`;
      case "more":
        return `+ ${node.section} ${node.hidden}`;
      case "less":
        return `- ${node.section}`;
      case "entry":
        return node.entry.kind === "agent"
          ? node.entry.agent.token
          : node.entry.kind === "thread"
            ? `t:${node.entry.thread.id}`
            : node.entry.kind === "file"
              ? `f:${node.entry.path}`
              : `k:${node.entry.scope}`;
      default:
        return "?";
    }
  });
}

const files = (count: number) =>
  Array.from({ length: count }, (_, i) => `src/file-${String(i + 1).padStart(2, "0")}.ts`);

describe("an empty @ drawer", () => {
  test(`shows ${SHORT_ROWS} rows of each section, the rest behind Show N more`, () => {
    const nodes = mentionNodes({
      query: "",
      keywords: ["agents", "threads", "files"],
      agents: [agent("pi", "pi-pro")],
      delegates: [],
      threads: Array.from({ length: 9 }, (_, i) => thread(`t${i}`, "pi")),
      files: files(8),
    });
    expect(shape(nodes)).toEqual([
      "k:agents",
      "k:threads",
      "k:files",
      "# Agents",
      "## pi",
      "pi/pi-pro",
      "# Threads",
      "t:t0",
      "t:t1",
      "t:t2",
      "t:t3",
      "t:t4",
      "+ threads 4",
      "# Files",
      "f:src/file-01.ts",
      "f:src/file-02.ts",
      "f:src/file-03.ts",
      "f:src/file-04.ts",
      "f:src/file-05.ts",
      "+ files 3",
    ]);
  });

  test("a section with nothing back has no Show N more row", () => {
    const nodes = mentionNodes({
      query: "",
      agents: [agent("pi", "pi-pro")],
      delegates: [],
      threads: [],
      files: files(2),
    });
    expect(shape(nodes)).toEqual([
      "# Agents",
      "## pi",
      "pi/pi-pro",
      "# Files",
      "f:src/file-01.ts",
      "f:src/file-02.ts",
    ]);
  });
});

describe("Show more and Show fewer", () => {
  test("draws the rest of that section, and only that one, in place", () => {
    const nodes = mentionNodes({
      query: "",
      expanded: new Set(["files"] as const),
      agents: [agent("pi", "pi-pro")],
      delegates: [],
      threads: Array.from({ length: 7 }, (_, i) => thread(`t${i}`, "pi")),
      files: files(8),
    });
    const drawn = shape(nodes);
    // Every file, none held back, and the way back to the short list under them; the threads keep
    // their own five and their own count.
    expect(drawn.filter((row) => row.startsWith("f:"))).toHaveLength(8);
    expect(drawn).toContain("+ threads 2");
    expect(drawn.filter((row) => row.startsWith("+ "))).toEqual(["+ threads 2"]);
    expect(drawn.filter((row) => row.startsWith("- "))).toEqual(["- files"]);
  });

  test("a section drawn whole says so, and no longer counts what it kept back", () => {
    const nodes = mentionNodes({
      query: "",
      expanded: new Set(["threads"] as const),
      agents: [],
      delegates: [],
      threads: Array.from({ length: 7 }, (_, i) => thread(`t${i}`, "pi")),
      files: [],
    });
    expect(shape(nodes)).toEqual(["t:t0", "t:t1", "t:t2", "t:t3", "t:t4", "t:t5", "t:t6", "- threads"]);
  });

  test("closing it again puts the short list and its count back", () => {
    const threads = Array.from({ length: 7 }, (_, i) => thread(`t${i}`, "pi"));
    const open = shape(
      mentionNodes({
        query: "",
        expanded: new Set(["threads"] as const),
        agents: [],
        delegates: [],
        threads,
        files: [],
      }),
    );
    const closed = shape(
      mentionNodes({
        query: "",
        expanded: new Set(),
        agents: [],
        delegates: [],
        threads,
        files: [],
      }),
    );
    expect(open).toEqual(["t:t0", "t:t1", "t:t2", "t:t3", "t:t4", "t:t5", "t:t6", "- threads"]);
    expect(closed).toEqual(["t:t0", "t:t1", "t:t2", "t:t3", "t:t4", "+ threads 2"]);
  });
});

describe("what is typed", () => {
  test("searches every item, not only the few rows on screen", () => {
    const many = Array.from({ length: 30 }, (_, i) => thread(`t${i}`, "pi"));
    // The 25th thread answers, and an empty drawer would never have drawn it.
    const nodes = mentionNodes({
      query: "t25",
      agents: [],
      delegates: [],
      threads: [many[25]!],
      files: [],
    });
    expect(shape(nodes)).toEqual(["t:t25"]);
    // Ranked over everything, so what the query ranked in arrives even behind the first rows.
    const ranked = mentionNodes({
      query: "x",
      agents: [],
      delegates: [],
      threads: many,
      files: [],
    });
    expect(shape(ranked).filter((row) => row.startsWith("t:"))).toHaveLength(QUERY_ROWS);
    expect(shape(ranked)).toContain(`+ threads ${30 - QUERY_ROWS}`);
  });
});

describe("agents grouped by harness", () => {
  test("a harness's own rows sit under its name, in the order the harnesses came", () => {
    const nodes = mentionNodes({
      query: "",
      agents: [
        agent("claude-code", "claude-sonnet", "Claude Code"),
        agent("codex", "gpt-6", "Codex"),
        agent("claude-code", "claude-opus", "Claude Code"),
      ],
      delegates: [],
      threads: [],
      files: [],
      harnessNames: { "claude-code": "Claude Code" },
    });
    expect(shape(nodes)).toEqual([
      "## Claude Code",
      "claude-code/claude-sonnet",
      "claude-code/claude-opus",
      "## Codex",
      "codex/gpt-6",
    ]);
  });

  test("a delegate sits under the harness it works for, after that harness's models", () => {
    const nodes = mentionNodes({
      query: "",
      agents: [agent("codex", "gpt-6", "Codex"), agent("pi", "pi-pro", "Pi")],
      delegates: [thread("running", "pi")],
      threads: [],
      files: [],
    });
    expect(shape(nodes)).toEqual(["## Codex", "codex/gpt-6", "## Pi", "pi/pi-pro", "t:running"]);
  });

  test("two rows under one harness still get its name", () => {
    const nodes = mentionNodes({
      query: "",
      agents: [agent("pi", "pi-pro", "Pi"), agent("pi", "pi-lite", "Pi")],
      delegates: [],
      threads: [],
      files: [],
    });
    expect(shape(nodes)).toEqual(["## Pi", "pi/pi-pro", "pi/pi-lite"]);
  });

  test("a harness whose one row is the harness by name needs no sub-heading", () => {
    const nodes = mentionNodes({
      query: "",
      agents: [
        agent("rustcode", "chatgpt/gpt-6-luna", "RustCode"),
        agent("rustcode", "claude-sonnet-5", "RustCode"),
        // A harness with no models of its own: its one row is the harness.
        { harness: "local-agent", harnessName: "Local Agent", name: "Local Agent", token: "local-agent" },
      ],
      delegates: [],
      threads: [],
      files: [],
    });
    expect(shape(nodes)).toEqual([
      "## RustCode",
      "rustcode/chatgpt/gpt-6-luna",
      "rustcode/claude-sonnet-5",
      "local-agent",
    ]);
  });

  test("a harness's one row keeps the sub-heading when the row is another name", () => {
    const nodes = mentionNodes({
      query: "",
      agents: [agent("codex", "gpt-6", "Codex"), agent("pi", "pi-pro", "Pi")],
      delegates: [],
      threads: [],
      files: [],
    });
    expect(shape(nodes)).toEqual(["## Codex", "codex/gpt-6", "## Pi", "pi/pi-pro"]);
  });

  test("a harness keeps its own rows while the section holds the others'", () => {
    const nodes = mentionNodes({
      query: "",
      agents: [
        ...Array.from({ length: 7 }, (_, i) => agent("pi", `pi-${i}`, "Pi")),
        agent("codex", "gpt-6", "Codex"),
      ],
      delegates: [],
      threads: [],
      files: [],
    });
    expect(shape(nodes)).toEqual([
      "## Pi",
      "pi/pi-0",
      "pi/pi-1",
      "pi/pi-2",
      "pi/pi-3",
      "pi/pi-4",
      "## Codex",
      "codex/gpt-6",
      "+ agents 2",
    ]);
  });
});

describe("one group at a time", () => {
  test("a keyword in the token drops the other sections, and the heading with them", () => {
    const nodes = mentionNodes({
      query: "",
      scope: "threads",
      agents: [agent("pi", "pi-pro")],
      delegates: [],
      threads: [thread("t0", "pi")],
      files: files(3),
    });
    expect(shape(nodes)).toEqual(["t:t0"]);
  });
});

describe("one header per harness", () => {
  test("the login and plan are said once on the harness's header, not on its rows", () => {
    const nodes = mentionNodes({
      query: "",
      agents: [
        { ...agent("claude-code", "opus", "Claude Code"), account: "Paral 2", plan: "97% left" },
        { ...agent("claude-code", "sonnet", "Claude Code"), account: "Paral 2", plan: "97% left" },
      ],
      delegates: [],
      threads: [],
      files: [],
    });
    const headers = nodes.filter((node) => node.kind === "harness");
    expect(headers).toEqual([
      { kind: "harness", harness: "claude-code", name: "Claude Code", account: "Paral 2", plan: "97% left" },
    ]);
  });

  test("a harness with one login names none", () => {
    const nodes = mentionNodes({
      query: "",
      agents: [agent("pi", "pi-pro", "Pi"), agent("pi", "pi-lite", "Pi")],
      delegates: [],
      threads: [],
      files: [],
    });
    expect(nodes[0]).toEqual({
      kind: "harness",
      harness: "pi",
      name: "Pi",
      account: undefined,
      plan: undefined,
    });
  });
});

describe("typing across the sections", () => {
  test("the section with the closest match comes first", () => {
    const nodes = mentionNodes({
      query: "deploy",
      agents: [],
      delegates: [],
      threads: [{ ...thread("x", "pi"), title: "Plan to deploy" }],
      files: ["src/deploy.ts"],
    });
    expect(shape(nodes)).toEqual(["# Files", "f:src/deploy.ts", "# Threads", "t:x"]);
  });

  test("a keyword being typed follows the matches, and leads nothing", () => {
    const nodes = mentionNodes({
      query: "th",
      keywords: ["threads"],
      agents: [],
      delegates: [],
      threads: [thread("t0", "pi")],
      files: [],
    });
    expect(shape(nodes)).toEqual(["t:t0", "k:threads"]);
  });
});
