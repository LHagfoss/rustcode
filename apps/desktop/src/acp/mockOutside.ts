// The mock engine's outside agents (Settings › Outside agents), as the
// `_rustcode/outside/*` methods answer them: connected apps, calls waiting for
// Allow, and hosted agents waiting on the public sign-in page. The
// `_mock/outside/*` hooks stage a waiting call or sign-in for the mock window.

import { bool, type Json, type JsonObject, str } from "./jsonrpc";

/** What each access level allows and does not, as the engine's `levels` lists them. */
const MOCK_OUTSIDE_LEVELS = [
  {
    scope: "read_only",
    label: "Read-only",
    summary: "Look, but change nothing",
    can: ["Read your threads, usage, harnesses and machines"],
    cannot: ["Start, message or change anything"],
  },
  {
    scope: "delegate",
    label: "Threads",
    summary: "Start and message threads",
    can: [
      "Everything Read-only can",
      "Start threads and send them messages",
      "Threads it starts can edit files, and are declined commands that need approval",
    ],
    cannot: ["Merge, clean up, or change settings"],
  },
  {
    scope: "manage",
    label: "Manage",
    summary: "Run work end to end",
    can: [
      "Everything Threads can",
      "Merge, clean up, projects and scheduled work",
      "Threads it starts may run with Full access",
    ],
    cannot: ["Change RustCode's own settings"],
  },
  {
    scope: "admin",
    label: "Full",
    summary: "Everything, settings included",
    can: ["Everything Manage can", "Change RustCode's own settings"],
    cannot: [],
  },
];

export class MockOutside {
  private outside: {
    grants: Array<{
      id: string;
      name: string;
      platform: string;
      scope: string;
      ask: boolean;
      createdAt: string;
      lastUsedAt: string | null;
      lastTool: string | null;
    }>;
    pending: Array<{
      id: string;
      clientId: string;
      clientName: string;
      tool: string;
      summary: string;
      askedAt: string;
    }>;
    recent: Array<Record<string, Json>>;
    /** Hosted agents waiting on the public sign-in page (`_mock/outside/signin` stages one). */
    signIns: Array<Record<string, Json>>;
    /** The owner's passkeys; a link "opened" by the mock adds one. */
    passkeys: Array<Record<string, Json>>;
    /** Browsers signed in to the web app; each passkey the mock adds signs one in. */
    webSessions: Array<Record<string, Json>>;
    next: number;
  } = { grants: [], pending: [], recent: [], signIns: [], passkeys: [], webSessions: [], next: 1 };

  private list(): JsonObject {
    return {
      grants: this.outside.grants,
      pending: this.outside.pending,
      recent: this.outside.recent.slice(0, 25),
      platforms: ["claude-desktop", "cursor", "codex", "other"],
      command: "/Applications/RustCode.app/Contents/MacOS/rustcode",
      httpUrl: "http://127.0.0.1:43871/mcp",
      maxGrants: 20,
      available: true,
      signIns: this.outside.signIns,
      publicUrl: "https://rc.example.com/mcp",
      publicError: null,
      machineName: "Mock Mac",
      levels: MOCK_OUTSIDE_LEVELS,
      passkeys: this.outside.passkeys,
      webSessions: this.outside.webSessions,
      webInstalled: true,
    } as unknown as JsonObject;
  }

