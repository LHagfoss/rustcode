import { arr, type Json, type JsonObject, num, obj, str } from "./jsonrpc";
import { mockSettingsAnswer, mockSettingsSet, withoutSkillContent } from "./mockFixtures";
import { EFFORTS, initialHarnesses, RUSTCODE_MODELS, withLast } from "./mockHarnesses";
import {
  MOCK_FLEET_PI,
  MOCK_MACHINE,
  MOCK_STUDIO,
  MOCK_WINDOWS,
  mockHostEntry,
  mockNameWins,
  mockRemote,
} from "./mockMachineFixtures";
import type { HarnessSnapshot } from "./types";

// The mock of a paired computer (`MockMachine`): its threads, settings and routes, as the
// fleet tools reach it. Split out of mock.ts.

export class MockMachine {
  /** The threads this window started on the machine, newest first (`session/new`). */
  private readonly created: {
    sessionId: string;
    cwd: string;
    title: string;
    harness: string;
    updatedAt: number;
  }[] = [];
  private createdCount = 0;
  /** The server settings of this machine, differing from this app's own. */
  private settings: JsonObject;
  private projectSettings: Record<string, JsonObject> = {};
  /** A harness change made here, over the machine's seeded snapshot. */
  private harnessOverride: HarnessSnapshot | null = null;
  /** What this client advertised: only one that lists threads hears about the ones made here. */
  private showsThreads = false;
  /** Its skills, with the SKILL.md so a copy can be seen. */
  private skills: JsonObject[];
  /** Its MCP servers, with key names but no secret values. */
  private mcpServers: JsonObject[];
  /** The name it was given from another computer, and when. */
  private ownName: string | null = null;
  private ownNameAt: string | null = null;

  constructor(
    private readonly name: string,
    private readonly threads?: number,
  ) {
    const studio = name === MOCK_STUDIO.name;
    this.settings = {
      "default-permission-mode": studio ? "ask" : "edits",
      "resume-after-restart": studio,
      "continue-after-limit": true,
      "auto-compact-mode": studio ? "size" : "cache",
      "auto-compact-threshold": studio ? 150_000 : 200_000,
      orchestration: true,
      "delegates-total": 8,
      "delegates-per-thread": studio ? 4 : 6,
      "read-across-projects": true,
      "delegate-presets": [],
      "limit-fallback": false,
      "limit-fallback-map": [],
      "wake-batch-seconds": 60,
      "workflow-concurrency": 3,
      "workflow-call-budget": 40,
      "workflow-timeout-minutes": 60,
      "workflow-kept-runs": 20,
    };
    this.skills = [
      {
        name: studio ? "frontend-design" : "release-automation",
        description: studio ? "Design guidance for web UIs" : "Cut a release with scripts/release.sh",
        path: studio
          ? "/Users/lars/.agents/skills/frontend-design"
          : "/home/lars/.agents/skills/release-automation",
        source: "universal",
        readOnly: false,
        enabled: true,
        content: studio
          ? "---\nname: frontend-design\ndescription: Design guidance for web UIs\n---\n\nRead docs/design.md before shaping a page.\n"
          : "---\nname: release-automation\ndescription: Cut a release with scripts/release.sh\n---\n\nRun scripts/release.sh --patch.\n",
      },
      {
        name: "shared-notes",
        description: "Notes every machine can have",
        path: "/home/lars/.config/rustcode/skills/shared-notes",
        source: "rustcode",
        readOnly: false,
        enabled: false,
        content:
          "---\nname: shared-notes\ndescription: Notes every machine can have\n---\n\nKeep notes short.\n",
      },
    ];
    this.mcpServers = studio
      ? [
          {
            name: "notes",
            transport: "http",
            target: "https://mcp.example.com/notes",
            enabled: true,
            scope: "user",
            connection: "connected",
            tools: ["search", "write"],
            error: null,
            envKeys: [],
            headerKeys: ["Authorization"],
            copyTarget: ["https://mcp.example.com/notes"],
          },
        ]
      : [
          {
            name: "github",
            transport: "stdio",
            target: "npx -y github-mcp",
            enabled: true,
            scope: "user",
            connection: "connected",
            tools: ["issues", "pulls"],
            error: null,
            envKeys: ["GITHUB_TOKEN"],
            headerKeys: [],
            copyTarget: ["npx", "-y", "github-mcp"],
          },
        ];
  }

