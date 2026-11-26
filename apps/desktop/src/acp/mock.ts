// design-lint: allow-file (fake link colours in canned data)

import {
  COMMANDS,
  configOptions,
  MOCK_CHECK_RUNS,
  MOCK_PULLS,
  MOCK_REPORTS,
  MOCK_SKILLS,
  type MockConnectRequest,
  type MockOptions,
  mockContext,
  mockSettingsAnswer,
  mockSettingsSet,
  withoutSkillContent,
} from "./mockFixtures";
import { mockGit, mockQrRows } from "./mockGit";
import { setMockAccess } from "./mockRemoteAccess";
// A fake `rustcode --acp` engine with canned traffic, for tests and for
// running the UI without an engine (`bun run dev:mock`, or `?transport=mock`).
// It speaks the same JSON-RPC lines as the real engine, including the
// harness contract in docs/harnesses.md, so the UI works before that lands.

import { BUILTIN_THEMES } from "../lib/themes";
import { arr, type Json, type JsonObject, num, obj, str } from "./jsonrpc";
import { MockConnections } from "./mockConnections";
import { mockBrowse, mockCreate } from "./mockFolders";
import { ACP_CAPS, HARNESS_MODELS, initialHarnesses, MOCK_CUSTOM_MODELS, withLast } from "./mockHarnesses";
import { MockLoops } from "./mockLoops";
import { MOCK_LOCAL_HOST, mockHostEntry, mockNameWins, mockRemote } from "./mockMachineFixtures";
import { MockAddedMcp } from "./mockMcp";
import { MockMemory } from "./mockMemory";
import { MockOutside } from "./mockOutside";
import {
  APP_PATCH,
  type MockThread,
  type MockWorktree,
  minutes,
  mockParent,
  receiptKey,
  seedBranchThreads,
  seedDelegatedThreads,
  seedDelegateLinks,
  seedDelegateTreeThreads,
  seedRepeatedNotices,
  seedThreads,
  seedToolRowsThread,
  slug,
  text,
  update,
} from "./mockSeeds";
import {
  catalogReply,
  MOCK_CHARTS,
  MOCK_DASH,
  MOCK_HEADINGS,
  MOCK_KIT,
  MOCK_PROPOSAL,
  MOCK_RECONNECT_HTML,
  MOCK_RECONNECT_SHORTHAND,
  MOCK_REVIEW,
  MOCK_TABLE,
  MOCK_VIEW,
  MOCK_VIZ,
  mockLinkMeta,
} from "./mockSmartUi";
import { handleMockTransfer, isMockTransferMethod } from "./mockTransfer";
import { mockAntigravityPlan, mockSharedPlans, mockUsage } from "./mockUsage";
import type { HarnessSnapshot } from "./types";

export {
  MOCK_CHARTS,
  MOCK_DASH,
  MOCK_HEADINGS,
  MOCK_KIT,
  MOCK_PROPOSAL,
  MOCK_RECONNECT_HTML,
  MOCK_RECONNECT_SHORTHAND,
  MOCK_REVIEW,
  MOCK_TABLE,
  MOCK_VIEW,
  MOCK_VIZ,
  mockLinkMeta,
};

