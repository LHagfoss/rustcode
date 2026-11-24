import { type JsonObject, str } from "./jsonrpc";

const APPROVE_MS = 4000;

interface GitHubRow {
  host: string;
  login: string;
  active: boolean;
  valid: boolean;
  scopes: string[];
}

interface AccountRow {
  provider: string;
  account: string;
  display: string;
  method: string;
  active: boolean;
  expiresAt: null;
  needsSignIn: boolean;
}

/**
 * The mock engine's Settings › Connections (`_rustcode/connections/*`): one GitHub sign-in, a
 * device-code sign-in that GitHub approves after a few seconds, and three model accounts.
 */
export class MockConnections {
  private github: GitHubRow[] = [
    { host: "github.com", login: "octocat", active: true, valid: true, scopes: ["repo", "read:org"] },
  ];
  private accounts: AccountRow[] = [
    {
      provider: "openai",
      account: "a1",
      display: "you@example.com",
      method: "chat_gpt",
      active: true,
      expiresAt: null,
      needsSignIn: false,
    },
    {
      provider: "anthropic",
      account: "work",
      display: "Work API key",
      method: "api_key",
      active: false,
      expiresAt: null,
      needsSignIn: false,
    },
    {
      provider: "github_copilot",
      account: "c1",
      display: "octocat",
      method: "github_copilot",
      active: false,
      expiresAt: null,
      needsSignIn: true,
    },
  ];
  private pending: { code: string; url: string } | null = null;
  private approval: ReturnType<typeof setTimeout> | null = null;

  snapshot(): JsonObject {
    return {
      github: {
        cliInstalled: true,
        installHint: "brew install gh",
        accounts: this.github.map((a) => ({ ...a })),
        pending: this.pending,
        error: null,
      },
      accounts: this.accounts.map((a) => ({ ...a })),
    };
  }

  async answer(method: string, params: JsonObject): Promise<JsonObject> {
    switch (method) {
      case "_rustcode/connections/github/login":
        this.pending = { code: "4F2A-9C7D", url: "https://github.com/login/device" };
        return this.pending;
      case "_rustcode/connections/github/wait":
        await new Promise<void>((resolve, reject) => {
          this.approval = setTimeout(resolve, APPROVE_MS);
          this.rejectWait = reject;
        });
        this.pending = null;
        this.github = this.github.filter((a) => a.login !== "hubot").map((a) => ({ ...a, active: false }));
        this.github.push({ host: "github.com", login: "hubot", active: true, valid: true, scopes: ["repo"] });
        return this.snapshot();
      case "_rustcode/connections/github/cancel":
        if (this.approval) clearTimeout(this.approval);
        this.pending = null;
        this.rejectWait?.(new Error("GitHub sign-in was cancelled."));
        return {};
      case "_rustcode/connections/github/logout":
        this.github = this.github.filter(
          (a) => !(a.host === str(params.host) && a.login === str(params.login)),
        );
        return this.snapshot();
      case "_rustcode/connections/account/logout":
        this.accounts = this.accounts.filter(
          (a) => !(a.provider === str(params.provider) && a.account === str(params.account)),
        );
        return this.snapshot();
      case "_rustcode/connections/account/refresh":
        await new Promise((r) => setTimeout(r, 600));
        return this.snapshot();
      default:
        return this.snapshot();
    }
  }

  private rejectWait: ((e: Error) => void) | null = null;
}
