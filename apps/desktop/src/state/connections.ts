import { arr, bool, type Json, obj, str } from "../acp/jsonrpc";

/** A GitHub CLI sign-in on this computer. */
export interface GitHubAccount {
  host: string;
  login: string;
  active: boolean;
  valid: boolean;
  scopes: string[];
}

/** A saved model account (`credentials.json`), signed in with `/login`. */
export interface ModelAccount {
  provider: string;
  account: string;
  display: string;
  method: string;
  active: boolean;
  needsSignIn: boolean;
}

/** The one-time code `gh auth login --web` printed, and where to enter it. */
export interface DeviceCode {
  code: string;
  url: string;
}

/** Settings › Connections: the engine's `_rustcode/connections` snapshot. */
export interface Connections {
  github?: {
    cliInstalled: boolean;
    installHint?: string;
    accounts: GitHubAccount[];
    pending?: DeviceCode;
    error?: string;
  };
  accounts: ModelAccount[];
}

export function parseDeviceCode(v: Json): DeviceCode | undefined {
  const o = obj(v);
  const code = str(o?.code);
  const url = str(o?.url);
  return code && url ? { code, url } : undefined;
}

export function parseConnections(v: Json): Connections {
  const root = obj(v) ?? {};
  const gh = obj(root.github);
  return {
    github: gh
      ? {
          cliInstalled: bool(gh.cliInstalled) ?? false,
          installHint: str(gh.installHint),
          accounts: arr(gh.accounts).map((a) => {
            const o = obj(a) ?? {};
            return {
              host: str(o.host) ?? "",
              login: str(o.login) ?? "",
              active: bool(o.active) ?? false,
              valid: bool(o.valid) ?? false,
              scopes: arr(o.scopes).flatMap((s) => (typeof s === "string" ? [s] : [])),
            };
          }),
          pending: parseDeviceCode(gh.pending ?? null),
          error: str(gh.error),
        }
      : undefined,
    accounts: arr(root.accounts).map((a) => {
      const o = obj(a) ?? {};
      return {
        provider: str(o.provider) ?? "",
        account: str(o.account) ?? "",
        display: str(o.display) ?? "",
        method: str(o.method) ?? "",
        active: bool(o.active) ?? false,
        needsSignIn: bool(o.needsSignIn) ?? false,
      };
    }),
  };
}

const PROVIDERS: Record<string, { label: string; mark?: string }> = {
  openai: { label: "OpenAI", mark: "codex" },
  anthropic: { label: "Anthropic", mark: "claude" },
  google: { label: "Google", mark: "gemini" },
  gemini: { label: "Google", mark: "gemini" },
  github_copilot: { label: "GitHub Copilot" },
  openrouter: { label: "OpenRouter" },
  deepseek: { label: "DeepSeek" },
  mistral: { label: "Mistral" },
  xai: { label: "xAI" },
};

const METHODS: Record<string, string> = {
  api_key: "API key",
  chat_gpt: "ChatGPT sign-in",
  github_copilot: "Copilot sign-in",
  claude_cli: "Claude Code sign-in",
};

/** A provider id as people write it: `openai` is OpenAI. An unknown id keeps its own spelling. */
export function providerLabel(provider: string): string {
  return PROVIDERS[provider]?.label ?? provider;
}

/** The harness whose mark stands for a provider (OpenAI is Codex's knot), if one does. */
export function providerMark(provider: string): string | undefined {
  return PROVIDERS[provider]?.mark;
}

/** How an account signed in, in words: `chat_gpt` is "ChatGPT sign-in". */
export function methodLabel(method: string): string {
  return METHODS[method] ?? method.replace(/_/g, " ");
}

/** The model account's second line: provider and how it signed in. */
export function accountDetail(account: ModelAccount): string {
  return [providerLabel(account.provider), methodLabel(account.method)].filter(Boolean).join(" · ");
}

/** A GitHub account's second line: host, and the scopes its token carries when it lists any. */
export function githubDetail(account: GitHubAccount): string {
  return account.scopes.length ? `${account.host} · ${account.scopes.join(", ")}` : account.host;
}