  handle(method: string, params: JsonObject): JsonObject {
    switch (method) {
      case "_rustcode/outside/list":
        return this.list();
      case "_rustcode/outside/create": {
        const name = (str(params.name) ?? "").trim();
        if (!name) throw Object.assign(new Error("name the app that will connect"), { code: -32602 });
        const scope = str(params.scope) ?? "read_only";
        const platform = str(params.platform) ?? "other";
        const id = `mock${this.outside.next++}`;
        const token = `rcx_${id}_${"0123456789abcdef".repeat(4)}`;
        const grant = {
          id,
          name,
          platform,
          scope,
          ask: bool(params.ask) ?? scope !== "read_only",
          createdAt: new Date().toISOString(),
          lastUsedAt: null,
          lastTool: null,
        };
        this.outside.grants.push(grant);
        const command = "/Applications/RustCode.app/Contents/MacOS/rustcode";
        const toml = platform === "codex";
        return {
          grant,
          token,
          snippet: toml
            ? {
                format: "toml",
                where: "~/.codex/config.toml",
                text: `[mcp_servers.rustcode]\ncommand = "${command}"\nargs = ["mcp-connect"]\n\n[mcp_servers.rustcode.env]\nRUSTCODE_MCP_TOKEN = "${token}"\n`,
              }
            : {
                format: "json",
                where: "Claude Desktop › Settings › Developer › Edit Config (claude_desktop_config.json)",
                text: JSON.stringify(
                  {
                    mcpServers: {
                      rustcode: { command, args: ["mcp-connect"], env: { RUSTCODE_MCP_TOKEN: token } },
                    },
                  },
                  null,
                  2,
                ),
              },
        } as unknown as JsonObject;
      }
      case "_rustcode/outside/update": {
        const grant = this.outside.grants.find((entry) => entry.id === str(params.id));
        if (!grant) throw Object.assign(new Error("no connected app has that id"), { code: -32602 });
        if (str(params.scope)) grant.scope = str(params.scope) ?? grant.scope;
        if (typeof params.ask === "boolean") grant.ask = params.ask;
        return { grant } as unknown as JsonObject;
      }
      case "_rustcode/outside/revoke": {
        const before = this.outside.grants.length;
        this.outside.grants = this.outside.grants.filter((entry) => entry.id !== str(params.id));
        this.outside.pending = this.outside.pending.filter((entry) => entry.clientId !== str(params.id));
        return { revoked: this.outside.grants.length < before };
      }
      case "_rustcode/outside/answer": {
        const item = this.outside.pending.find((entry) => entry.id === str(params.id));
        this.outside.pending = this.outside.pending.filter((entry) => entry !== item);
        if (item) {
          this.outside.recent.unshift({
            at: new Date().toISOString(),
            clientId: item.clientId,
            clientName: item.clientName,
            scope: "delegate",
            tool: item.tool,
            outcome: params.allow === true ? "ok" : "declined",
            durationMs: 40,
            detail: null,
          });
        }
        return { answered: !!item };
      }
      case "_rustcode/outside/signin": {
        const item = this.outside.signIns.find((entry) => entry.id === str(params.id));
        this.outside.signIns = this.outside.signIns.filter((entry) => entry !== item);
        if (item && params.allow === true) {
          const id = `mock${this.outside.next++}`;
          this.outside.grants.push({
            id,
            name: String(item.clientName),
            platform: "other",
            scope: str(params.scope) ?? String(item.scope),
            ask: params.ask === true,
            createdAt: new Date().toISOString(),
            lastUsedAt: null,
            lastTool: null,
            hosted: true,
            signedInUntil: Math.round(Date.now() / 1000) + 30 * 86_400,
          } as never);
        }
        return { answered: !!item };
      }
      case "_rustcode/outside/code":
        return { code: "K7QM-4XPD", scope: str(params.scope) ?? "read_only", minutes: 10 };
      case "_rustcode/outside/passkey/add": {
        // The mock has no browser to finish the ceremony: the link adds the
        // passkey at once, as opening it on a phone would.
        const id = `pk${this.outside.next++}`;
        this.outside.passkeys.push({
          id,
          name: str(params.name) ?? "Passkey",
          host: "rc.example.com",
          createdAt: new Date().toISOString(),
          lastUsedAt: null,
        });
        // ...and a browser then signs in to the web app with it.
        this.outside.webSessions.push({
          id: `web${this.outside.next++}`,
          name: "Safari on macOS",
          passkey: id,
          createdAt: new Date().toISOString(),
          lastUsedAt: new Date().toISOString(),
          expiresAt: new Date(Date.now() + 30 * 86_400_000).toISOString(),
        });
        return { link: `https://rc.example.com/owner/passkey?ticket=${id}`, minutes: 10 };
      }
      case "_rustcode/outside/passkey/remove": {
        const id = str(params.id);
        const item = this.outside.passkeys.find((passkey) => passkey.id === id);
        if (!item) throw Object.assign(new Error(`no passkey matches ${id}`), { code: -32602 });
        this.outside.passkeys = this.outside.passkeys.filter((passkey) => passkey !== item);
        // Removing a passkey signs out the browsers it signed in.
        this.outside.webSessions = this.outside.webSessions.filter((session) => session.passkey !== id);
        return { removed: item };
      }
      case "_rustcode/outside/web/sign-out": {
        const id = str(params.id);
        const removed = this.outside.webSessions.filter((session) => id === "all" || session.id === id);
        if (removed.length === 0)
          throw Object.assign(new Error(`no browser session matches ${id}`), { code: -32602 });
        this.outside.webSessions = this.outside.webSessions.filter((session) => !removed.includes(session));
        return { removed };
      }
      // Mock-window hook: a hosted agent waits on the public sign-in page.
      case "_mock/outside/signin": {
        const id = `signin${this.outside.next++}`;
        this.outside.signIns.push({
          id,
          clientName: str(params.name) ?? "ChatGPT",
          destination: "chatgpt.com",
          scope: str(params.scope) ?? "delegate",
          words: "amber fox",
          askedAt: new Date().toISOString(),
          expiresIn: 300,
        });
        return { id };
      }
      // Mock-window hook: a connected app asks to run a tool, as an outside agent's call would.
      case "_mock/outside/ask": {
        const grant = this.outside.grants[0];
        if (!grant) return { asked: false };
        this.outside.pending.push({
          id: `pending${this.outside.next++}`,
          clientId: grant.id,
          clientName: grant.name,
          tool: str(params.tool) ?? "start_session",
          summary: str(params.summary) ?? "start_session in ~/code/app “Fix the failing build”",
          askedAt: new Date().toISOString(),
        });
        return { asked: true };
      }
      default:
        throw new Error(`mock outside: ${method}`);
    }
  }
}