export class MockEngine {
  /** What this computer knows of the others (`fleet.json`), by host id. */
  private textGenerators: JsonObject = { generators: [], selected: null };
  /** Review conversations resolved or unresolved through `_rustcode/pulls/thread`. */
  private pullResolved = new Map<string, boolean>();
  private fleet = new Map<string, JsonObject>();
  /** This computer's own chosen name and when it was chosen (`machine.json`). */
  private ownName: { name: string; at: string } | null = null;
  private threads: MockThread[];
  private harnesses: HarnessSnapshot;
  private running = new Map<string, { cancelled: boolean; steers: string[] }>();
  private nextRpcId = 1000;
  private waiting = new Map<number, (result: JsonObject) => void>();
  private counter = 0;
  private loops = new MockLoops();
  private memory = new MockMemory();
  private connections = new MockConnections();
  private agents = new Map<string, JsonObject[]>();
  /** The inbox seed runs once, after the list the app first reads. */
  private needsYouSeeded = false;
  private pullEventDelivered = false;
  /** The client's `_meta.rustcode.orchestration` at initialize. */
  private orchestration = 0;
  /** Discord Rich Presence (docs/discord-rich-presence.md), as `discord_rpc_enabled` holds it. */
  private presence = true;
  /** The server settings of this machine (`_rustcode/settings`, docs/configuration.md). */
  private projectSettings: Record<string, JsonObject> = {};
  private serverSettings: JsonObject = {
    "default-permission-mode": "ask",
    "resume-after-restart": true,
    "continue-after-limit": true,
    "auto-compact-mode": "size",
    "auto-compact-threshold": 150_000,
    orchestration: true,
    "delegates-total": 8,
    "delegates-per-thread": 4,
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
  /** The payload-capture switches (docs/data-store.md › Captured payloads), off as `config.toml` defaults them. */
  private payloadCapture = { providerPayloads: false, harnessRpcPayloads: false };
  /** The thread mirror's switch and opt-outs (`engine/src/acp/mirror.rs`). */
  private mirror = { enabled: true, excludedProjects: [] as string[] };
  /**
  /**
   * Remote access (docs/remote.md), as the engine's gateway reports it: a
   * machine-wide switch and gateway, the threads this app shared (whatever
   * harness runs them), one paired phone, and the pairing offer while it
   * lasts. `refused` holds the threads the host turns down and why, which the
   * machine's answer counts as `notShared` and a thread's answer names.
   */
  /** Outside agents (Settings › Outside agents), in `mockOutside.ts`. */
  private outside = new MockOutside();

  private remote = {
    enabled: false,
    shared: new Set<string>(),
    gateway: null as JsonObject | null,
    devices: [
      {
        id: "7c1f3a2b",
        name: "Lars's iPhone",
        kind: "phone",
        platform: "ios",
        lastSeenAt: new Date(Date.now() - 2 * 3_600_000).toISOString(),
      },
    ] as JsonObject[],
    offer: null as JsonObject | null,
    pairingDue: false,
    pairingError: null as string | null,
    refused: new Map<string, string>(),
    /** Connections the gateway turned away, newest first (`refusedConnections`). */
    refusedConnections: [] as JsonObject[],
    /** Phones asking to connect, as the gateway holds them (`remote_gateway/connect.rs`). */
    connect: [] as MockConnectRequest[],
    /** Requests that ended, oldest first, as the gateway keeps its last eight. */
    endedConnect: [] as { id: string; deviceName: string; end: string; deviceId?: string; at: number }[],
    /** The open connect code for another computer, apart from the challenge (`pairing.rs`). */
    machineOffer: null as { serial: number; expiresAt: number } | null,
    machineSerial: 0,
  };
  private removedWorktrees = new Set<string>();
  /** The link book the app keeps in the data store: none until it saves one, and kept in `options` as the engine's disk does. */
  private get linkBook(): JsonObject | undefined {
    return this.options.links;
  }
  private set linkBook(book: JsonObject | undefined) {
    this.options.links = book;
  }
  /** What the store remembers of each orchestration call made with a retry key. */
  private get receipts(): JsonObject {
    this.options.receipts ??= {};
    return this.options.receipts;
  }
  /** The engine's themes: the app's built-ins plus one user theme, as `_rustcode/themes` lists them. */
  private themes: JsonObject[] = [
    ...BUILTIN_THEMES.map((t) => ({ ...t, tokens: { ...t.tokens } }) as unknown as JsonObject),
    {
      name: "graphite",
      description: "Graphite: lighter greys, round corners",
      source: "user",
      path: "~/.config/rustcode/themes/graphite.toml",
      mode: "dark",
      radius: "round",
      tokens: {
        ...BUILTIN_THEMES[0]!.tokens,
        bg: "#101012",
        sidebar: "#141416",
        surface: "#1a1a1d",
        surface_2: "#212125",
        surface_3: "#2a2a2f",
        accent: "#8b8bf5",
        focus: "#8b8bf5",
        selection: "#8b8bf54d",
      },
    },
  ];
  private activeTheme = "rustcode";
  /** MCP servers the mock restarted through `_rustcode/capabilities/mcp/restart`. */
  private mcpRestarted = new Set<string>();
  private mcpAdded = new MockAddedMcp();
  /**
   * The timed approvals `_rustcode/permissions` lists: one seeded hour so the
   * details panel has "Allowed for 42m · Revoke" to show, and whatever the user
   * granted while the mock ran.
   */
  private grants: JsonObject[] = [
    {
      id: "prefix-v1:bun test",
      prefix: "bun test",
      expiresAt: Math.floor(Date.now() / 1000) + 42 * 60,
      remainingSecs: 42 * 60,
    },
  ];

  /** A timed approval the user just gave in the mock: listed and revocable like the engine's. */
  private addGrant(prefix: string) {
    const remainingSecs = 60 * 60;
    const id = `prefix-v1:${prefix}`;
    this.grants = [
      { id, prefix, expiresAt: Math.floor(Date.now() / 1000) + remainingSecs, remainingSecs },
      ...this.grants.filter((grant) => str(grant.id) !== id),
    ];
  }

  private themesSnapshot(): JsonObject {
    return { active: this.activeTheme, themes: this.themes as unknown as Json };
  }

  /** The MCP rows `_rustcode/capabilities` lists; `fagbrev` starts failed until it is restarted. */
  private mcpServers(): JsonObject[] {
    const up = this.mcpRestarted.has("fagbrev");
    return [
      {
        name: "github",
        transport: "http",
        target: "https://api.githubcopilot.com/mcp/",
        enabled: true,
        scope: "user",
        connection: "connected",
        tools: 41,
        error: null,
      },
      {
        name: "sqlite",
        transport: "stdio",
        target: "uvx mcp-server-sqlite",
        enabled: false,
        scope: "user",
        connection: "disabled",
        tools: 0,
        error: null,
      },
      up
        ? {
            name: "fagbrev",
            transport: "stdio",
            target: "npx -y fagbrev-mcp",
            enabled: true,
            scope: "user",
            connection: "connected",
            tools: 3,
            error: null,
          }
        : {
            name: "fagbrev",
            transport: "stdio",
            target: "npx -y fagbrev-mcp",
            enabled: true,
            scope: "user",
            connection: "failed",
            tools: 0,
            error: "MCP server 'fagbrev' failed to start: spawn npx ENOENT",
          },
      ...this.mcpAdded.servers,
    ];
  }

  /** `_rustcode/environment/sync` (`rustcode/engine/src/fleet.rs`): merge a report, answer with ours. */
  private machineSync(ask: JsonObject): JsonObject {
    const from = obj(ask.from);
    const fromId = str(from?.hostId) ?? "";
    for (const raw of [...arr(ask.entries), ...(from ? [from as Json] : [])]) {
      const entry = obj(raw);
      const id = str(entry?.hostId);
      if (!entry || !id) continue;
      if (id === MOCK_LOCAL_HOST.id) {
        const at = str(entry.nameUpdatedAt);
        const name = str(entry.name);
        if (at && name && (!this.ownName || at > this.ownName.at)) this.ownName = { name, at };
        continue;
      }
      if (mockNameWins(entry, this.fleet.get(id)) || !this.fleet.has(id)) {
        const told: JsonObject = { ...entry };
        delete told.via;
        if (fromId && id !== fromId) told.via = fromId;
        this.fleet.set(id, told);
      }
    }
    const host = mockHostEntry({
      ...MOCK_LOCAL_HOST,
      name: this.ownName?.name ?? MOCK_LOCAL_HOST.name,
    });
    return {
      host: this.ownName ? { ...host, nameUpdatedAt: this.ownName.at } : host,
      entries: [...this.fleet.values()],
    };
  }

  /** A restarted engine reads the threads the one before it saved, as the real one reads its folders. */
  adoptThreads(previous: MockEngine): void {
    this.threads = previous.threads;
  }

  constructor(
    private readonly emit: (line: string) => void,
    private readonly options: MockOptions = {},
  ) {
    const scenario = options.scenario ?? "default";
    this.threads =
      scenario === "default"
        ? seedThreads()
        : scenario === "delegated"
          ? seedDelegatedThreads()
          : scenario === "delegates"
            ? seedDelegateTreeThreads()
            : scenario === "branches"
              ? seedBranchThreads()
              : [];
    // The stored book is the only one the app reads at connect; a scenario brings its own.
    if (options.notices === "repeat") seedRepeatedNotices(this.threads);
    if (scenario === "delegates") {
      this.options.links ??= {
        links: seedDelegateLinks() as unknown as Json,
        grants: {},
        counters: {},
        fanOuts: [],
      };
    }
    // A thread of its own for a tool row's hard cases, in front of the scenario's own.
    if (options.tools === "background") this.threads = [seedToolRowsThread(), ...this.threads];
    this.harnesses = withLast(initialHarnesses(scenario !== "onboarding"));
    if (options.remote === "connect") {
      this.remote.enabled = true;
      this.remote.gateway = this.remoteGateway();
      for (const open of this.threads) this.remote.shared.add(open.id);
      this.askToConnect("Lars's Pixel", "android", "192.168.1.31");
    }
    if (options.remote === "refused") {
      this.remote.enabled = true;
      this.remote.gateway = this.remoteGateway();
      for (const open of this.threads) this.remote.shared.add(open.id);
      // One of each refusal the owner can act on, newest first, as the gateway
      // keeps them (`remote_gateway/gateway.rs`).
      this.remote.refusedConnections = [
        {
          reason: "unsupported_version",
          attempts: 3,
          deviceName: "Lars's iPhone",
          deviceKind: "phone",
          platform: "ios",
          // The phone build from #3301: a protocol version this Mac does not speak.
          protocolVersion: 0,
          supportedVersions: [1],
          remoteAddr: "192.168.1.31",
          refusedSecsAgo: 40,
        },
        {
          reason: "pairing_failed",
          attempts: 1,
          deviceName: "Pixel 8",
          deviceKind: "phone",
          platform: "android",
          supportedVersions: [],
          remoteAddr: "192.168.1.72",
          refusedSecsAgo: 190,
        },
        {
          reason: "unauthorized",
          attempts: 1,
          deviceKind: "phone",
          platform: "android",
          supportedVersions: [],
          remoteAddr: "192.168.1.44",
          refusedSecsAgo: 320,
        },
        {
          reason: "unexpected_origin",
          attempts: 2,
          deviceKind: "phone",
          platform: "unknown",
          supportedVersions: [],
          remoteAddr: "192.168.1.72",
          refusedSecsAgo: 500,
        },
      ];
    }
  }

  /** A phone (or a desktop app) on the network asks to connect, as `connect_request` does at the gateway. */
  private askToConnect(
    deviceName: string,
    platform: string,
    remoteAddr: string,
    deviceKind = "phone",
  ): string {
    const id = Math.random().toString(16).slice(2, 18).padEnd(16, "0");
    const now = Date.now();
    this.remote.connect.push({
      id,
      deviceName,
      deviceKind,
      platform,
      remoteAddr,
      createdAt: now,
      expiresAt: now + 120_000,
      state: "pending",
    });
    return id;
  }

  private endConnect(id: string, end: string, deviceId?: string) {
    const request = this.remote.connect.find((entry) => entry.id === id);
    if (!request) return;
    this.remote.connect = this.remote.connect.filter((entry) => entry !== request);
    this.remote.endedConnect.push({ id, deviceName: request.deviceName, end, deviceId, at: Date.now() });
    this.remote.endedConnect = this.remote.endedConnect.slice(-8);
  }

  /** The phone enters the code the app showed: it pairs, as a new device. */
  private enterConnectCode(): boolean {
    const request = this.remote.connect.find((entry) => entry.state === "approved");
    if (!request) return false;
    const deviceId = Math.random().toString(16).slice(2, 10).padEnd(8, "0");
    this.remote.devices.push({
      id: deviceId,
      name: request.deviceName,
      kind: request.deviceKind,
      platform: request.platform,
      lastSeenAt: new Date().toISOString(),
    });
    this.endConnect(request.id, "paired", deviceId);
    return true;
  }

  private connectRequestsJson(): JsonObject {
    const now = Date.now();
    for (const request of [...this.remote.connect]) {
      if (request.expiresAt <= now) this.endConnect(request.id, "expired");
    }
    return {
      connectRequests: this.remote.connect.map((request) => ({
        id: request.id,
        deviceName: request.deviceName,
        deviceKind: request.deviceKind,
        platform: request.platform,
        remoteAddr: request.remoteAddr,
        ageSecs: Math.floor((now - request.createdAt) / 1000),
        expiresInSecs: Math.max(0, Math.floor((request.expiresAt - now) / 1000)),
        state: request.state,
      })),
      endedConnectRequests: this.remote.endedConnect.map((ended) => {
        const entry: JsonObject = {
          id: ended.id,
          deviceName: ended.deviceName,
          end: ended.end,
          endedSecsAgo: Math.floor((now - ended.at) / 1000),
        };
        if (ended.deviceId) entry.deviceId = ended.deviceId;
        return entry;
      }),
    };
  }

  private get delay() {
    return this.options.delayMs ?? 40;
  }

  /**
   * The inbox's demo: a question, an approval and a failed turn on threads the
   * app has not opened, so the list has something to show on first open. Each
   * request is asked like a live one and answered by the app's own cards.
   */
  private seedNeedsYou() {
    const has = (id: string) => this.threads.some((t) => t.id === id);
    // One of each risk level, so the card's tint and preview are visible in the mock:
    // a read-only call, an edit with a diff, and a command that would destroy a tree.
    if (has("mock-thread-1")) {
      void this.ask("session/request_permission", {
        sessionId: "mock-thread-1",
        toolCall: {
          toolCallId: "seed-approval-1",
          title: "read src/ui/Composer.tsx",
          kind: "read",
          rawInput: {
            path: "src/ui/Composer.tsx",
            summary: "Read the composer before changing it",
          },
        },
        options: [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          {
            optionId: "allow_hour",
            name: "Allow for 1 hour `cat src/ui/Composer.tsx`",
            kind: "allow_always",
          },
          { optionId: "always", name: "Always allow `cat src/ui/Composer.tsx`", kind: "allow_always" },
          { optionId: "reject", name: "Reject", kind: "reject_once" },
        ],
      }).then((answer) => {
        // An hour granted here is listed and revocable, like the engine's own.
        if (obj(answer.outcome)?.optionId === "allow_hour") this.addGrant("cat src/ui/Composer.tsx");
      });
    }
    if (has("mock-thread-4")) {
      void this.ask("session/request_permission", {
        sessionId: "mock-thread-4",
        toolCall: {
          toolCallId: "seed-approval-destructive",
          title: "bash",
          kind: "execute",
          rawInput: {
            command: "rm -rf node_modules && bun install",
            cwd: "/Users/you/.config/rustcode/worktrees/rustcode/feat-login-form",
            summary: "Start the dependencies over",
          },
        },
        // A harness that offers grants for a destructive command: the app must
        // show only Allow once and Reject (lib/approval.ts `grantAllowed`).
        options: [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          { optionId: "allow_hour", name: "Allow for 1 hour `rm -rf node_modules`", kind: "allow_always" },
          { optionId: "always", name: "Always allow `rm -rf node_modules`", kind: "allow_always" },
          { optionId: "reject", name: "Reject", kind: "reject_once" },
        ],
      });
    }
    if (has("mock-thread-claude")) {
      void this.ask("session/request_permission", {
        sessionId: "mock-thread-claude",
        toolCall: {
          toolCallId: "seed-question-1",
          title: "Which tone?",
          rawInput: {
            header: "Pricing copy",
            question: "Which tone should the pricing blurbs use?",
            options: ["Plain and short", "Warmer, more human", "Keep the current voice"],
            descriptions: [
              "Three lines, no adjectives",
              "Talks to one reader at a time",
              "Only fix the jargon",
            ],
            multiple: false,
          },
        },
        options: [
          { optionId: "tone-plain", name: "Plain and short", kind: "allow_once" },
          { optionId: "tone-warm", name: "Warmer, more human", kind: "allow_once" },
          { optionId: "tone-keep", name: "Keep the current voice", kind: "allow_once" },
        ],
        _meta: {
          rustcode: {
            kind: "question",
            version: 2,
            questionId: "seed-question-1",
            answerKinds: ["options", "text", "attachments"],
          },
        },
      });
    }
    if (has("mock-thread-4")) {
      void this.ask("session/request_permission", {
        sessionId: "mock-thread-4",
        toolCall: {
          toolCallId: "seed-approval-4",
          title: "edit src/ui/LoginForm.tsx",
          kind: "edit",
          content: [
            {
              type: "diff",
              path: "src/ui/LoginForm.tsx",
              oldText: '  <button type="submit">Sign in</button>',
              newText: '  <button type="submit" disabled={!email}>Sign in</button>',
              _meta: { rustcode: { oldStart: 8, newStart: 8 } },
            },
          ],
          rawInput: { path: "src/ui/LoginForm.tsx", summary: "Disable sign-in until the email is typed" },
        },
        options: [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          { optionId: "always", name: "Always allow edits", kind: "allow_always" },
          { optionId: "reject", name: "Reject", kind: "reject_once" },
        ],
      });
    }
  }

  private sleep(ms = this.delay): Promise<void> {
    return ms > 0 ? new Promise((r) => setTimeout(r, ms)) : Promise.resolve();
  }

  private send(message: JsonObject) {
    this.emit(JSON.stringify({ jsonrpc: "2.0", ...message }));
  }

  private notify(params: JsonObject) {
    this.send({ method: "session/update", params });
  }

  /** Ask the client something and wait for its answer. */
  private ask(method: string, params: JsonObject): Promise<JsonObject> {
    const id = this.nextRpcId++;
    return new Promise((resolve) => {
      this.waiting.set(id, resolve);
      this.send({ id, method, params });
    });
  }

  receive(line: string): void {
    let message: JsonObject;
    try {
      message = JSON.parse(line) as JsonObject;
    } catch {
      return;
    }
    const method = str(message.method);
    const id = message.id as number | string | undefined;
    if (!method) {
      if (typeof id === "number") {
        const waiter = this.waiting.get(id);
        this.waiting.delete(id);
        waiter?.(obj(message.result) ?? {});
      }
      return;
    }
    const params = obj(message.params) ?? {};
    void this.handle(method, params).then(
      (result) => {
        if (id !== undefined) this.send({ id, result });
      },
      (error: Error & { code?: number; data?: Json }) => {
        // `data` travels as the engine sends it: the refusals an orchestration
        // tool maps (`{ reason, message }`, `error.data.reason`) are read there.
        if (id !== undefined)
          this.send({
            id,
            error: {
              code: error.code ?? -32603,
              message: error.message,
              ...(error.data === undefined ? {} : { data: error.data }),
            },
          });
      },
    );
  }

  private thread(id: unknown): MockThread {
    const thread = this.threads.find((t) => t.id === id);
    if (!thread) throw Object.assign(new Error("Unknown session"), { code: -32602 });
    return thread;
  }

  /** The shell's thread actions, on the mock's own threads. */
  renameThread(id: string, title: string, user = false): string {
    const thread = this.thread(id);
    thread.title = title.split(/\s+/).join(" ").trim();
    if (user) this.userTitled.add(id);
    return thread.title;
  }

  /** The threads the user named themselves (`thread_user_titled`). */
  userTitled = new Set<string>();

  trashThread(id: string) {
    this.thread(id);
    this.threads = this.threads.filter((t) => t.id !== id);
  }

  /**
   * The gateway as the engine names it: the phones connected to it and the
   * ones attached to the thread being read, both by name (arrays, as
   * `acp/remote.rs` sends them), and whether anything is connected at all.
   */
  private remoteGateway(): JsonObject {
    const connected = this.options.remote !== "disconnected";
    const phones = connected ? this.remote.devices.map((device) => str(device.name) ?? "phone") : [];
    return {
      connected,
      instanceId: "mock-gateway-instance-1",
      advertisedAddress: "192.168.1.24:17879",
      listenAddress: "192.168.1.24:17879",
      loopbackOnly: false,
      attachedDevices: phones,
      connectedDevices: phones,
      pairedDevices: this.remote.devices.length,
    };
  }

  /** The pairing challenge as the engine's offer: the rows are drawn, the payload is not scannable in a mock. */
  private remoteOffer(): JsonObject | null {
    const stored = obj(this.remote.offer);
    if (!stored) return null;
    const expiresAt = num(stored.expiresAt) ?? 0;
    if (expiresAt <= Date.now()) return null;
    const offer: JsonObject = {
      qrPayload: str(stored.qrPayload) ?? "",
      qrRows: mockQrRows(),
      code: str(stored.code) ?? "",
      address: str(stored.address) ?? "",
      alternates: arr(stored.alternates),
      expiresInSecs: Math.max(0, Math.ceil((expiresAt - Date.now()) / 1000)),
      loopbackOnly: stored.loopbackOnly === true,
      gatewayId: str(stored.gatewayId) ?? "",
      instanceId: str(stored.instanceId) ?? "mock-instance",
      challenge: num(stored.challenge) ?? 0,
    };
    const lockedFor = num(stored.lockedForSecs);
    if (lockedFor !== undefined) offer.lockedForSecs = lockedFor;
    return offer;
  }

  /** The machine's answer: the switch, the gateway, the open challenge and the paired devices. */
  private remoteHost(): JsonObject {
    const stored = obj(this.remote.offer);
    const expiresAt = num(stored?.expiresAt) ?? 0;
    const offer = this.remoteOffer();
    const host: JsonObject = {
      available: true,
      enabled: this.remote.enabled,
      shared: false,
      pairingPending: this.remote.pairingDue,
      pairingExpired: stored !== undefined && expiresAt <= Date.now(),
      pairingError: this.remote.pairingError,
      notShared: this.remote.refused.size,
      pairedDevices: this.remote.devices.length,
      refusedConnections: this.remote.refusedConnections,
      // The machine's answer says which paired devices are connected; the mock's gateway has them all.
      devices: this.remote.devices.map((device) => {
        const connected = this.remote.gateway !== null && this.options.remote !== "disconnected";
        return {
          ...device,
          connected,
          // The gateway classifies the address each device dialled; the mock's phones are on the LAN.
          ...(connected
            ? { routes: [{ path: "lan", localAddress: "192.168.1.24:17879", encrypted: false }] }
            : {}),
        };
      }),
      ...this.connectRequestsJson(),
    };
    if (this.remote.gateway) {
      const gateway: JsonObject = {
        ...this.remote.gateway,
        instanceId: "mock-instance",
        pairedDevices: this.remote.devices.length,
        pairingOpen: offer !== null,
        sessions: this.remote.shared.size,
      };
      const machineOffer = this.remote.machineOffer;
      gateway.machineOffer = {
        open:
          machineOffer && machineOffer.expiresAt > Date.now()
            ? {
                serial: machineOffer.serial,
                expiresInSecs: Math.ceil((machineOffer.expiresAt - Date.now()) / 1000),
                attemptsLeft: 5,
              }
            : null,
        lastEnded: null,
      };
      if (offer) {
        gateway.pairing = {
          expiresInSecs: num(offer.expiresInSecs) ?? 0,
          attemptsLeft: num(stored?.attemptsLeft) ?? 5,
          challenge: num(offer.challenge) ?? 0,
        };
      }
      host.gateway = gateway;
    }
    return host;
  }

  /**
   * The answer `_rustcode/remote` gives: with no session, the machine's; with
   * one it shares, that thread's; with one it does not, the machine's plus the
   * thread and why the host holds it back (`notShared`), as the engine does.
   */
  private remoteStatus(sessionId: string): Json {
    const shared = sessionId !== "" && this.remote.shared.has(sessionId);
    if (!shared) {
      const host = this.remoteHost();
      if (!sessionId) return host;
      const held: JsonObject = { ...host, sessionId };
      const reason = this.remote.refused.get(sessionId);
      if (reason !== undefined) held.notShared = reason;
      return held as Json;
    }
    const stored = obj(this.remote.offer);
    const expiresAt = num(stored?.expiresAt) ?? 0;
    const share: JsonObject = {
      available: true,
      enabled: this.remote.enabled,
      shared: true,
      sessionId,
      target: this.thread(sessionId).harness,
      registrationEpoch: 1,
      offer: this.remoteOffer(),
      pairingPending: this.remote.pairingDue,
      pairingExpired: stored !== undefined && expiresAt <= Date.now(),
      pairedDevices: this.remote.devices.length,
      devices: this.remote.devices,
    };
    if (this.remote.gateway) share.gateway = this.remoteGateway();
    if (this.remote.pairingError) share.pairingError = this.remote.pairingError;
    return share as Json;
  }

  /** A fresh pairing challenge: the QR payload carries the credential, the code is typed by hand. */
  private issueRemoteOffer() {
    const address = "192.168.1.24:17879";
    const alternates = ["100.90.16.237:17879"];
    this.remote.gateway = this.remoteGateway();
    this.remote.offer = {
      qrPayload: JSON.stringify({
        protocol_version: 1,
        address,
        gateway_id: "9f2c1d0b4a6e8c7d5b3a1908f7e6d5c4",
        credential: "jH8sK2mQ9xT4vLpR6nB1yC3fW7gZ0aE5uI2oP8sD4qX",
        host_name: "Mock MacBook Pro",
        alternates,
      }),
      code: "4927-3318",
      address,
      alternates,
      expiresAt: Date.now() + (this.options.remote === "expired" ? -1_000 : 120_000),
      loopbackOnly: false,
      gatewayId: "9f2c1d0b4a6e8c7d5b3a1908f7e6d5c4",
      challenge: 17,
      attemptsLeft: 5,
    };
    this.remote.pairingDue = false;
    this.remote.pairingError = null;
  }

  private snapshot(): Json {
    return this.harnesses as unknown as Json;
  }

  private async handle(method: string, params: JsonObject): Promise<Json> {
    if (method.startsWith("_rustcode/loops/")) return this.loops.answer(method, obj(params) ?? {});
    if (method.startsWith("_rustcode/memory/")) return this.memory.answer(method, obj(params) ?? {});
    if (method === "_rustcode/connections" || method.startsWith("_rustcode/connections/"))
      return this.connections.answer(method, obj(params) ?? {});
    if (isMockTransferMethod(method)) {
      return handleMockTransfer(method, params, {
        getThread: (id) => this.thread(id),
        threads: () => this.threads,
        setThreads: (next) => {
          this.threads = next;
        },
        isRunning: (id) => this.running.has(id),
        harnesses: this.harnesses,
        nextId: () => `mock-imported-${++this.counter}`,
        notify: (msg) => this.notify(msg),
      });
    }
    switch (method) {
      case "initialize": {
        const rc = obj(obj(obj(params.clientCapabilities)?._meta)?.rustcode);
        this.orchestration = typeof rc?.orchestration === "number" ? rc.orchestration : 0;
        return {
          protocolVersion: 1,
          agentCapabilities: {
            loadSession: true,
            promptCapabilities: { image: true, audio: false, embeddedContext: false },
            sessionCapabilities: { list: {}, close: {} },
            _meta: {
              rustcode: {
                steer: 1,
                agents: 2,
                rewind: 1,
                pullRequests: 3,
                sessionMove: 1,
                sessionRepair: 1,
                sessionTransfer: 1,
                checkpoints: 2,
                lineage: 1,
              },
            },
          },
          authMethods: [],
        };
      }
      // The catalog, for a thread that has no session yet to send it on.
      case "_rustcode/commands":
        return { sessionUpdate: "available_commands_update", availableCommands: COMMANDS };
      case "session/list": {
        if (this.options.needsYou && !this.needsYouSeeded) {
          this.needsYouSeeded = true;
          setTimeout(() => this.seedNeedsYou(), 500);
        }
        return {
          sessions: this.threads
            .filter((t) => t.history.length > 0)
            .map((t) => ({
              sessionId: t.id,
              cwd: t.cwd,
              title: t.title ?? null,
              updatedAt: new Date(t.updatedAt).toISOString(),
              _meta: {
                model: t.model,
                rustcode: {
                  harness: { id: t.harness },
                  ...(t.account ? { account: t.account } : {}),
                  ...(t.status ? { status: t.status } : {}),
                  ...(t.pinned ? { pinned: true } : {}),
                  ...(t.parent ? { parent: mockParent(t.parent) } : {}),
                  ...(t.worktree ? { worktree: t.worktree as unknown as Json } : {}),
                  // The project's git remote, when its machine reports one: what finds the same
                  // project on another computer (state/projects.ts).
                  ...(mockRemote(t.cwd) ? { remote: mockRemote(t.cwd) } : {}),
                },
              },
            })),
        };
      }
      case "session/new": {
        const meta = obj(obj(params._meta)?.rustcode);
        const harness = str(meta?.harness) ?? this.harnesses.default.harness;
        const h = this.harnesses.harnesses.find((x) => x.id === harness);
        if (!h) throw Object.assign(new Error(`Unknown harness ${harness}`), { code: -32602 });
        const account = str(meta?.account);
        if (account && !(h.accounts ?? []).some((a) => a.id === account))
          throw Object.assign(new Error(`unknown ${harness} account '${account}'`), { code: -32602 });
        const wanted = obj(meta?.worktree);
        let worktree: MockWorktree | undefined;
        if (wanted) {
          const repo = str(wanted.repo) ?? str(params.cwd) ?? "/";
          worktree = this.makeWorktree(repo, str(wanted.branch), str(wanted.title));
        }
        // The thread an agent starts this one from: like the engine, keep it so
        // pruning the link book cannot make the child a top-level thread.
        const parent = str(meta?.parent);
        const parentTitle = this.threads.find((t) => t.id === parent)?.title;
        const relationship = str(meta?.relationship) ?? "delegate";
        const thread: MockThread = {
          id: `mock-${Date.now().toString(36)}-${++this.counter}`,
          cwd: worktree?.path ?? str(params.cwd) ?? "/",
          worktree,
          checkpoints: [],
          updatedAt: Date.now(),
          harness,
          ...(account ? { account } : {}),
          ...(parent ? { parent: { id: parent, title: parentTitle, relationship } } : {}),
          model:
            str(meta?.model) ??
            h.last?.model ??
            (harness === this.harnesses.default.harness ? this.harnesses.default.model : null) ??
            (harness === "rustcode" ? "chatgpt/gpt-6-luna" : (HARNESS_MODELS[harness]?.[0]?.id ?? "default")),
          // Like the real engine, the built-in harness takes its effort from config, not from `session/new`.
          effort:
            (harness === "rustcode" ? undefined : str(meta?.effort)) ??
            h.last?.effort ??
            this.harnesses.default.effort ??
            "medium",
          history: [],
        };
        this.threads.unshift(thread);
        queueMicrotask(() =>
          this.notify(
            update(thread.id, { sessionUpdate: "available_commands_update", availableCommands: COMMANDS }),
          ),
        );
        return {
          sessionId: thread.id,
          configOptions: configOptions(thread, this.harnesses),
          _meta: {
            rustcode: {
              harness: { id: harness, capabilities: h.capabilities as unknown as Json },
              ...(account ? { account: { id: account } } : {}),
              ...(worktree ? { worktree: worktree as unknown as Json } : {}),
            },
          },
        };
      }
      case "session/load": {
        const thread = this.thread(params.sessionId);
        for (const line of thread.history) this.notify(line);
        this.notify(
          update(thread.id, { sessionUpdate: "available_commands_update", availableCommands: COMMANDS }),
        );
        this.notify(
          update(thread.id, {
            sessionUpdate: "usage_update",
            used: 18_400,
            size: 272_000,
            cost: { amount: 0.0123, currency: "USD" },
            _meta: {
              rustcode: {
                estimated: true,
                inputTokens: 90_000,
                outputTokens: 3_200,
                cachedTokens: 61_000,
                context: mockContext(18_400),
              },
            },
          }),
        );
        const h = this.harnesses.harnesses.find((x) => x.id === thread.harness);
        // Like the engine before a harness ever listed its models: a restored external thread knows
        // only its own model, until `_rustcode/harnesses/models` ran.
        const unlisted = !!h && !h.builtin && !h.models.length;
        return {
          configOptions: unlisted
            ? [
                {
                  id: "model",
                  name: "Model",
                  category: "model",
                  type: "select",
                  currentValue: thread.model,
                  options: [{ value: thread.model, name: thread.model }],
                },
              ]
            : configOptions(thread, this.harnesses),
          _meta: {
            rustcode: {
              harness: { id: thread.harness, capabilities: (h?.capabilities ?? {}) as Json },
              ...(thread.account ? { account: { id: thread.account } } : {}),
            },
          },
        };
      }
      case "session/close":
        return {};
      case "_rustcode/attachments/read": {
        // A kept image reads back as a picture; any other path is gone, as on an engine whose
        // attachment was cleaned away.
        const path = str(params.path) ?? "";
        if (!path.includes("/attachments/")) throw new Error("that image is not kept on this computer");
        const svg =
          '<svg xmlns="http://www.w3.org/2000/svg" width="480" height="320"><rect width="480" height="320" fill="#5b8def"/><circle cx="360" cy="90" r="40" fill="#ffd166"/><path d="M0 320 L160 150 L300 320 Z" fill="#2a4d8f"/></svg>';
        return { mimeType: "image/svg+xml", size: svg.length, offset: 0, data: btoa(svg), done: true };
      }
      case "_rustcode/session/move": {
        const thread = this.thread(params.sessionId);
        const cwd = str(params.cwd);
        if (!cwd?.startsWith("/")) throw new Error("cwd must be absolute");
        thread.cwd = cwd;
        return { cwd };
      }
      case "_rustcode/session/repair": {
        const thread = this.thread(params.sessionId);
        return { cwd: thread.cwd, fixed: [], problems: [] };
      }
      case "session/set_config_option": {
        const thread = this.thread(params.sessionId);
        const value = str(params.value);
        const before = { model: thread.model, effort: thread.effort };
        if (params.configId === "model" && value) thread.model = value;
        if (params.configId === "effort" && value) thread.effort = value;
        // Like the engine: a change after the first message marks the place.
        if (thread.history.length && (before.model !== thread.model || before.effort !== thread.effort)) {
          const last = thread.history.at(-1) as
            | {
                update?: {
                  _meta?: { rustcode?: { kind?: string; model_from?: string; effort_from?: string } };
                };
              }
            | undefined;
          const pending = last?.update?._meta?.rustcode;
          const from =
            pending?.kind === "switch" ? { model: pending.model_from, effort: pending.effort_from } : before;
          if (pending?.kind === "switch") thread.history.pop();
          const divider = update(thread.id, {
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: "Changed the model or effort" },
            _meta: {
              rustcode: {
                kind: "switch",
                model_from: from.model ?? null,
                model_to: thread.model,
                effort_from: from.effort ?? null,
                effort_to: thread.effort,
              },
            },
          });
          if (from.model !== thread.model || from.effort !== thread.effort) thread.history.push(divider);
          queueMicrotask(() => this.notify(divider));
        }
        // Like the engine: a thread's pick is also its harness's next new-thread choice.
        const picked = this.harnesses.harnesses.find((x) => x.id === thread.harness);
        if (picked && (params.configId === "model" || params.configId === "effort")) {
          picked.last = { model: thread.model, effort: thread.effort };
        }
        return { configOptions: configOptions(thread, this.harnesses) };
      }
      case "session/cancel":
      case "_rustcode/session/stop": {
        const run = this.running.get(str(params.sessionId) ?? "");
        if (run) run.cancelled = true;
        return method === "session/cancel" ? null : { route: "builtin", active: !!run };
      }
      case "session/prompt":
        return this.prompt(params);
      case "_rustcode/steer": {
        const run = this.running.get(str(params.sessionId) ?? "");
        if (!run) return { accepted: false };
        run.steers.push(str(params.text) ?? "");
        return { accepted: true };
      }
      case "_rustcode/mcp/app": {
        // The mock has no MCP servers: no tool declares an app, and none can be called.
        if (params.action === "read") return { app: null };
        throw Object.assign(new Error("the MCP server that ran this call is not connected"), {
          code: -32602,
        });
      }
      case "_rustcode/agents": {
        const sid = str(params.sessionId) ?? "";
        const list = this.agents.get(sid) ?? [];
        const action = str(params.action);
        if (action === "list") return { agents: list };
        const agent = list.find((a) => a.id === params.id);
        if (!agent) throw Object.assign(new Error(`unknown subagent: ${params.id}`), { code: -32602 });
        if (action === "history") {
          return {
            agent,
            updates: [
              { sessionUpdate: "user_message_chunk", content: text(String(agent.task)) },
              {
                sessionUpdate: "tool_call",
                toolCallId: `a${agent.id}-1`,
                title: "rg",
                kind: "search",
                status: "completed",
                rawInput: { pattern: "Composer", summary: "Find the composer" },
              },
              ...(agent.status === "completed"
                ? [{ sessionUpdate: "agent_message_chunk", content: text(String(agent.completion)) }]
                : []),
            ],
          };
        }
        if (action === "cancel") {
          agent.status = "cancelled";
          agent.finishedAtMs = Date.now();
          return { settled: true };
        }
        if (action === "send") return { queued: false };
        throw Object.assign(new Error("bad action"), { code: -32602 });
      }
      case "_rustcode/data/usage": {
        // The same shape engine/src/acp/data.rs answers with.
        const ask = params as { groupBy?: string; since?: string } | undefined;
        return mockUsage(this.threads, ask?.groupBy ?? "harness", ask?.since);
      }
      case "_rustcode/data/query":
        if (String(obj(params)?.sql ?? "").includes("operational_events")) {
          const at = (ago: number) => new Date(Date.now() - ago).toISOString();
          return {
            columns: ["at", "event_type", "thread_id", "fields_json"],
            rows: [
              {
                at: at(40_000),
                event_type: "turn.summary",
                thread_id: "t-login",
                fields_json: '{"rounds":6,"tool_calls":11}',
              },
              {
                at: at(95_000),
                event_type: "tools.batch.finish",
                thread_id: "t-login",
                fields_json: '{"calls":3,"failed":1}',
              },
              {
                at: at(120_000),
                event_type: "turn.start",
                thread_id: "t-login",
                fields_json: '{"wakeup":false}',
              },
            ],
            truncated: false,
          };
        }
        return {
          columns: ["tool_name", "failures", "calls"],
          rows: [
            { tool_name: "run_command", failures: 19, calls: 240 },
            { tool_name: "replace_file_content", failures: 6, calls: 310 },
          ],
          truncated: false,
        };
      case "_rustcode/data/activity": {
        // The same shape engine/src/acp/data.rs answers with: summaries only.
        const at = new Date(Date.now() - 60_000).toISOString();
        const items: JsonObject[] = [
          { source: "event", at, type: "turn.start", fields: { tool_rounds: 0 } },
          {
            source: "provider_request",
            at,
            provider: "openai",
            model: "gpt-6-luna",
            status_code: 429,
            error_kind: "rate_limit",
            latency_ms: 412,
            input_tokens: 1200,
            output_tokens: 80,
            attempt: 1,
          },
        ];
        if (params.order === "newest") items.reverse();
        return {
          session_id: str(params.sessionId) ?? "",
          items,
          next: null,
          has_more: false,
          last_provider_error: { at, status_code: 429, error_kind: "rate_limit" },
        };
      }
      case "_rustcode/data/links/load":
        // The same shape `_rustcode/data/links/load` answers with (engine/src/acp/data.rs).
        return {
          stored: this.linkBook !== undefined,
          book: this.linkBook ?? { links: [], grants: {}, counters: {}, fanOuts: [] },
        };
      case "_rustcode/data/receipts/get": {
        // The same shape engine/src/acp/data.rs answers with.
        const key = receiptKey(params);
        const stored = obj(this.receipts[key]);
        if (!stored) return { state: "none" };
        if (stored.action !== str(obj(params)?.action) || stored.argsHash !== str(obj(params)?.argsHash)) {
          return { state: "conflict" };
        }
        return { state: "replayed", result: stored.result ?? {}, created_at: stored.createdAt ?? "" };
      }
      case "_rustcode/data/receipts/put": {
        const key = receiptKey(params);
        const result = obj(obj(params)?.result);
        if (!result) throw Object.assign(new Error("result must be a JSON object"), { code: -32602 });
        // The first receipt under a key stays, as the store keeps it.
        this.receipts[key] ??= {
          action: str(obj(params)?.action) ?? "",
          argsHash: str(obj(params)?.argsHash) ?? "",
          result,
          createdAt: new Date().toISOString(),
        };
        return { saved: true };
      }
      case "_rustcode/data/links/save": {
        const book = obj(params)?.book;
        if (!book || typeof book !== "object" || Array.isArray(book)) {
          throw Object.assign(new Error("book is required"), { code: -32602 });
        }
        this.linkBook = book as JsonObject;
        return { saved: true };
      }
      case "_rustcode/usage/shared":
        return mockSharedPlans();
      case "_rustcode/usage/plans": {
        const at = (h: number) => Math.floor(Date.now() / 1000) + h * 3600;
        const p = params as
          | {
              antigravity?: boolean;
              claude?: boolean;
              claudeAccounts?: boolean;
              claudeAccount?: string;
              force?: boolean;
            }
          | undefined;
        // `?claudeUsage=expired|refused|denied` stages a sign-in RustCode could
        // not read, so Settings › Usage's expired and denied rows can be seen
        // (and its Refresh tried) without a Keychain. A forced read answers the
        // same: the real engine re-reads and only then knows it is still broken.
        const failure = this.options.claudeUsage;
        // A lapsed access token that RustCode answered with the numbers it last
        // read for the login: the row draws them, marked with when.
        const recorded: JsonObject =
          failure === "recorded" ? { asOf: Math.floor(Date.now() / 1000) - 3 * 3600 } : {};
        const failed = (reason: Exclude<NonNullable<MockOptions["claudeUsage"]>, "recorded">) => ({
          id: "claude",
          name: "Claude",
          via: "Claude Code",
          status: "unavailable",
          reason,
          error:
            reason === "expired"
              ? "Run a Claude Code thread on this account to refresh its usage."
              : reason === "refused"
                ? "Claude Code's sign-in was refused. Sign in again in Claude Code, then Refresh."
                : reason === "forbidden"
                  ? "Anthropic does not allow this sign-in to read plan usage. Press Refresh to try once more."
                  : "macOS did not allow reading Claude Code's sign-in from the Keychain.",
          windows: [],
        });
        // One row per login, as the engine answers it: the default login when
        // asked for, and each account the client names (all of them on an
        // explicit look in Settings › Usage, else the one the app asks for).
        // Naming an account is independent of `claude`, so a background read of
        // one account leaves the default login unread (engine `acp/usage.rs`).
        const accounts = [
          {
            id: "paral-2",
            name: "Paral 2",
            windows: [
              { label: "5-hour", usedPercent: 4, windowMinutes: 300, resetsAt: at(4) },
              { label: "Weekly", usedPercent: 12, windowMinutes: 10080, resetsAt: at(90) },
            ],
          },
        ].filter((a) => (p?.claude && p?.claudeAccounts) || a.id === p?.claudeAccount);
        return {
          plans: [
            p?.claude
              ? failure && failure !== "recorded"
                ? failed(failure)
                : {
                    id: "claude",
                    name: "Claude",
                    via: "Claude Code",
                    status: "ok",
                    plan: "max",
                    ...recorded,
                    windows: [
                      { label: "5-hour", usedPercent: 18, windowMinutes: 300, resetsAt: at(3) },
                      { label: "Weekly", usedPercent: 46, windowMinutes: 10080, resetsAt: at(80) },
                      { label: "Weekly · Opus", usedPercent: 83, windowMinutes: 10080, resetsAt: at(80) },
                    ],
                  }
              : { id: "claude", name: "Claude", via: "Claude Code", status: "notAsked", windows: [] },
            ...accounts.map((a) => ({
              id: `claude:${a.id}`,
              name: a.name,
              via: "Claude Code",
              harness: "claude-code",
              account: a.id,
              status: "ok",
              plan: "max",
              ...recorded,
              windows: a.windows,
            })),
            {
              id: "codex",
              name: "ChatGPT",
              via: "Codex CLI",
              status: "ok",
              windows: [
                { label: "5-hour", usedPercent: 42, windowMinutes: 300, resetsAt: at(2) },
                { label: "Weekly", usedPercent: 71, windowMinutes: 10080, resetsAt: at(72) },
              ],
            },
            ...(p?.antigravity ? [mockAntigravityPlan(at)] : []),
          ],
        };
      }
      case "_rustcode/usage/limits": {
        // A thread's own windows, as the engine answers them: Claude Code's are
        // null (only its sign-in's plan read has them), so the composer meter
        // reads the account's row from `_rustcode/usage/plans`.
        const sessionId = str(obj(params)?.sessionId);
        const thread = this.threads.find((t) => t.id === sessionId);
        if (thread?.harness === "claude-code") return null;
        return {
          source: "codex",
          primary: { usedPercent: 42, windowMinutes: 300, resetsAt: Math.floor(Date.now() / 1000) + 7200 },
          secondary: {
            usedPercent: 71,
            windowMinutes: 10080,
            resetsAt: Math.floor(Date.now() / 1000) + 3 * 86400,
          },
        };
      }
      case "_rustcode/pulls/watches": {
        const thread = this.threads[0];
        if (!thread || typeof location === "undefined" || !location.search.includes("pullWatchPreview"))
          return { watches: [] };
        const watch = { sessionId: thread.id, number: 3375, link: true };
        if (obj(params)?.take) {
          const events = this.pullEventDelivered
            ? []
            : [{ message: "CI checks failed", kind: "checks_failed" }];
          this.pullEventDelivered = true;
          return { watch, events };
        }
        return { watches: [watch] };
      }
      case "_rustcode/workflows/list":
        return { runs: [] };
      case "_rustcode/workflows/resume":
        return {
          run: { run_id: str(obj(params)?.runId) ?? "", status: "running", driven: true },
        };
      case "_rustcode/workflows/retry":
        return {
          run: { run_id: str(obj(params)?.runId) ?? "", status: "running", driven: true },
        };
      case "_rustcode/workflows/stop":
        // The mock has no runs of its own: a staged run (state/workflows.ts) is simply stopped.
        return {
          run: { run_id: str(obj(params)?.runId) ?? "", status: "cancelled", finished_at: Date.now() },
        };
      case "_rustcode/workflows/wake":
        return { claimed: false };
      case "_rustcode/environment/sync":
        return this.machineSync(obj(params) ?? {});
      case "_rustcode/pulls":
        return { list: { provider: "github", hasMore: false, pulls: MOCK_PULLS } };
      case "_rustcode/project/read": {
        // The engine's `_rustcode/project/read`: a folder's git facts.
        const path = str(obj(params)?.path) || "/Users/me/code/rustcode";
        return {
          path,
          name: path.split("/").filter(Boolean).pop() ?? path,
          is_git: true,
          remote: "https://github.com/LHagfoss/rustcode.git",
          // The website checkout sits on a feature branch, so a row shows it in place of a path.
          branch: path.endsWith("/website") ? "redesign/pricing-page" : "main",
          default_branch: "main",
        };
      }
      case "_rustcode/project/create":
        return mockCreate(obj(params) ?? {});
      case "_rustcode/project/browse":
        return mockBrowse(obj(params) ?? {});
      case "_rustcode/project/clone": {
        const ask = obj(params) ?? {};
        const parent = str(ask.parent) || "/Users/me/code";
        const name =
          str(ask.name) ||
          String(str(ask.url) ?? "cloned")
            .replace(/\/+$/, "")
            .split(/[/:]/)
            .pop()!
            .replace(/\.git$/, "");
        return {
          path: `${parent}/${name}`,
          name,
          is_git: true,
          remote: str(ask.url) ?? null,
          branch: "main",
          default_branch: "main",
          created: true,
        };
      }
      case "_rustcode/scheduled/list":
        return {
          daemon_running: true,
          tasks: [
            {
              id: "task-morning",
              name: "Morning build check",
              paused: false,
              schedule: { kind: "cron", expression: "0 9 * * 1-5", timezone: "Europe/Oslo" },
              workspace: "/Users/me/code/rustcode",
              next_due_at: new Date(new Date().setHours(9, 0, 0, 0) + 86_400_000).toISOString(),
            },
          ],
        };
      case "_rustcode/scheduled/create": {
        const ask = obj(params) ?? {};
        const prompt = str(ask.prompt) ?? "Scheduled work";
        return {
          daemon_running: true,
          task: {
            id: `task-${this.counter}`,
            name: str(ask.name) || prompt.split("\n")[0]!.slice(0, 60),
            paused: false,
            schedule: ask.schedule ?? { kind: "cron", expression: "0 9 * * *" },
            workspace: str(ask.workspace) ?? "",
            next_due_at: new Date(Date.now() + 86_400_000).toISOString(),
          },
        };
      }
      case "_rustcode/scheduled/delete":
        return this.loops.forget(str(obj(params)?.id) ?? "");
      case "_rustcode/scheduled/run":
        return this.loops.run(str(obj(params)?.id) ?? "");
      case "_rustcode/pulls/state": {
        const FORK_PULL = 999;
        // A project whose remote is on another forge: `gh` knows no pull requests there.
        if (str(obj(params)?.cwd)?.includes("/gitlab-")) {
          return {
            error: {
              code: "no_provider",
              message: "none of the git remotes configured for this repository point to a known GitHub host",
            },
          };
        }
        const number = num(obj(params)?.number);
        if (number === FORK_PULL) {
          return {
            branch: null,
            pull: null,
            error: {
              code: "not_allowed",
              message: "pull request #999 is not from a branch of this repository",
            },
          };
        }
        const branch =
          number && number !== 2790
            ? "chore/release-v9.9.9"
            : str(obj(params)?.branch) || "feat/delegate-pull";
        return {
          branch,
          pull: {
            number: number ?? 2790,
            title:
              number && number !== 2790
                ? "chore(release): v9.9.9"
                : "feat(mcp): report and finish delegate pull requests",
            state: "open",
            merged: false,
            draft: false,
            url: "https://github.com/LHagfoss/rustcode/pull/2790",
            branch,
            headSha: "58961fc5aa",
            ci: "pending",
            failing: [],
            pending: 1,
            passed: 5,
          },
        };
      }
      case "_rustcode/pulls/merge/cancel":
        return { cancelled: false };
      case "_rustcode/pulls/merge":
        // Long enough for the panel's "Waiting for checks" to show.
        if (obj(params)?.mergeId) await new Promise((r) => setTimeout(r, 2500));
        return {
          merge: {
            number: num(obj(params)?.number) ?? 2790,
            state: "merged",
            merged: true,
            failing: [],
            url: "https://github.com/LHagfoss/rustcode/pull/2790",
            waitedSecs: 42,
            message: "squash-merged and the branch was deleted",
          },
        };
      case "_rustcode/worktrees/cleanup":
        return {
          cleanup: {
            removed: true,
            branch: str(obj(params)?.branch) ?? "feat/delegate-pull",
            branch_removed: true,
            base: "main",
            commit_count: 0,
          },
        };
      case "_rustcode/textGeneration/configure": {
        const action = str(params.action);
        const generators = arr(this.textGenerators.generators);
        if (action === "set" && obj(params.generator)) generators.push(params.generator ?? null);
        if (action === "select") this.textGenerators.selected = params.id ?? null;
        this.textGenerators.generators =
          action === "remove" ? generators.filter((g) => obj(g)?.id !== params.id) : generators;
        if (action === "remove" && this.textGenerators.selected === params.id)
          this.textGenerators.selected = null;
        return this.textGenerators;
      }
      case "_rustcode/voice/transcribe":
        // Canned text, so the dictation flow works without an engine or a key.
        if (!str(params.generatorId)) throw new Error("No transcription provider");
        if (params.cleanup === true)
          return this.textGenerators.selected
            ? { text: "Summarize what changed in this branch since main.", cleanup: "applied" }
            : { text: "um summarize what changed in this branch since main", cleanup: "unavailable" };
        return { text: "Summarize what changed in this branch since main." };
      case "_rustcode/textGeneration/generate":
        if (!this.textGenerators.selected) return { text: null };
        return {
          text: "feat(desktop): improve workspace navigation\n\nMake the workspace easier to navigate.",
        };
      case "_rustcode/git/state":
        return mockGit(this.threads, str(params.path) ?? "") as unknown as Json;
      case "_rustcode/git/run": {
        const path = str(params.path) ?? "";
        const git = mockGit(this.threads, path);
        const action = str(params.action);
        if (
          (action === "commit" || action === "commit_push") &&
          git.onDefault &&
          !params.newBranch &&
          !params.allowDefaultBranch
        )
          throw Object.assign(new Error("default_branch"), {
            code: -32602,
            data: {
              reason: "default_branch",
              message: `This commits to ${git.branch}. Confirm it, or commit to a new branch.`,
            },
          });
        if (action === "commit" || action === "commit_push") {
          const picked = new Set(Array.isArray(params.files) ? params.files : []);
          if (str(params.newBranch))
            Object.assign(git, { branch: str(params.newBranch), onDefault: false, upstream: null });
          git.files = git.files.filter((f) => !picked.has(f.path));
          git.ahead += 1;
        }
        if (action === "push" || action === "commit_push") {
          git.upstream = `origin/${git.branch}`;
          git.ahead = 0;
        }
        if (action === "pull") git.behind = 0;
        let url: string | null = null;
        if (action === "create_pr") {
          git.upstream = `origin/${git.branch}`;
          git.ahead = 0;
          url = "https://github.com/LHagfoss/rustcode/pull/4242";
        }
        return { action, output: "", url, state: git } as unknown as Json;
      }
      case "_rustcode/pulls/branch":
        return {
          branch: {
            branch: "feat/mock-pull",
            head_ref: "feat/mock-pull",
            cross_repository: false,
            read_only: false,
          },
        };
      case "_rustcode/worktrees/state": {
        const paths = Array.isArray(params.paths) ? params.paths : [];
        return {
          worktrees: paths.map((path) => {
            const t = this.threads.find((x) => x.worktree?.path === path);
            if (!t?.worktree) return { path, exists: false } as unknown as Json;
            return {
              path,
              exists: true,
              branch: t.worktree.branch,
              against: "origin/main",
              ahead: t.worktree.ahead ?? 1,
              behind: t.worktree.behind ?? 0,
              dirty: t.worktree.dirty ? 1 : 0,
              last_commit: new Date(Date.now() - 12 * 60_000).toISOString(),
              last_commit_subject: "feat(desktop): the delegate's last commit",
            } as unknown as Json;
          }),
        };
      }
      case "_rustcode/worktrees/handoff": {
        const sessionId = str(params.sessionId ?? params.session_id);
        const t = this.threads.find((x) => x.id === sessionId);
        if (!t) throw Object.assign(new Error(`unknown session: ${sessionId}`), { code: -32602 });
        const to = str(params.to) ?? "checkout";
        if (to === "checkout") {
          if (!t.worktree) {
            throw Object.assign(new Error("this thread does not run in a worktree"), {
              code: -32000,
              data: { reason: "no_worktree" },
            });
          }
          if (t.worktree.dirty) {
            throw Object.assign(new Error("the worktree has uncommitted changes"), {
              code: -32000,
              data: { reason: "dirty" },
            });
          }
          const repo = t.worktree.repo;
          const branch = t.worktree.branch;
          const path = t.worktree.path;
          this.removedWorktrees.add(path);
          t.worktree = undefined;
          t.cwd = repo;
          return {
            session_id: sessionId,
            to,
            moved: true,
            worktree_removed: true,
            switched: true,
            branch,
            path: repo,
          } as unknown as Json;
        }
        if (t.worktree) {
          throw Object.assign(new Error("this thread already runs in a worktree"), {
            code: -32000,
            data: { reason: "has_worktree" },
          });
        }
        const worktree = this.makeWorktree(t.cwd, undefined, t.title);
        t.worktree = worktree;
        t.cwd = worktree.path;
        return {
          session_id: sessionId,
          to,
          moved: true,
          path: worktree.path,
          branch: worktree.branch,
          repo: worktree.repo,
          base: worktree.base,
          checkout_branch: "main",
        } as unknown as Json;
      }
      case "_rustcode/pulls/view": {
        // The row the list shows, so the detail agrees with it; a number the list
        // does not have opens as an open pull with its checks still running.
        const row = MOCK_PULLS.find((pull) => pull.number === params.number);
        return {
          pull: {
            title: "feat(desktop): Tauri desktop app on every harness",
            author: "LHagfoss",
            state: "open",
            draft: false,
            checks: { passed: 5, failed: 0, pending: 1, skipped: 0 },
            headRef: "feat/desktop-tauri-harnesses",
            baseRef: "main",
            headSha: "58961fc5aa",
            additions: 9120,
            deletions: 12,
            changedFiles: 64,
            ...row,
            number: params.number as number,
            url: `https://github.com/LHagfoss/rustcode/pull/${params.number}`,
            body: "Adds `apps/desktop`: a Tauri shell around `rustcode --acp`.\n\n- Composer, approvals, harness pickers\n- Link chips and Smart UI",
            checkRuns: MOCK_CHECK_RUNS[params.number as number] ?? [
              { name: "desktop / check", status: "passed" },
              { name: "lint", status: "passed" },
              { name: "label", status: "passed" },
              { name: "Desktop UI", status: "passed" },
              { name: "rust / test", status: "passed" },
              // A matrix job GitHub skipped before it expanded: the template stays in its name.
              { name: ["Desktop shell (", "$", "{{ matrix.name }})"].join(""), status: "pending" },
            ],
            files: [
              { path: "apps/desktop/src/ui/Composer.tsx", additions: 4, deletions: 2 },
              { path: "apps/desktop/src/state/draft.ts", additions: 2, deletions: 1 },
              { path: "apps/desktop/src/generated/models.ts", additions: 450, deletions: 0 },
              { path: "apps/desktop/public/icon.png", additions: 0, deletions: 0 },
            ],
            activity: [
              {
                author: "reviewer",
                kind: "CHANGES_REQUESTED",
                at: new Date(minutes(45)).toISOString(),
                body: "A few things before this lands: the focus ring and the draft helper.",
              },
              {
                author: "reviewer",
                kind: "COMMENTED",
                at: new Date(minutes(40)).toISOString(),
                body: "Looks good so far.",
              },
            ],
            // Every state of a conversation: open with a reply, resolved, outdated, on a range,
            // on the base's side of a removed line.
            reviewComments: [
              {
                id: 101,
                author: "reviewer",
                path: "apps/desktop/src/ui/Composer.tsx",
                line: 42,
                side: "RIGHT",
                outdated: false,
                at: new Date(minutes(35)).toISOString(),
                body: "Restore the focus ring after sending.",
                url: `https://github.com/LHagfoss/rustcode/pull/${params.number}#discussion_r101`,
              },
              {
                id: 103,
                author: "LHagfoss",
                path: "apps/desktop/src/ui/Composer.tsx",
                line: 42,
                side: "RIGHT",
                outdated: false,
                at: new Date(minutes(30)).toISOString(),
                body: "Good catch, the `ring` class is dropped on blur. Fixing.",
                replyTo: 101,
              },
              {
                id: 102,
                author: "reviewer",
                path: "apps/desktop/src/ui/Composer.tsx",
                line: 18,
                side: "RIGHT",
                outdated: true,
                at: new Date(minutes(38)).toISOString(),
                body: "This import is unused.",
              },
              {
                id: 104,
                author: "reviewer",
                path: "apps/desktop/src/ui/Composer.tsx",
                line: 2,
                side: "RIGHT",
                outdated: false,
                at: new Date(minutes(50)).toISOString(),
                body: "Import `compose` from the barrel instead.",
              },
              {
                id: 107,
                author: "LHagfoss",
                path: "apps/desktop/src/ui/Composer.tsx",
                line: 2,
                side: "RIGHT",
                outdated: false,
                at: new Date(minutes(48)).toISOString(),
                body: "Done.",
                replyTo: 104,
              },
              {
                id: 105,
                author: "reviewer",
                path: "apps/desktop/src/state/draft.ts",
                line: 3,
                startLine: 1,
                side: "RIGHT",
                outdated: false,
                at: new Date(minutes(33)).toISOString(),
                body: "Name these after what they hold:\n\n```ts\nexport const draftKey = …\n```",
              },
              {
                id: 106,
                author: "maintainer",
                path: "apps/desktop/src/ui/Composer.tsx",
                line: 3,
                side: "LEFT",
                outdated: false,
                at: new Date(minutes(31)).toISOString(),
                body: "Keep the null composer for the tests.",
              },
            ],
            reviewThreads: [
              {
                id: "PRRT_101",
                resolved: this.pullResolved.get("PRRT_101") ?? false,
                outdated: false,
                rootComment: 101,
              },
              {
                id: "PRRT_102",
                resolved: this.pullResolved.get("PRRT_102") ?? false,
                outdated: true,
                rootComment: 102,
              },
              {
                id: "PRRT_104",
                resolved: this.pullResolved.get("PRRT_104") ?? true,
                outdated: false,
                rootComment: 104,
              },
              {
                id: "PRRT_105",
                resolved: this.pullResolved.get("PRRT_105") ?? false,
                outdated: false,
                rootComment: 105,
              },
              {
                id: "PRRT_106",
                resolved: this.pullResolved.get("PRRT_106") ?? false,
                outdated: false,
                rootComment: 106,
              },
            ],
          },
        };
      }
      case "_rustcode/pulls/diff":
        return {
          diff: [
            "diff --git a/apps/desktop/src/ui/Composer.tsx b/apps/desktop/src/ui/Composer.tsx",
            "index 1111111..2222222 100644",
            "--- a/apps/desktop/src/ui/Composer.tsx",
            "+++ b/apps/desktop/src/ui/Composer.tsx",
            "@@ -1,4 +1,5 @@",
            ' import { useApp } from "../state/store";',
            '+import { compose } from "../state/compose";',
            " ",
            "-export const Composer = () => null;",
            "+export const Composer = () => <div className={compose()} />;",
            "@@ -38,6 +39,8 @@ function send() {",
            "   const text = field.value;",
            "   submit(text);",
            "-  field.blur();",
            '+  field.value = "";',
            "+  field.focus();",
            "+  restoreRing(field);",
            "   return true;",
            " }",
            "diff --git a/apps/desktop/src/state/drafts.ts b/apps/desktop/src/state/draft.ts",
            "similarity index 88%",
            "rename from apps/desktop/src/state/drafts.ts",
            "rename to apps/desktop/src/state/draft.ts",
            "index 3333333..4444444 100644",
            "--- a/apps/desktop/src/state/drafts.ts",
            "+++ b/apps/desktop/src/state/draft.ts",
            "@@ -1,3 +1,4 @@",
            '-export const KEY = "rustcode.drafts";',
            '+export const draftKey = "rustcode.drafts";',
            "+export const draftLimit = 200;",
            ' export const empty = "";',
            " export const seen = new Set<string>();",
            "diff --git a/apps/desktop/src/generated/models.ts b/apps/desktop/src/generated/models.ts",
            "new file mode 100644",
            "index 0000000..5555555",
            "--- /dev/null",
            "+++ b/apps/desktop/src/generated/models.ts",
            "@@ -0,0 +1,450 @@",
            ...Array.from({ length: 450 }, (_, i) => `+export const model${i} = "model-${i}";`),
            "diff --git a/apps/desktop/public/icon.png b/apps/desktop/public/icon.png",
            "index 6666666..7777777 100644",
            "Binary files a/apps/desktop/public/icon.png and b/apps/desktop/public/icon.png differ",
          ].join("\n"),
        };
      case "_rustcode/pulls/references":
        return {
          references: [
            { number: 2795, title: "feat(desktop): add the login form", kind: "pull", state: "open" },
            {
              number: 2788,
              title: "feat(desktop): Tauri desktop app on every harness",
              kind: "pull",
              state: "open",
            },
            {
              number: 2781,
              title: "fix(engine): keep the steer queue across a restart",
              kind: "pull",
              state: "open",
            },
            {
              number: 2770,
              title: "The composer loses its focus ring after sending",
              kind: "issue",
              state: "open",
            },
            { number: 2702, title: "Draft helper drops the last line", kind: "issue", state: "closed" },
          ],
        };
      case "_rustcode/pulls/thread": {
        const action = obj(params.action);
        const kind = str(action?.kind);
        if ((kind === "reply" || kind === "comment") && !str(action?.body)?.trim())
          return { error: { code: "failed", message: "the comment is empty" } };
        if (kind === "resolve" || kind === "unresolve") {
          this.pullResolved.set(str(action?.threadId) ?? "", kind === "resolve");
          return { done: { resolved: kind === "resolve" } };
        }
        return { done: {} };
      }
      case "_rustcode/pulls/review":
        return { review: { url: "https://github.com/LHagfoss/rustcode/pull/2788#pullrequestreview-1" } };
      case "_rustcode/rewind": {
        const thread = this.thread(params.sessionId);
        const wanted = Number(params.userIndex);
        let n = -1;
        const at = thread.history.findIndex(
          (u) => str(obj(u.update)?.sessionUpdate) === "user_message_chunk" && ++n === wanted,
        );
        if (at < 0) throw Object.assign(new Error("No such message"), { code: -32602 });
        thread.history.splice(at);
        return {};
      }
      case "_rustcode/remote": {
        const sessionId = str(params.sessionId) ?? "";
        return this.remoteStatus(sessionId);
      }
      case "_rustcode/remote/start": {
        const sessionId = str(params.sessionId) ?? "";
        // Settings' switch names no thread: remote access is on for the
        // machine and every open thread is shared. A thread's own start
        // shares that thread, as the engine's auto-share pass does.
        await this.sleep(this.delay * 2);
        if (!sessionId) {
          this.remote.enabled = true;
          this.remote.gateway = this.remoteGateway();
          for (const open of this.threads) this.remote.shared.add(open.id);
          this.remote.refused.clear();
          if (params.pair === true) this.issueRemoteOffer();
          return this.remoteStatus("");
        }
        const thread = this.thread(sessionId);
        this.remote.enabled = true;
        this.remote.gateway = this.remoteGateway();
        this.remote.shared.add(thread.id);
        if (params.pair === true) this.issueRemoteOffer();
        return this.remoteStatus(thread.id);
      }
      case "_rustcode/remote/pair": {
        const sessionId = str(params.sessionId) ?? "";
        // A challenge is the gateway's, so a thread the app does not share
        // still pairs the machine (acp/remote.rs › `pair`).
        await this.sleep(this.delay * 2);
        this.remote.enabled = true;
        if (!this.remote.gateway) this.remote.gateway = this.remoteGateway();
        this.issueRemoteOffer();
        return { ...obj(this.remoteStatus(sessionId)), offer: this.remoteOffer() } as Json;
      }
      case "_rustcode/remote/connect_code": {
        await this.sleep(this.delay * 2);
        if (params.cancel === true) {
          const wasOpen = this.remote.machineOffer !== null;
          this.remote.machineOffer = null;
          return { ...this.remoteHost(), connectCodeCancelled: wasOpen };
        }
        // A connect code lives beside the QR challenge; creating one replaces only the last code.
        this.remote.enabled = true;
        if (!this.remote.gateway) this.remote.gateway = this.remoteGateway();
        this.remote.machineSerial += 1;
        this.remote.machineOffer = { serial: this.remote.machineSerial, expiresAt: Date.now() + 600_000 };
        return {
          ...this.remoteHost(),
          connectCode: {
            code: "K7MQ-29XH-TP4W-NRD6",
            link: "rustcode://connect/eyJ2IjoxLCJnYXRld2F5X2lkIjoiOWYyYzFkMGI0YTZlOGM3ZCIsImhvc3RfbmFtZSI6Ik1vY2sgTWFjQm9vayBQcm8iLCJyb3V0ZXMiOlt7ImFkZHJlc3MiOiIxMDAuOTAuMTYuMjM3OjE3ODc5Iiwia2luZCI6Im92ZXJsYXkifV0sImNyZWRlbnRpYWwiOiJtb2NrIn0",
            address: "100.90.16.237:17879",
            addressInCode: true,
            gatewayId: "9f2c1d0b4a6e8c7d5b3a1908f7e6d5c4",
            instanceId: "mock-instance",
            hostName: "Mock MacBook Pro",
            routes: [{ address: "100.90.16.237:17879", kind: "overlay", provider: "netbird" }],
            loopbackOnly: false,
            expiresInSecs: 600,
            serial: this.remote.machineSerial,
          },
        };
      }
      case "_rustcode/remote/answer": {
        const id = str(params.id) ?? "";
        const accept = params.accept === true;
        await this.sleep(this.delay * 2);
        const request = this.remote.connect.find((entry) => entry.id === id);
        if (!request)
          return { ...this.remoteHost(), connectError: "that connection request is no longer open" };
        if (!accept) {
          this.endConnect(id, "denied");
          return {
            ...this.remoteHost(),
            connectAnswer: { id, deviceName: request.deviceName, accepted: false },
          };
        }
        if (request.state !== "pending")
          return { ...this.remoteHost(), connectError: "that connection request was already accepted" };
        // One challenge at a time: the code replaces the QR code's and any other accepted request's.
        for (const other of this.remote.connect.filter((entry) => entry.state === "approved"))
          this.endConnect(other.id, "replaced");
        this.remote.offer = null;
        request.state = "approved";
        request.expiresAt = Date.now() + 120_000;
        // In the browser the phone types the code a few seconds later; tests enter it themselves.
        if (this.delay > 0) setTimeout(() => this.enterConnectCode(), 6_000);
        return {
          ...this.remoteHost(),
          connectAnswer: {
            id,
            deviceName: request.deviceName,
            instanceId: "mock-gateway-instance-1",
            accepted: true,
            code: "4821-9034",
            expiresInSecs: 120,
          },
        };
      }
      case "_rustcode/remote/revoke": {
        const deviceId = str(params.deviceId) ?? "";
        await this.sleep(this.delay * 2);
        const device = this.remote.devices.find((entry) => entry.id === deviceId);
        if (!device) return { ...this.remoteHost(), revokeError: `no paired device matches ${deviceId}` };
        this.remote.devices = this.remote.devices.filter((entry) => entry !== device);
        return {
          ...this.remoteHost(),
          revoked: { deviceId: str(device.id) ?? deviceId, deviceName: str(device.name) ?? deviceId },
        };
      }
      case "_rustcode/remote/access":
        return setMockAccess(this.remote.devices, params);
      case "_rustcode/outside/list":
      case "_rustcode/outside/create":
      case "_rustcode/outside/update":
      case "_rustcode/outside/revoke":
      case "_rustcode/outside/answer":
      case "_rustcode/outside/signin":
      case "_rustcode/outside/code":
      case "_rustcode/outside/passkey/add":
      case "_rustcode/outside/passkey/remove":
      case "_rustcode/outside/web/sign-out":
      case "_mock/outside/signin":
      case "_mock/outside/ask":
        return this.outside.handle(method, params);
      // Test and mock-window hooks: a phone asks to connect, or enters the code it was shown.
      case "_mock/remote/askToConnect":
        return {
          id: this.askToConnect(
            str(params.deviceName) ?? "Lars's iPhone",
            str(params.platform) ?? "ios",
            str(params.remoteAddr) ?? "192.168.1.42",
            str(params.deviceKind) ?? "phone",
          ),
        };
      case "_mock/remote/enterCode":
        return { paired: this.enterConnectCode() };
      case "_rustcode/remote/stop": {
        const sessionId = str(params.sessionId) ?? "";
        await this.sleep(this.delay * 2);
        this.remote.enabled = false;
        this.remote.shared.clear();
        this.remote.gateway = null;
        this.remote.offer = null;
        this.remote.pairingDue = false;
        this.remote.pairingError = null;
        this.remote.refused.clear();
        // The gateway is gone, and with it every phone waiting on it.
        this.remote.connect = [];
        return this.remoteStatus(sessionId);
      }
      case "_rustcode/warm":
        return {};
      case "_rustcode/worktrees/list": {
        const repo = str(params.repo);
        if (!repo) throw Object.assign(new Error("repo is required"), { code: -32602 });
        return {
          worktrees: this.threads
            .filter(
              (t) => t.worktree && t.worktree.repo === repo && !this.removedWorktrees.has(t.worktree.path),
            )
            .map((t) => ({ ...t.worktree, session_ids: [t.id] }) as unknown as Json),
        };
      }
      case "_rustcode/worktrees/create": {
        const repo = str(params.repo);
        if (!repo) throw Object.assign(new Error("repo is required"), { code: -32602 });
        return this.makeWorktree(repo, str(params.branch), str(params.title)) as unknown as Json;
      }
      case "_rustcode/worktrees/remove": {
        const path = str(params.path);
        if (!path) throw Object.assign(new Error("path is required"), { code: -32602 });
        const t = this.threads.find((x) => x.worktree?.path === path);
        if (!t?.worktree) throw Object.assign(new Error("No such worktree"), { code: -32602 });
        if (params.force !== true && t.worktree.dirty) {
          throw Object.assign(new Error("The worktree has uncommitted changes."), {
            code: -32000,
            data: { reason: "dirty" },
          });
        }
        if (params.force !== true && t.worktree.ahead > 0) {
          throw Object.assign(new Error("The branch has commits that are on no remote."), {
            code: -32000,
            data: { reason: "unpushed" },
          });
        }
        this.removedWorktrees.add(path);
        return { removed: true };
      }
      case "_rustcode/session/title": {
        const title = str(params.title)?.trim();
        if (!title) throw Object.assign(new Error("A title can't be empty."), { code: -32602 });
        const thread = this.thread(params.session_id);
        if (params.user !== true && this.userTitled.has(thread.id))
          return { title: thread.title ?? title, kept: true };
        return { title: this.renameThread(thread.id, title, params.user === true), kept: false };
      }
      case "_rustcode/session/status": {
        const status = str(params.status);
        if (!status || !["review", "done", "idle"].includes(status)) {
          throw Object.assign(new Error("status must be review, done or idle"), { code: -32602 });
        }
        const thread = this.thread(params.session_id);
        thread.status = status;
        return {};
      }
      case "_rustcode/checkpoints/list": {
        const thread = this.thread(params.session_id);
        return {
          checkpoints: thread.checkpoints.map((c) => ({
            turn: c.turn,
            ref: `refs/rustcode/checkpoints/${thread.id}/${c.turn}`,
            created_at: c.created_at,
            prompt: c.prompt.slice(0, 80),
            files_changed: c.files.length,
            insertions: c.files.reduce((n, f) => n + f.insertions, 0),
            deletions: c.files.reduce((n, f) => n + f.deletions, 0),
            ...(c.attempt ? { attempt: true } : {}),
          })),
        };
      }
      case "_rustcode/checkpoints/diff": {
        const thread = this.thread(params.session_id);
        const c = thread.checkpoints.find((x) => x.turn === params.turn);
        if (!c) throw Object.assign(new Error("No such checkpoint"), { code: -32602 });
        return { files: c.files as unknown as Json };
      }
      case "_rustcode/checkpoints/restore": {
        const thread = this.thread(params.session_id);
        const turn = Number(params.turn);
        const at = thread.checkpoints.findIndex((x) => x.turn === turn);
        if (at < 0) throw Object.assign(new Error("No such checkpoint"), { code: -32602 });
        // A kept attempt can be gone back to as well: it restores the state it left, and the
        // conversation stays where it is (the engine has no rewind forward either).
        const attempt = thread.checkpoints[at]!.attempt === true;
        // The engine keeps every checkpoint: a rewind never deletes the turns it leaves.
        let kept: { turn: number; ref: string } | undefined;
        if (params.keep === true) {
          const next = (thread.checkpoints.at(-1)?.turn ?? 0) + 1;
          const abandoned = thread.checkpoints.filter((c) => c.turn >= turn && !c.attempt);
          thread.checkpoints.push({
            turn: next,
            prompt: "",
            created_at: new Date().toISOString(),
            files: abandoned.flatMap((c) => c.files),
            attempt: true,
          });
          kept = { turn: next, ref: `refs/rustcode/checkpoints/${thread.id}/${next}` };
        }
        const canRewind = thread.harness === "rustcode" && !attempt;
        if (params.rewind === true && canRewind) {
          // Its own turn's message, in the history the conversation still has.
          const own = thread.checkpoints.filter((c) => !c.attempt).findIndex((c) => c.turn === turn);
          let n = 0;
          const cut = thread.history.findIndex(
            (u) => str(obj(u.update)?.sessionUpdate) === "user_message_chunk" && ++n === own + 1,
          );
          if (cut >= 0) thread.history.splice(cut);
        }
        return {
          restored: true,
          conversation: params.rewind === true && canRewind ? "rewound" : "kept",
          ...(kept ? { kept } : {}),
        };
      }
      case "_rustcode/config/telemetry":
        // The same shape `_rustcode/config/telemetry` answers with (engine/src/acp/telemetry.rs).
        return { ...this.payloadCapture };
      case "_rustcode/config/telemetry/set": {
        const asked = obj(params);
        if (typeof asked?.providerPayloads !== "boolean" || typeof asked?.harnessRpcPayloads !== "boolean") {
          throw Object.assign(new Error("both switches are required"), { code: -32602 });
        }
        this.payloadCapture = {
          providerPayloads: asked.providerPayloads,
          harnessRpcPayloads: asked.harnessRpcPayloads,
        };
        return { ...this.payloadCapture };
      }
      case "_rustcode/mirror": {
        // Only the switch and the opt-outs: the mock mirrors nothing, so the page reads "Nothing
        // copied yet." (engine/src/acp/mirror.rs).
        const status = () => ({ ...this.mirror, hosts: {} });
        switch (params.action) {
          case "setSettings":
            if (typeof params.enabled === "boolean") this.mirror.enabled = params.enabled;
            if (Array.isArray(params.excludedProjects)) {
              this.mirror.excludedProjects = params.excludedProjects.filter(
                (path): path is string => typeof path === "string",
              );
            }
            return status();
          case "status":
            return status();
          case "list":
            return { enabled: this.mirror.enabled, hosts: {} };
          case "transcript":
            return { found: false };
          default:
            if (!this.mirror.enabled) {
              throw Object.assign(new Error("thread mirroring is off"), { code: -32602 });
            }
            return { stored: 0, unchanged: 0, stale: 0, skipped: 0, excluded: 0 };
        }
      }
      case "_rustcode/presence":
        return { enabled: this.presence, discordDetected: true };
      case "_rustcode/presence/set":
        this.presence = params.enabled === true;
        return { enabled: this.presence, discordDetected: true };
      case "_rustcode/settings/get": {
        this.serverSettings["computer-use"] ??= {
          enabled: false,
          allowed_domains: [],
          browser_available: false,
        };
        const keys = arr(params.keys).flatMap((key) => (typeof key === "string" ? [key] : []));
        return mockSettingsAnswer(this.serverSettings, this.projectSettings, keys, str(params.project));
      }
      case "_rustcode/settings/set": {
        this.serverSettings = mockSettingsSet(this.serverSettings, this.projectSettings, params);
        return mockSettingsAnswer(this.serverSettings, this.projectSettings, [], str(params.project));
      }
      case "_rustcode/harnesses":
        return this.snapshot();
      case "_rustcode/harnesses/models": {
        const id = str(params.harness) ?? "";
        const h = this.harnesses.harnesses.find((x) => x.id === id);
        if (!h) throw Object.assign(new Error(`Unknown harness ${id}`), { code: -32602 });
        await this.sleep(this.delay * 6);
        if (!h.installed) throw new Error(`${h.name} is not installed (${h.command[0]} not on PATH).`);
        if (h.auth === "needs_login") throw new Error(`${h.name} needs you to sign in first.`);
        // A hand-added command named "broken" starts but never answers ACP.
        if (!h.known && h.command[0] === "broken")
          throw new Error(`${h.name} did not answer the ACP handshake (initialize timed out after 30 s).`);
        h.models = h.builtin ? h.models : (HARNESS_MODELS[id] ?? (h.known ? [] : MOCK_CUSTOM_MODELS));
        if (h.auth === "unknown") h.auth = "ok";
        return { models: h.models as unknown as Json };
      }
      case "_rustcode/harnesses/set": {
        const id = str(params.id) ?? "";
        let h = this.harnesses.harnesses.find((x) => x.id === id);
        if (!h) {
          h = {
            id,
            name: str(params.name) ?? id,
            kind: "acp",
            builtin: false,
            known: false,
            enabled: true,
            command: arr(params.command).map(String),
            // A command named "missing" is not on PATH.
            installed: arr(params.command).map(String)[0] !== "missing",
            env_names: Object.keys(obj(params.env) ?? {}),
            auth: "unknown",
            capabilities: ACP_CAPS,
            accounts: [],
            models: [],
            error: null,
          };
          this.harnesses.harnesses.push(h);
        }
        // As the engine: RustCode turns off only once another harness is the default.
        if (h.builtin && params.enabled === false && this.harnesses.default.harness === h.id)
          throw new Error("choose another default harness before turning RustCode off");
        if (typeof params.enabled === "boolean") h.enabled = params.enabled;
        if (Array.isArray(params.command)) {
          h.command = params.command.map(String);
          h.installed = h.command[0] !== "missing";
        }
        if (obj(params.env))
          h.env_names = [
            ...new Set([...Object.keys(obj(params.env)!), ...arr(params.retain_env).map(String)]),
          ];
        return this.snapshot();
      }
      case "_rustcode/harnesses/remove": {
        this.harnesses.harnesses = this.harnesses.harnesses.filter((h) => h.id !== params.id || h.known);
        return this.snapshot();
      }
      case "_rustcode/harnesses/default":
        this.harnesses.default = {
          harness: str(params.harness) ?? "rustcode",
          model: str(params.model) ?? null,
          effort: str(params.effort) ?? null,
        };
        return this.snapshot();
      case "_rustcode/harnesses/accounts/add": {
        const harness = str(params.harness) ?? "";
        const h = this.harnesses.harnesses.find((x) => x.id === harness);
        if (!h) throw Object.assign(new Error(`Unknown harness ${harness}`), { code: -32602 });
        const id = str(params.id) ?? "";
        if (!id || !/^[a-z0-9_.-]+$/.test(id))
          throw Object.assign(new Error(`invalid account id '${id}'`), { code: -32602 });
        if ((h.accounts ?? []).some((a) => a.id === id))
          throw Object.assign(new Error(`there is already a ${harness} account '${id}'`), {
            code: -32602,
          });
        const prefix = h.account_env === "CODEX_HOME" ? "~/.codex-" : "~/.claude-";
        const config_dir = str(params.config_dir) ?? `${prefix}${id}`;
        h.accounts = [
          ...(h.accounts ?? []),
          {
            id,
            name: str(params.name) ?? id,
            config_dir,
            auth: "unknown",
          },
        ];
        // A login found on this Mac is no longer offered as a discovery.
        h.discovered_accounts = (h.discovered_accounts ?? []).filter(
          (found) => found.config_dir !== config_dir,
        );
        return this.snapshot();
      }
      case "_rustcode/harnesses/accounts/remove": {
        const harness = str(params.harness) ?? "";
        const h = this.harnesses.harnesses.find((x) => x.id === harness);
        if (!h) throw Object.assign(new Error(`Unknown harness ${harness}`), { code: -32602 });
        const id = str(params.id) ?? "";
        if ((h.accounts ?? []).some((a) => a.id === id && this.threads.some((t) => t.account === id)))
          throw Object.assign(new Error(`a thread is running on the account '${id}'`), {
            code: -32602,
          });
        h.accounts = (h.accounts ?? []).filter((a) => a.id !== id);
        return this.snapshot();
      }
      case "_rustcode/harnesses/accounts/check":
        return this.snapshot();
      case "_rustcode/harnesses/remember": {
        const id = str(params.harness) ?? "";
        const h = this.harnesses.harnesses.find((x) => x.id === id);
        if (!h) throw Object.assign(new Error(`unknown harness '${id}'`), { code: -32602 });
        const model = str(params.model) ?? null;
        const effort = str(params.effort) ?? null;
        h.last = model || effort ? { model, effort } : null;
        return { harness: id, last: (h.last ?? null) as unknown as Json };
      }
      case "_rustcode/onboarding/complete":
        this.harnesses.onboarded = true;
        return this.snapshot();
      case "_rustcode/capabilities":
        return {
          configPath: "~/.config/rustcode/config.toml",
          skills: MOCK_SKILLS.map(withoutSkillContent),
          mcpServers: this.mcpServers(),
        };
      case "_rustcode/skills/get": {
        const name = str(params.name);
        const path = str(params.path);
        const skill = MOCK_SKILLS.find((entry) => str(entry.name) === name && str(entry.path) === path);
        if (!skill || typeof skill.content !== "string")
          throw Object.assign(new Error(`Could not read ${name ?? "that"}'s SKILL.md`), {
            code: -32602,
          });
        return { content: skill.content };
      }
      case "_rustcode/permissions": {
        // The same shape `_rustcode/permissions` answers with (engine/src/acp/permissions.rs).
        const now = Math.floor(Date.now() / 1000);
        return {
          grants: this.grants.filter((grant) => num(grant.expiresAt)! > now),
        };
      }
      case "_rustcode/permissions/revoke": {
        const id = str(params.id);
        this.grants = this.grants.filter((grant) => str(grant.id) !== id);
        return { grants: this.grants };
      }
      case "_rustcode/themes":
        return this.themesSnapshot();
      case "_rustcode/themes/set": {
        const name = str(params.name);
        if (!name || !this.themes.some((t) => t.name === name))
          throw Object.assign(new Error(`no theme named '${name}'`), { code: -32602 });
        this.activeTheme = name;
        const snapshot = this.themesSnapshot();
        queueMicrotask(() => this.send({ method: "_rustcode/themes/changed", params: snapshot }));
        return snapshot;
      }
      case "_rustcode/themes/new": {
        const name = str(params.name) ?? "";
        const from = this.themes.find((t) => t.name === (str(params.from) ?? "rustcode")) ?? this.themes[0]!;
        if (!/^[a-z0-9][a-z0-9-]*$/.test(name) || this.themes.some((t) => t.name === name))
          throw Object.assign(new Error(`cannot create '${name}'`), { code: -32602 });
        const path = `~/.config/rustcode/themes/${name}.toml`;
        this.themes.push({ ...from, name, description: name, source: "user", path });
        return { path };
      }
      case "_rustcode/session/pin": {
        const thread = this.thread(params.session_id);
        if (typeof params.pinned !== "boolean")
          throw Object.assign(new Error("pinned must be a boolean"), { code: -32602 });
        thread.pinned = params.pinned;
        return {};
      }
      case "_rustcode/capabilities/set":
        return this.handle("_rustcode/capabilities", {});
      case "_rustcode/capabilities/mcp/logout":
      case "_rustcode/capabilities/mcp/add":
      case "_rustcode/capabilities/mcp/remove":
        this.mcpAdded.apply(method, params);
        return this.handle("_rustcode/capabilities", {});
      case "_rustcode/capabilities/mcp/restart": {
        const name = str(params.name) ?? "";
        if (!this.mcpAdded.signIn(name) && name !== "github" && name !== "sqlite" && name !== "fagbrev")
          throw Object.assign(new Error(`MCP server '${name}' is no longer configured`), {
            code: -32602,
          });
        await this.sleep(400);
        this.mcpRestarted.add(name);
        return this.handle("_rustcode/capabilities", {});
      }
      default:
        throw Object.assign(new Error("Method not found"), { code: -32601 });
    }
  }