  /** The frames this machine sends back for one line the page wrote. */
  receive(line: string): string[] {
    let frame: JsonObject;
    try {
      const parsed = JSON.parse(line) as Json;
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return [];
      frame = parsed;
    } catch {
      return [];
    }
    const id = frame.id;
    if (typeof id !== "number") return [];
    if (frame.method === "initialize") {
      const rustcode = obj(obj(obj(frame.params)?.clientCapabilities)?._meta)?.rustcode;
      this.showsThreads = !!num(obj(rustcode)?.threads);
      return [JSON.stringify({ jsonrpc: "2.0", id, result: { protocolVersion: 1, agentCapabilities: {} } })];
    }
    if (frame.method === "session/list")
      return [JSON.stringify({ jsonrpc: "2.0", id, result: { sessions: this.sessions() } })];
    if (frame.method === "_rustcode/harnesses")
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: (this.harnessOverride ?? this.harnesses()) as unknown as Json,
        }),
      ];
    if (frame.method === "session/new") {
      const created = this.newSession(id, frame.params);
      return this.showsThreads
        ? [
            JSON.stringify({
              jsonrpc: "2.0",
              method: "_rustcode/threads/created",
              params: created.notification,
            }),
            created.response,
          ]
        : [created.response];
    }
    if (frame.method === "_rustcode/threads/delete") {
      const sessionId = str(obj(frame.params)?.sessionId);
      const at = sessionId ? this.created.findIndex((entry) => entry.sessionId === sessionId) : -1;
      if (at >= 0) this.created.splice(at, 1);
      const gone = sessionId
        ? [
            JSON.stringify({
              jsonrpc: "2.0",
              method: "_rustcode/threads/deleted",
              params: { sessionId },
            }),
          ]
        : [];
      return [...gone, JSON.stringify({ jsonrpc: "2.0", id, result: {} })];
    }
    if (frame.method === "session/load") return this.load(id, frame.params);
    if (frame.method === "session/prompt") {
      const sessionId = str(obj(frame.params)?.sessionId);
      this.titleFromPrompt(sessionId, frame.params);
      const reply = [
        sessionId
          ? JSON.stringify({
              jsonrpc: "2.0",
              method: "session/update",
              params: {
                sessionId,
                update: {
                  sessionUpdate: "agent_message_chunk",
                  content: { type: "text", text: `Ran it on ${this.name}. The answer stays there.` },
                },
              },
            })
          : null,
        JSON.stringify({ jsonrpc: "2.0", id, result: { stopReason: "end_turn" } }),
      ];
      return reply.filter((x): x is string => x !== null);
    }
    if (frame.method === "_rustcode/settings/get") {
      const keys = arr(obj(frame.params)?.keys).flatMap((key) => (typeof key === "string" ? [key] : []));
      const project = str(obj(frame.params)?.project);
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: mockSettingsAnswer(this.settings, this.projectSettings, keys, project),
        }),
      ];
    }
    if (frame.method === "_rustcode/settings/set") {
      const params = obj(frame.params) ?? {};
      this.settings = mockSettingsSet(this.settings, this.projectSettings, params);
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: mockSettingsAnswer(this.settings, this.projectSettings, [], str(params.project)),
        }),
      ];
    }
    if (frame.method === "_rustcode/harnesses/default") {
      const params = obj(frame.params);
      const snapshot = this.harnessOverride ?? this.harnesses();
      this.harnessOverride = {
        ...snapshot,
        default: {
          harness: str(params?.harness) ?? snapshot.default.harness,
          model: str(params?.model) ?? null,
          effort: str(params?.effort) ?? null,
        },
      };
      return [JSON.stringify({ jsonrpc: "2.0", id, result: this.harnessOverride as unknown as Json })];
    }
    if (frame.method === "_rustcode/harnesses/set") {
      const params = obj(frame.params);
      const id = str(params?.id);
      const enabled = params?.enabled;
      const snapshot = this.harnessOverride ?? this.harnesses();
      this.harnessOverride = {
        ...snapshot,
        harnesses: snapshot.harnesses.map((harness) =>
          harness.id === id && typeof enabled === "boolean" ? { ...harness, enabled } : harness,
        ),
      };
      return [JSON.stringify({ jsonrpc: "2.0", id, result: this.harnessOverride as unknown as Json })];
    }
    if (frame.method === "_rustcode/capabilities") {
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: {
            configPath: `/home/lars/.config/rustcode/config.toml`,
            skills: this.skills.map(withoutSkillContent),
            mcpServers: this.mcpServers,
          },
        }),
      ];
    }
    if (frame.method === "_rustcode/capabilities/set") {
      const params = obj(frame.params);
      const name = str(params?.name);
      const enabled = params?.enabled === true;
      if (params?.kind === "skill")
        this.skills = this.skills.map((skill) => (str(skill.name) === name ? { ...skill, enabled } : skill));
      if (params?.kind === "mcp")
        this.mcpServers = this.mcpServers.map((server) =>
          str(server.name) === name ? { ...server, enabled } : server,
        );
      return this.receive(JSON.stringify({ jsonrpc: "2.0", id, method: "_rustcode/capabilities" }));
    }
    if (frame.method === "_rustcode/capabilities/mcp/add") {
      const params = obj(frame.params);
      const target = arr(params?.target).flatMap((part) => (typeof part === "string" ? [part] : []));
      const transport = str(params?.transport) ?? (target[0]?.startsWith("http") ? "http" : "stdio");
      this.mcpServers = [
        ...this.mcpServers.filter((server) => str(server.name) !== str(params?.name)),
        {
          name: str(params?.name) ?? "server",
          transport,
          target: target.join(" "),
          enabled: true,
          scope: "user",
          connection: "connected",
          tools: [],
          error: null,
          envKeys: arr(params?.env).map((line) => String(line).split("=")[0] ?? ""),
          headerKeys: arr(params?.headers).map((line) => String(line).split(":")[0] ?? ""),
          copyTarget: target,
        },
      ];
      return this.receive(JSON.stringify({ jsonrpc: "2.0", id, method: "_rustcode/capabilities" }));
    }
    if (frame.method === "_rustcode/capabilities/mcp/remove") {
      const name = str(obj(frame.params)?.name);
      this.mcpServers = this.mcpServers.filter((server) => str(server.name) !== name);
      return this.receive(JSON.stringify({ jsonrpc: "2.0", id, method: "_rustcode/capabilities" }));
    }
    if (frame.method === "_rustcode/skills/get") {
      const params = obj(frame.params);
      const skill = this.skills.find(
        (entry) => str(entry.name) === str(params?.name) && str(entry.path) === str(params?.path),
      );
      if (!skill || typeof skill.content !== "string") {
        return [
          JSON.stringify({
            jsonrpc: "2.0",
            id,
            error: {
              code: -32602,
              message: `Could not read ${str(params?.name) ?? "that"}'s SKILL.md`,
            },
          }),
        ];
      }
      return [JSON.stringify({ jsonrpc: "2.0", id, result: { content: skill.content } })];
    }
    if (frame.method === "_rustcode/environment/sync") {
      const known = [MOCK_MACHINE, MOCK_STUDIO, MOCK_WINDOWS].find((machine) => machine.name === this.name);
      const version = known && "version" in known ? known.version : "1.28.0";
      const host = known
        ? { id: known.id, name: this.ownName ?? known.name, platform: known.platform, version }
        : { id: `gw-mock-${this.name}`, name: this.ownName ?? this.name, platform: "linux", version };
      const params = obj(frame.params);
      const about = arr(params?.entries)
        .map((entry) => obj(entry))
        .find((entry) => str(entry?.hostId) === host.id);
      if (
        about &&
        str(about.nameUpdatedAt) &&
        mockNameWins(about, { name: host.name, nameUpdatedAt: this.ownNameAt ?? "" })
      ) {
        this.ownName = str(about.name) ?? this.ownName;
        this.ownNameAt = str(about.nameUpdatedAt) ?? this.ownNameAt;
        host.name = this.ownName ?? host.name;
      }
      const entry = mockHostEntry(host);
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: {
            host: this.ownNameAt ? { ...entry, nameUpdatedAt: this.ownNameAt } : entry,
            // The Studio is paired with a Pi this computer has never met.
            entries:
              this.name === MOCK_STUDIO.name
                ? [{ ...MOCK_FLEET_PI, lastSeenAt: new Date(Date.now() - 3 * 3_600_000).toISOString() }]
                : [],
          },
        }),
      ];
    }
    if (frame.method === "_rustcode/skills/add") {
      const params = obj(frame.params);
      const name = str(params?.name) ?? "skill";
      this.skills = [
        ...this.skills.filter((skill) => str(skill.name) !== name),
        {
          name,
          description: "Copied from another machine",
          path: `/home/lars/.config/rustcode/skills/${name}`,
          source: "rustcode",
          readOnly: false,
          enabled: true,
          ...(str(params?.content) ? { content: str(params?.content) } : {}),
        },
      ];
      return [
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: { path: `/home/lars/.config/rustcode/skills/${name}/SKILL.md` },
        }),
      ];
    }
    return [
      JSON.stringify({
        jsonrpc: "2.0",
        id,
        error: {
          code: -32601,
          message: `${this.name} does not answer ${String(frame.method)}.`,
        },
      }),
    ];
  }

  /** Opening one of its threads: a two-line history, then the answer. */
  private load(id: number, params: Json | undefined): string[] {
    const sessionId = str(obj(params)?.sessionId);
    const title = this.sessions().find((s) => s.sessionId === sessionId)?.title ?? "A thread";
    const update = (u: JsonObject) =>
      JSON.stringify({ jsonrpc: "2.0", method: "session/update", params: { sessionId, update: u } });
    return [
      update({
        sessionUpdate: "user_message_chunk",
        content: { type: "text", text: `Run "${title}" on ${this.name}.` },
      }),
      update({
        sessionUpdate: "agent_message_chunk",
        content: {
          type: "text",
          text: `This thread lives on ${this.name}. Its workspace, its files and its shell are there; this window reads and drives it over the machine network.`,
        },
      }),
      JSON.stringify({ jsonrpc: "2.0", id, result: { configOptions: [] } }),
    ];
  }

  /** The engine titles a thread from its first prompt; a thread started here does the same. */
  private titleFromPrompt(sessionId: string | undefined, params: Json | undefined): void {
    const session = this.created.find((entry) => entry.sessionId === sessionId);
    if (session?.title !== "New thread") return;
    const text = arr(obj(params)?.prompt)
      .map((block) => str(obj(block)?.text) ?? "")
      .join(" ")
      .trim();
    if (text) session.title = text.length > 60 ? `${text.slice(0, 60)}…` : text;
  }

  /** `session/new` on this machine: a thread in its folder, run by one of its harnesses. */
  private newSession(id: number, params: Json | undefined): { response: string; notification: JsonObject } {
    const studio = this.name === MOCK_STUDIO.name;
    const meta = obj(obj(params)?._meta)?.rustcode;
    this.createdCount += 1;
    const cwd = str(obj(params)?.cwd) || (studio ? "/Users/lars/code/rustcode" : "/home/lars/code/rustcode");
    const sessionId = `${studio ? "mock-studio" : "mock-linux"}-new-${this.createdCount}`;
    const harness = str(obj(meta)?.harness) ?? (studio ? "claude-code" : "codex");
    this.created.unshift({
      sessionId,
      cwd,
      title: "New thread",
      harness,
      updatedAt: Date.now(),
    });
    return {
      response: JSON.stringify({ jsonrpc: "2.0", id, result: { sessionId, configOptions: [] } }),
      // What the engine says to a client that lists threads, as the thread is made.
      notification: { sessionId, cwd, harness },
    };
  }

  /**
   * This machine's own harnesses, models, logins and default: the Studio runs Claude Code,
   * the Linux box Codex, and neither has this Mac's built-in harness list
   * (docs/desktop.md › Several machines).
   */
  private harnesses(): HarnessSnapshot {
    const studio = this.name === MOCK_STUDIO.name;
    const base = initialHarnesses(true);
    const kept = studio ? ["claude-code", "codex"] : ["rustcode", "codex"];
    const models: Record<string, HarnessSnapshot["harnesses"][number]["models"]> = studio
      ? {
          "claude-code": [
            {
              id: "sonnet-5.5",
              name: "Sonnet 5.5",
              efforts: ["low", "medium", "high", "max"],
              default_effort: "high",
            },
            { id: "opus-5.5", name: "Opus 5.5", efforts: ["medium", "high", "max"], default_effort: "high" },
          ],
          codex: [{ id: "gpt-5.5", name: "GPT-5.5", efforts: EFFORTS, default_effort: "medium" }],
        }
      : {
          rustcode: RUSTCODE_MODELS.map((m) => ({
            id: m.value,
            name: m.name,
            efforts: EFFORTS,
            default_effort: "medium",
          })),
          codex: [{ id: "gpt-5.5-codex", name: "GPT-5.5 Codex", efforts: EFFORTS, default_effort: "medium" }],
        };
    return withLast({
      harnesses: base.harnesses
        .filter((harness) => kept.includes(harness.id))
        .map((harness) => ({ ...harness, enabled: true, auth: "ok", models: models[harness.id] ?? [] })),
      default: studio
        ? { harness: "claude-code", model: "sonnet-5.5", effort: "high" }
        : { harness: "codex", model: "gpt-5.5-codex", effort: "medium" },
      onboarded: true,
      remembers: true,
    });
  }

  private sessions(): JsonObject[] {
    const studio = this.name === MOCK_STUDIO.name;
    const now = Date.now();
    const seeded = studio
      ? [
          { title: "Ship the mock window shots", cwd: "/Users/lars/code/rustcode", harness: "claude-code" },
          { title: "Review the machine groups", cwd: "/Users/lars/code/rustcode", harness: "claude-code" },
          { title: "Draft the release notes", cwd: "/Users/lars/code/notes", harness: "codex" },
        ]
      : [
          { title: "Run the tests", cwd: "/home/lars/code/rustcode", harness: "codex" },
          { title: "Fix the flaky CI job", cwd: "/home/lars/code/rustcode", harness: "codex" },
          { title: "Draft the release notes", cwd: "/home/lars/code/rustcode", harness: "rustcode" },
        ];
    const count = this.threads ?? (studio ? MOCK_STUDIO.threads : MOCK_MACHINE.threads);
    // The engine lists a session once it saved history: a just-made thread has none until its
    // first prompt, and reaches a client through `_rustcode/threads/created` until then.
    const listed = [
      ...this.created.filter((session) => session.title !== "New thread"),
      ...seeded.slice(0, count).map((session, index) => ({
        ...session,
        sessionId: `${studio ? "mock-studio" : "mock-linux"}-${index + 1}`,
        updatedAt: now,
      })),
    ];
    return listed.map((session, index) => ({
      sessionId: session.sessionId,
      cwd: session.cwd,
      title: session.title,
      updatedAt: new Date(session.updatedAt - index * 60_000).toISOString(),
      // The shape `session/list` really carries (`acp/types.ts` `parseSessionList`), including
      // the project's git remote, which finds the same project on this computer (state/projects.ts).
      _meta: {
        model: studio ? "sonnet-5.5" : "gpt-5.5-codex",
        rustcode: {
          harness: session.harness,
          ...(mockRemote(session.cwd) ? { remote: mockRemote(session.cwd) } : {}),
        },
      },
    }));
  }
}
