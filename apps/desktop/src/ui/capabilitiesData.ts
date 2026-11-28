import { arr, bool, type Json, num, obj, str } from "../acp/jsonrpc";

export interface Skill {
  name: string;
  description: string;
  path: string;
  source: string;
  /** A harness owns the folder: listed here, never written by RustCode. */
  readOnly: boolean;
  enabled: boolean;
}

export interface McpServer {
  name: string;
  transport: string;
  target: string;
  enabled: boolean;
  scope: string;
  connection: string;
  tools: string[];
  toolCount: number;
  error?: string;
  envKeys: string[];
  headerKeys: string[];
  copyTarget: string[] | null;
  /** A remote server that signs in with OAuth: whether RustCode holds its token. */
  auth?: "signed_in" | "signed_out";
}

/** A browser sign-in waits up to five minutes for the user (the engine's loopback limit). */
export const SIGN_IN_TIMEOUT = 310_000;

export interface Capabilities {
  skills: Skill[];
  mcpServers: McpServer[];
  configPath?: string;
}

/** The engine sends `tools` as the tool names; an older mock sent a count. */
export const parseCapabilities = (v: Json): Capabilities => {
  const root = obj(v) ?? {};
  return {
    configPath: str(root.configPath),
    skills: arr(root.skills).map((s) => {
      const o = obj(s) ?? {};
      return {
        name: str(o.name) ?? "",
        description: str(o.description) ?? "",
        path: str(o.path) ?? "",
        source: str(o.source) ?? "",
        readOnly: bool(o.readOnly) ?? false,
        enabled: bool(o.enabled) ?? false,
      };
    }),
    mcpServers: arr(root.mcpServers).map((s) => {
      const o = obj(s) ?? {};
      const tools = Array.isArray(o.tools)
        ? arr(o.tools)
            .map((t) => str(t) ?? "")
            .filter(Boolean)
        : [];
      return {
        name: str(o.name) ?? "",
        transport: str(o.transport) ?? "",
        target: str(o.target) ?? "",
        enabled: bool(o.enabled) ?? false,
        scope: str(o.scope) ?? "user",
        connection: str(o.connection) ?? "",
        tools,
        toolCount: Array.isArray(o.tools) ? tools.length : (num(o.tools) ?? 0),
        error: str(o.error),
        envKeys: arr(o.envKeys).flatMap((key) => (typeof key === "string" ? [key] : [])),
        headerKeys: arr(o.headerKeys).flatMap((key) => (typeof key === "string" ? [key] : [])),
        copyTarget: Array.isArray(o.copyTarget)
          ? arr(o.copyTarget).flatMap((part) => (typeof part === "string" ? [part] : []))
          : null,
        auth: o.auth === "signed_in" || o.auth === "signed_out" ? o.auth : undefined,
      };
    }),
  };
};