  private makeWorktree(repo: string, branch?: string, title?: string): MockWorktree {
    let name = branch || `feat/${slug(title ?? "thread")}`;
    const taken = new Set(this.threads.flatMap((t) => (t.worktree ? [t.worktree.branch] : [])));
    for (let n = 2; taken.has(name); n++) name = `${branch || `feat/${slug(title ?? "thread")}`}-${n}`;
    const repoName = repo.split("/").filter(Boolean).pop() ?? "repo";
    return {
      path: `/Users/you/.config/rustcode/worktrees/${repoName}/${name.replace(/\//g, "-")}`,
      branch: name,
      repo,
      base: "c1ace820",
      base_ref: "origin/main",
      created_at: new Date().toISOString(),
      dirty: false,
      ahead: 0,
      behind: 0,
    };
  }

  /** The agent's side of orchestration: a tool call forwarded to the client, as the engine does. */
  private orchestrate(sid: string, tool: string, args: JsonObject): Promise<JsonObject> {
    return this.ask("_rustcode/orchestration", { sessionId: sid, tool, args });
  }

  /** "fan out …", "ask the other thread …", "start a thread …": the v2 tools, when the client offers them. */
  private async orchestrationTurn(
    sid: string,
    prompt: string,
    push: (body: JsonObject) => void,
  ): Promise<string | null> {
    if (/manage (this|the) thread/i.test(prompt) && this.orchestration >= 3) {
      const call = async (name: string, args: JsonObject) => {
        const callId = `orch-${++this.counter}`;
        push({
          sessionUpdate: "tool_call",
          toolCallId: callId,
          title: name,
          kind: "other",
          status: "in_progress",
          rawInput: args,
        });
        const result = await this.orchestrate(sid, name, args);
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: callId,
          status: obj(result.error) ? "failed" : "completed",
          rawOutput: { content: JSON.stringify(result) },
        });
      };
      await call("list_harnesses", {});
      await call("search_sessions", { query: "login" });
      await call("list_worktrees", {});
      await call("list_projects", {});
      await call("list_pull_requests", { state: "open" });
      await call("update_session", { title: "Managed by its agent", pinned: true });
      await call("organize_threads", { session_ids: [sid], pinned: true, unread: true });
      await call("configure_session", { effort: "high" });
      return "Renamed and pinned this thread, marked it unread, and raised its effort.";
    }
    const tool = /fan.?out/i.test(prompt)
      ? "fan_out"
      : /start (a |another )?thread/i.test(prompt)
        ? "start_session"
        : /(ask|message) (the )?(other|another) thread/i.test(prompt)
          ? "message_session"
          : null;
    if (!tool) return null;
    if (this.orchestration < (tool === "message_session" ? 1 : 2)) {
      return "I can't reach other threads here: turn on Settings › Harnesses › Threads can start and message other threads, then restart the engine.";
    }
    const call = (name: string, args: JsonObject) => {
      const callId = `orch-${++this.counter}`;
      push({
        sessionUpdate: "tool_call",
        toolCallId: callId,
        title: name,
        kind: "other",
        status: "in_progress",
        rawInput: args,
      });
      return this.orchestrate(sid, name, args).then((result) => {
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: callId,
          status: obj(result.error) ? "failed" : "completed",
          rawOutput: { content: JSON.stringify(result) },
        });
        return result;
      });
    };
    const task =
      prompt.replace(
        /^.*?(fan.?out|start (a |another )?thread|(ask|message) (the )?(other|another) thread)[:,]?\s*/i,
        "",
      ) || "Update the greeting";
    let links: string[] = [];
    if (tool === "fan_out") {
      const result = await call("fan_out", {
        prompt: `${task} (yolo)`,
        title: task.slice(0, 50),
        harnesses: [{ harness: "rustcode" }, { harness: "codex", model: "gpt-6-luna", effort: "low" }],
      });
      const error = obj(result.error);
      if (error) return `The fan-out did not start: ${str(error.message)}.`;
      links = arr(result.links).flatMap((l) => str(obj(l)?.link_id) ?? []);
    } else if (tool === "start_session") {
      const result = await call("start_session", { prompt: `${task} (yolo)`, title: task.slice(0, 50) });
      const error = obj(result.error);
      if (error) return `The thread did not start: ${str(error.message)}.`;
      links = [str(result.link_id) ?? ""];
    } else {
      const list = await call("list_sessions", {});
      const target = str(obj(arr(list.sessions)[0])?.session_id);
      if (!target) return "There is no other thread in this project to ask.";
      const result = await call("message_session", { session_id: target, message: `${task} (yolo)` });
      const error = obj(result.error);
      if (error) return `The message was not sent: ${str(error.message)}.`;
      links = [str(result.link_id) ?? ""];
    }
    const answers: string[] = [];
    for (let i = 0; i < 40 && answers.length < links.length; i++) {
      const pending = links.filter((_, j) => !answers[j]);
      const waited = await call("wait_session", { link_ids: pending, timeout_ms: 30_000 });
      for (const l of arr(waited.links)) {
        const o = obj(l);
        const at = links.indexOf(str(o?.link_id) ?? "");
        if (at >= 0 && str(o?.status) !== "working" && str(o?.status) !== "queued") {
          answers[at] =
            `${str(o?.title)}: ${str(o?.status)}${str(o?.answer) ? ` — ${str(o?.answer)?.split("\n")[0]}` : ""}`;
        }
      }
    }
    return `Here is what came back:\n\n${answers.map((a) => `- ${a}`).join("\n")}`;
  }

  /** "One agent spawns several": two children, started by `spawn_agent` calls, that finish a moment later. */
  private async spawnAgents(sid: string, push: (body: JsonObject) => void, run: { cancelled: boolean }) {
    const tasks: [string, string][] = [
      ["explorer", "Map where the composer handles paste and drop"],
      ["worker", "Write tests for link chips in the composer"],
    ];
    const list = this.agents.get(sid) ?? [];
    this.agents.set(sid, list);
    for (const [type, task] of tasks) {
      const id = list.length + 1;
      const callId = `spawn-${++this.counter}`;
      push({
        sessionUpdate: "tool_call",
        toolCallId: callId,
        title: "spawn_agent",
        kind: "other",
        status: "in_progress",
        rawInput: { task, agent_type: type },
      });
      list.push({
        id,
        name: `agent-${id}`,
        task,
        model: "gpt-6-luna",
        agentType: type,
        status: "running",
        activeTurn: true,
        depth: 1,
        startedAtMs: Date.now(),
        lastActivityMs: Date.now(),
        toolCalls: 3,
        requests: 2,
        inputTokens: 18_000,
        outputTokens: 900,
        cachedInputTokens: 12_000,
      });
      push({
        sessionUpdate: "tool_call_update",
        toolCallId: callId,
        status: "completed",
        rawOutput: { content: `{"agent_id":${id},"name":"agent-${id}"}` },
      });
    }
    const finish = () => {
      for (const agent of list) {
        if (agent.status !== "running") continue;
        agent.status = "completed";
        agent.activeTurn = false;
        agent.finishedAtMs = Date.now();
        agent.completion = `Done: ${String(agent.task).toLowerCase()}.`;
      }
    };
    if (this.delay) setTimeout(finish, this.delay * 40);
    else finish();
    if (run.cancelled) return;
  }

  /**
   * "Task agents": Claude Code's subagents, as its adapter streams them. Each
   * `Agent` call's own steps carry `_meta.claudeCode.parentToolUseId`.
   */
  private async claudeAgents(push: (body: JsonObject) => void, run: { cancelled: boolean }) {
    const agents: [string, string, string, [string, string, JsonObject][]][] = [
      [
        "Explore",
        "Map the composer's paste handling",
        "explore",
        [
          ["Grep", "search", { pattern: "onPaste", path: "src/ui" }],
          ["Read Composer.tsx", "read", { file_path: "src/ui/Composer.tsx" }],
          ["Read compose.ts", "read", { file_path: "src/state/compose.ts" }],
        ],
      ],
      [
        "general-purpose",
        "Write tests for link chips",
        "worker",
        [
          ["Read Markdown.tsx", "read", { file_path: "src/ui/Markdown.tsx" }],
          ["Write Markdown.test.tsx", "edit", { file_path: "src/ui/Markdown.test.tsx" }],
          ["bun test src/ui", "execute", { command: "bun test src/ui" }],
        ],
      ],
    ];
    const ids = agents.map(([type, description]) => {
      const id = `agent-${++this.counter}`;
      push({
        sessionUpdate: "tool_call",
        toolCallId: id,
        title: description,
        kind: "think",
        status: "in_progress",
        rawInput: { description, prompt: `${description}.`, subagent_type: type },
        _meta: { claudeCode: { toolName: "Agent" } },
      });
      return id;
    });
    const longest = Math.max(...agents.map((a) => a[3].length));
    for (let step = 0; step < longest; step++) {
      for (const [i, agent] of agents.entries()) {
        const s = agent[3][step];
        if (!s || run.cancelled) continue;
        const id = `${ids[i]}-${step}`;
        const meta = { claudeCode: { toolName: s[0].split(" ")[0] ?? s[0], parentToolUseId: ids[i] ?? "" } };
        push({
          sessionUpdate: "tool_call",
          toolCallId: id,
          title: s[0],
          kind: s[1],
          status: "in_progress",
          rawInput: s[2],
          _meta: meta,
        });
        await this.sleep(this.delay * 8);
        push({ sessionUpdate: "tool_call_update", toolCallId: id, status: "completed", _meta: meta });
      }
    }
    for (const [i, agent] of agents.entries()) {
      push({
        sessionUpdate: "tool_call_update",
        toolCallId: ids[i]!,
        status: "completed",
        rawOutput: { content: `Done: ${agent[1].toLowerCase()}. Three files touched, all tests pass.` },
      });
      await this.sleep(this.delay * 4);
    }
  }

  private async prompt(params: JsonObject): Promise<Json> {
    const thread = this.thread(params.sessionId);
    const prompt = arr(params.prompt)
      .map((b) => str(obj(b)?.text) ?? "")
      .join("");
    const sid = thread.id;
    const run = { cancelled: false, steers: [] as string[] };
    this.running.set(sid, run);
    const push = (body: JsonObject) => {
      const line = update(sid, body);
      thread.history.push(line);
      this.notify(line);
    };
    thread.history.push(update(sid, { sessionUpdate: "user_message_chunk", content: text(prompt) }));
    thread.updatedAt = Date.now();
    const started = Date.now();
    const done = (stopReason: string) => {
      this.running.delete(sid);
      return { stopReason, _meta: { rustcode: { unusedSteers: run.steers } } };
    };
    try {
      // Report commands answer like the engine's: marked as a report, kept out of the transcript.
      const report = MOCK_REPORTS[prompt.trim()];
      if (report) {
        for (const chunk of report) {
          this.notify(
            update(sid, {
              sessionUpdate: "agent_message_chunk",
              content: text(chunk),
              _meta: { rustcode: { report: true } },
            }),
          );
          await this.sleep();
        }
        return done("end_turn");
      }
      if (prompt.trim() === "/fork") {
        // Like the engine: a copy of the chat in a new thread that records this one as its parent.
        const title = `${thread.title ?? "Forked chat"} (fork)`;
        this.threads.unshift({
          ...thread,
          id: `mock-${Date.now().toString(36)}-${++this.counter}`,
          title,
          updatedAt: Date.now(),
          parent: { id: thread.id, title: thread.title, relationship: "fork" },
          history: [...thread.history],
          checkpoints: [],
        });
        push({
          sessionUpdate: "agent_message_chunk",
          content: text(`**Fork**\n\nForked this chat into **${title}**. Open it from the chat list.`),
        });
        return done("end_turn");
      }
      // `/compact` says it is compacting while it runs, then replaces that line with its card. Two
      // variants stage the other states: `/compact hold` keeps compacting until stopped, and
      // `/compact fail` ends without compacting, with the reason. Neither line is history: a
      // reopened thread replays the card only.
      const compact = /^\/compact(?:\s+(hold|fail))?$/.exec(prompt.trim());
      if (compact) {
        const compacting = (phase: string, line: string) =>
          this.notify(
            update(sid, {
              sessionUpdate: "agent_message_chunk",
              content: text(line),
              _meta: { rustcode: { kind: "compaction", phase } },
            }),
          );
        compacting("started", "Compacting context…");
        if (compact[1] === "hold") {
          for (let i = 0; i < 600 && !run.cancelled; i++) await this.sleep(1_000);
          return done(run.cancelled ? "cancelled" : "end_turn");
        }
        if (compact[1] === "fail") {
          await this.sleep();
          compacting("failed", "Context compaction failed: the summary request timed out.");
          return done("end_turn");
        }
        thread.compactions = (thread.compactions ?? 0) + 1;
        const before = 18_400;
        const after = 15_500;
        // A harness compacts with a call of its own (Claude Code's `Compact conversation`) and then
        // says so; the app keeps the call out of a turn it started itself, so one notice is the
        // record of it and the answer above stays where the reader left it (`state/thread.ts`).
        const compactId = `compact-${++this.counter}`;
        push({
          sessionUpdate: "tool_call",
          toolCallId: compactId,
          title: "Compact conversation",
          kind: "think",
          status: "in_progress",
          rawInput: { summary: "Summarize the conversation" },
        });
        await this.sleep();
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: compactId,
          status: "completed",
          rawOutput: { content: "Summarized the conversation." },
        });
        push({
          sessionUpdate: "agent_message_chunk",
          content: text(`Context compacted: reduced from ${before / 1_000}k to ${after / 1_000}k tokens.`),
          _meta: { rustcode: { kind: "compaction" } },
        });
        this.notify(
          update(sid, {
            sessionUpdate: "usage_update",
            used: after,
            size: 272_000,
            _meta: {
              rustcode: {
                estimated: false,
                inputTokens: 90_000,
                outputTokens: 3_200,
                cachedTokens: 61_000,
                context: mockContext(after, {
                  count: thread.compactions,
                  reclaimed: before - after,
                  before,
                }),
              },
            },
          }),
        );
        return done("end_turn");
      }
      if (!thread.title) {
        thread.title = prompt.split("\n")[0]?.slice(0, 60) || "New thread";
        this.notify(update(sid, { sessionUpdate: "session_info_update", title: thread.title }));
      }
      // Two row states that only exist while something happens, so the sidebar's status chip can be
      // seen without a model: "keep working" leaves a turn running for two minutes (`Working 0:12`
      // with its clock, then `Waiting`), and "fail this turn" ends it with an error (`Failed` until
      // the row is opened). `?scenario=delegates` covers `Waiting`, `Needs you` and the stack.
      if (/\bfail this turn\b/i.test(prompt)) throw new Error("The mock failed this turn, as asked.");
      if (/\bkeep working\b/i.test(prompt)) {
        const callId = `live-${++this.counter}`;
        push({
          sessionUpdate: "tool_call",
          toolCallId: callId,
          title: "bash",
          kind: "execute",
          status: "in_progress",
          rawInput: { command: "bun test --watch", summary: "Keep the turn running" },
        });
        for (let i = 0; i < 60 && !run.cancelled; i++) {
          await this.sleep(2_000);
          push({
            sessionUpdate: "tool_call_update",
            toolCallId: callId,
            _meta: { rustcode: { outputDelta: `✓ ${i + 1} tests still passing\n` } },
          });
        }
        if (run.cancelled) return done("cancelled");
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: callId,
          status: "completed",
          rawOutput: { content: "✓ 60 pass, 0 fail\n", exitCode: 0 },
        });
        return done("end_turn");
      }
      for (const chunk of [
        "**Reading the request**\n",
        "Checking which files are involved ",
        "before editing.",
      ]) {
        if (run.cancelled) return done("cancelled");
        push({ sessionUpdate: "agent_thought_chunk", content: text(chunk) });
        await this.sleep();
      }
      if (/^From thread/.test(prompt) === false) {
        const answer = await this.orchestrationTurn(sid, prompt, push);
        if (answer !== null) {
          push({ sessionUpdate: "agent_message_chunk", content: text(answer) });
          return done("end_turn");
        }
      }
      // Claude Code's own words when a plan's limit stops a turn: a reply, not a failure.
      if (/\bhit the limit\b/i.test(prompt)) {
        push({
          sessionUpdate: "agent_message_chunk",
          content: text("You've hit your 5-hour limit · resets 11:59pm"),
        });
        return done("end_turn");
      }
      if (/\btask agents\b/i.test(prompt)) await this.claudeAgents(push, run);
      else if (/\bagents\b|subagents?/i.test(prompt)) await this.spawnAgents(sid, push, run);
      push({
        sessionUpdate: "plan",
        entries: [
          { content: "Find the code", status: "completed", priority: "medium" },
          { content: "Make the change", status: "in_progress", priority: "medium" },
          { content: "Run the checks", status: "pending", priority: "low" },
        ],
      });
      // A prompt about a plan stops there, with the plan still open: the card at the foot of the
      // transcript has something left to do and offers Implement.
      if (/\bplan\b/i.test(prompt)) {
        push({
          sessionUpdate: "agent_message_chunk",
          content: text("Here's how I'd do it. Say the word and I'll start."),
        });
        push({
          sessionUpdate: "plan",
          entries: [
            { content: "Read the composer and the transcript", status: "completed", priority: "medium" },
            { content: "Move the approval to its tool row", status: "pending", priority: "high" },
            { content: "Keep a resolved question in the transcript", status: "pending", priority: "medium" },
            { content: "Draw the plan card with Expand and Copy", status: "pending", priority: "medium" },
            { content: "Persist drafts across restarts", status: "pending", priority: "low" },
            { content: "Write the design.md paragraphs", status: "pending", priority: "low" },
            { content: "Add a mock state for each of the three", status: "pending", priority: "low" },
            { content: "Run the checks", status: "pending", priority: "high" },
          ],
        });
        return done("end_turn");
      }
      if (/request_secret|secret card/i.test(prompt)) {
        await this.ask("session/request_permission", {
          sessionId: sid,
          toolCall: {
            toolCallId: "secret-demo",
            title: "Secret API_KEY",
            rawInput: {
              name: "API_KEY",
              reason: "Authenticate the deployment",
              destination: ".env.local",
              env: true,
            },
          },
          options: [
            { optionId: "secret_save", name: "Save", kind: "allow_once" },
            { optionId: "secret_decline", name: "Decline", kind: "reject_once" },
          ],
          _meta: { rustcode: { kind: "secret", version: 1 } },
        });
      }
      if (/question/i.test(prompt)) {
        const answer = await this.ask("session/request_permission", {
          sessionId: sid,
          toolCall: {
            toolCallId: "q-1",
            title: "Which test runner?",
            rawInput: {
              header: "Tests",
              question: "Which test runner should I use?",
              options: ["bun test", "vitest"],
              descriptions: ["Built in, no dependency", "Familiar, needs a dependency"],
              multiple: false,
            },
          },
          options: [
            { optionId: "question_option_0", name: "bun test", kind: "allow_once" },
            { optionId: "question_option_1", name: "vitest", kind: "allow_once" },
            { optionId: "question_cancel", name: "Cancel", kind: "reject_once" },
          ],
          _meta: {
            rustcode: {
              kind: "question",
              version: 2,
              questionId: "q-1",
              answerKinds: ["options", "text", "attachments"],
            },
          },
        });
        if (
          str(obj(answer.outcome)?.outcome) === "cancelled" ||
          obj(answer.outcome)?.optionId === "question_cancel"
        ) {
          return done("cancelled");
        }
      }
      // One usage update per round, as the engine sends it: the session's
      // running totals. The app's Trace subtracts them to price each call, so a
      // mock turn shows where its money went too.
      const round = (input: number, output: number, cost: number) => {
        const used = 24_000 + Math.round((Date.now() - started) / 10);
        push({
          sessionUpdate: "usage_update",
          used,
          size: 272_000,
          cost: { amount: cost, currency: "USD" },
          _meta: {
            rustcode: {
              estimated: false,
              inputTokens: input,
              outputTokens: output,
              cachedTokens: Math.round(input * 0.55),
              context: mockContext(used),
            },
          },
        });
      };
      round(120_000, 4_100, 0.0276);
      // A live call's id never reuses one the replayed history already has: an
      // update keyed the same would replace an old row instead of adding one.
      const callId = `live-${++this.counter}`;
      push({
        sessionUpdate: "tool_call",
        toolCallId: callId,
        title: "bash",
        kind: "execute",
        status: "in_progress",
        rawInput: { command: "bun test src", summary: "Run the unit tests" },
      });
      for (const line of ["bun test v1.3.14\n", "src/state/reducer.test.ts:\n", "✓ 24 pass, 0 fail\n"]) {
        if (run.cancelled) return done("cancelled");
        await this.sleep();
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: callId,
          _meta: { rustcode: { outputDelta: line } },
        });
      }
      push({
        sessionUpdate: "tool_call_update",
        toolCallId: callId,
        status: "completed",
        rawOutput: {
          content: "bun test v1.3.14\nsrc/state/reducer.test.ts:\n✓ 24 pass, 0 fail\n",
          exitCode: 0,
        },
      });
      round(147_000, 4_900, 0.0341);
      // "trace" asks for the whole picture: a few more calls and one delegate,
      // so the Trace chip under the turn draws every kind of row.
      const forTrace = /\b(trace|waterfall|delegate)\b/i.test(prompt);
      if (forTrace) {
        const readId = `live-${++this.counter}`;
        push({
          sessionUpdate: "tool_call",
          toolCallId: readId,
          title: "read src/ui/Composer.tsx",
          kind: "read",
          status: "in_progress",
          rawInput: { path: "src/ui/Composer.tsx", summary: "Read the composer" },
        });
        await this.sleep(this.delay * 3);
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: readId,
          status: "completed",
          rawOutput: {
            content: "export function Composer() {\n  const [draft, setDraft] = useState('');\n  ...\n}\n",
          },
        });
        round(166_000, 5_400, 0.0402);
      }
      const editId = `live-${++this.counter}`;
      const diff = {
        type: "diff",
        path: "src/app.ts",
        oldText: 'const greeting = "hello";\nexport default greeting;',
        newText: 'const greeting = "hello, world";\nexport default greeting;',
        _meta: { rustcode: { oldStart: 1, newStart: 1 } },
      };
      push({
        sessionUpdate: "tool_call",
        toolCallId: editId,
        title: "edit src/app.ts",
        kind: "edit",
        status: "pending",
        rawInput: { path: "src/app.ts", summary: "Update the greeting" },
      });
      thread.checkpoints.push({
        turn: (thread.checkpoints.at(-1)?.turn ?? 0) + 1,
        prompt: prompt.slice(0, 80),
        created_at: new Date().toISOString(),
        files: [{ path: "src/app.ts", status: "modified", insertions: 1, deletions: 1, patch: APP_PATCH }],
      });
      if (thread.worktree) thread.worktree.dirty = true;
      if (!/yolo/i.test(prompt) && !/^From thread/.test(prompt)) {
        const answer = await this.ask("session/request_permission", {
          sessionId: sid,
          toolCall: {
            toolCallId: editId,
            title: "edit src/app.ts",
            kind: "edit",
            content: [diff],
            rawInput: { path: "src/app.ts" },
          },
          options: [
            { optionId: "allow", name: "Allow", kind: "allow_once" },
            { optionId: "always", name: "Always allow edits", kind: "allow_always" },
            { optionId: "reject", name: "Reject", kind: "reject_once" },
          ],
        });
        const outcome = obj(answer.outcome);
        if (str(outcome?.outcome) === "cancelled") return done("cancelled");
        if (outcome?.optionId === "reject") {
          push({
            sessionUpdate: "tool_call_update",
            toolCallId: editId,
            status: "failed",
            rawOutput: { content: "Rejected by the user." },
          });
        } else {
          push({
            sessionUpdate: "tool_call_update",
            toolCallId: editId,
            status: "completed",
            content: [diff],
          });
        }
      } else {
        push({ sessionUpdate: "tool_call_update", toolCallId: editId, status: "completed", content: [diff] });
      }
      if (forTrace) {
        round(184_000, 6_100, 0.0493);
        // A delegate: one call folded into the trace's single dotted row. Its
        // agent is recorded like `spawnAgents` records one, so its row and its
        // steps answer from the engine rather than from nothing.
        const task = "Check the composer tests for the focus fix";
        const agents = this.agents.get(sid) ?? [];
        this.agents.set(sid, agents);
        const agentId = agents.length + 1;
        agents.push({
          id: agentId,
          name: `agent-${agentId}`,
          task,
          model: "gpt-6-luna",
          agentType: "explorer",
          status: "running",
          activeTurn: true,
          depth: 1,
          startedAtMs: Date.now(),
          lastActivityMs: Date.now(),
          toolCalls: 1,
          requests: 1,
          inputTokens: 12_000,
          outputTokens: 600,
          cachedInputTokens: 8_000,
        });
        const delegateId = `spawn-${++this.counter}`;
        push({
          sessionUpdate: "tool_call",
          toolCallId: delegateId,
          title: "spawn_agent",
          kind: "other",
          status: "in_progress",
          rawInput: { task, agent_type: "explorer" },
        });
        await this.sleep(this.delay * 10);
        const agent = agents.find((a) => a.id === agentId);
        if (agent) {
          agent.status = "completed";
          agent.activeTurn = false;
          agent.finishedAtMs = Date.now();
          agent.completion = "The composer tests pass; nothing to change.";
        }
        push({
          sessionUpdate: "tool_call_update",
          toolCallId: delegateId,
          status: "completed",
          rawOutput: {
            content: JSON.stringify({ agent_id: agentId, name: `agent-${agentId}`, status: "completed" }),
          },
        });
      }
      // A message the turn read mid-step (`_rustcode/steer`): it says so where it read it, and the
      // plan moves on, so the app has a step that was active when the note landed and a next step
      // whose clock starts (`state/steer.ts`). A steer that arrives after this is still answered at
      // the foot of the turn, below.
      if (run.steers.length) {
        const read = run.steers.splice(0);
        push({
          sessionUpdate: "agent_message_chunk",
          content: text(`Noted ${read.map((steer) => `“${steer}”`).join(", ")}.\n\n`),
        });
        push({
          sessionUpdate: "plan",
          entries: [
            { content: "Find the code", status: "completed", priority: "medium" },
            { content: "Make the change", status: "completed", priority: "medium" },
            { content: "Run the checks", status: "in_progress", priority: "low" },
          ],
        });
      }
      const reply =
        catalogReply(prompt) ??
        (/\bshorthand\b/i.test(prompt)
          ? MOCK_RECONNECT_SHORTHAND
          : /\breconnect\b/i.test(prompt)
            ? MOCK_RECONNECT_HTML
            : /\breview\b/i.test(prompt)
              ? MOCK_REVIEW
              : /\bheadings?\b/i.test(prompt)
                ? MOCK_HEADINGS
                : /\bdashboard\b/i.test(prompt)
                  ? MOCK_DASH
                  : /\bkit\b/i.test(prompt)
                    ? MOCK_KIT
                    : /\bproposal\b/i.test(prompt)
                      ? MOCK_PROPOSAL
                      : /\b(hover|trend|graph)\b/i.test(prompt)
                        ? MOCK_CHARTS
                        : /\btable\b/i.test(prompt)
                          ? MOCK_TABLE
                          : /\bview\b/i.test(prompt)
                            ? MOCK_VIEW
                            : /\bcharts?\b/i.test(prompt)
                              ? MOCK_VIZ
                              : [
                                  "Done. ",
                                  "I updated the greeting in `src/app.ts` ",
                                  "and the tests pass.\n\n",
                                  "```ts\n",
                                  'const greeting = "hello, world";\n',
                                  "export default greeting;\n",
                                  "```\n\n",
                                  "| Check | Result |\n|---|---|\n| tests | 24 pass |\n| lint | clean |\n",
                                ]);
      for (const chunk of reply) {
        if (run.cancelled) return done("cancelled");
        await this.sleep();
        push({ sessionUpdate: "agent_message_chunk", content: text(chunk) });
      }
      for (const steer of run.steers.splice(0)) {
        push({ sessionUpdate: "agent_message_chunk", content: text(`\n\nNoted: “${steer}”.`) });
      }
      push({
        sessionUpdate: "plan",
        entries: [
          { content: "Find the code", status: "completed" },
          { content: "Make the change", status: "completed" },
          { content: "Run the checks", status: "completed" },
        ],
      });
      round(214_000, 7_100, 0.0612);
      return done("end_turn");
    } finally {
      this.running.delete(sid);
    }
  }
}

export { type MockOptions, mockSettingsAnswer, mockSettingsSet } from "./mockFixtures";
export { MockMachine } from "./mockMachine";
export { MockTransport } from "./mockTransport";
