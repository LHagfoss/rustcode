// Written next to the engine's run files and loaded with `pi --extension`: Pi takes no ACP
// `mcpServers`, so this gives it the `rustcode` thread tools and the user's own MCP servers by
// starting them itself and registering what each lists as `mcp__<server>__<tool>`.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const PROTOCOL = "2025-06-18";
const START_TIMEOUT_MS = 30_000;

const take = (name: string) => {
  const value = process.env[name];
  // Not for the shell commands the agent runs.
  delete process.env[name];
  return value;
};

type Request = (method: string, params: unknown, signal?: AbortSignal) => Promise<any>;
type Client = { request: Request; notify: (method: string) => void };

type ServerSpec = {
  name: string;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  url?: string;
  headers?: Record<string, string>;
};

function stdioClient(command: string, args: string[], env: Record<string, string | undefined>): Client {
  const server = spawn(command, args, { stdio: ["pipe", "pipe", "ignore"], env });
  const waiting = new Map<number, (message: any) => void>();
  let lastId = 0;
  createInterface({ input: server.stdout }).on("line", (line) => {
    try {
      const message = JSON.parse(line);
      waiting.get(message.id)?.(message);
      waiting.delete(message.id);
    } catch {
      // Not a JSON-RPC line.
    }
  });
  const fail = (error: string) => {
    for (const answer of waiting.values()) answer({ error: { message: error } });
    waiting.clear();
  };
  server.on("exit", () => fail(`the ${command} tool server stopped`));
  server.on("error", () => fail(`the ${command} tool server could not start`));
  process.on("exit", () => server.kill());
  const send = (message: unknown) => server.stdin.write(`${JSON.stringify(message)}\n`);
  const request: Request = (method, params, signal) =>
    new Promise<any>((resolve, reject) => {
      const id = ++lastId;
      const onAbort = () => {
        waiting.delete(id);
        send({ jsonrpc: "2.0", method: "notifications/cancelled", params: { requestId: id } });
        reject(new Error("cancelled"));
      };
      signal?.addEventListener("abort", onAbort, { once: true });
      waiting.set(id, (message) => {
        signal?.removeEventListener("abort", onAbort);
        if (message.error) reject(new Error(message.error.message));
        else resolve(message.result);
      });
      send({ jsonrpc: "2.0", id, method, params });
    });
  return { request, notify: (method) => send({ jsonrpc: "2.0", method }) };
}

async function eventStreamAnswer(body: ReadableStream<Uint8Array>, id: number) {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let data: string[] = [];
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) throw new Error("the MCP server closed the stream without an answer");
      buffer += decoder.decode(value, { stream: true });
      let end: number;
      while ((end = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, end).replace(/\r$/, "");
        buffer = buffer.slice(end + 1);
        if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
        else if (line === "" && data.length > 0) {
          const message = JSON.parse(data.join("\n"));
          data = [];
          if (message.id === id) return message;
        }
      }
    }
  } finally {
    reader.cancel().catch(() => {});
  }
}

function httpClient(url: string, headers: Record<string, string>): Client {
  let session: string | undefined;
  let lastId = 0;
  const post = async (message: any, signal?: AbortSignal) => {
    const response = await fetch(url, {
      method: "POST",
      signal,
      headers: {
        ...headers,
        "content-type": "application/json",
        accept: "application/json, text/event-stream",
        "mcp-protocol-version": PROTOCOL,
        ...(session ? { "mcp-session-id": session } : {}),
      },
      body: JSON.stringify(message),
    });
    session = response.headers.get("mcp-session-id") ?? session;
    if (!response.ok) throw new Error(`${url} answered HTTP ${response.status}`);
    if (message.id === undefined || !response.body) return undefined;
    const type = response.headers.get("content-type") ?? "";
    return type.includes("text/event-stream") ? eventStreamAnswer(response.body, message.id) : response.json();
  };
  const request: Request = async (method, params, signal) => {
    const answer = await post({ jsonrpc: "2.0", id: ++lastId, method, params }, signal);
    if (answer?.error) throw new Error(answer.error.message);
    return answer?.result;
  };
  return { request, notify: (method) => void post({ jsonrpc: "2.0", method }).catch(() => {}) };
}

const toolName = (server: string, tool: string) =>
  `mcp__${server}__${tool}`.replace(/[^A-Za-z0-9_-]/g, "_").slice(0, 64);

async function register(pi: any, server: string, client: Client) {
  await client.request("initialize", {
    protocolVersion: PROTOCOL,
    capabilities: {},
    clientInfo: { name: "pi", version: "1" },
  });
  client.notify("notifications/initialized");
  const { tools } = await client.request("tools/list", {});
  for (const tool of tools ?? []) {
    pi.registerTool({
      name: toolName(server, tool.name),
      label: tool.name,
      description: tool.description ?? "",
      parameters: tool.inputSchema ?? { type: "object", properties: {} },
      async execute(_id: string, params: unknown, signal?: AbortSignal) {
        const result = await client.request("tools/call", { name: tool.name, arguments: params ?? {} }, signal);
        return { content: result?.content ?? [], details: {}, isError: result?.isError === true };
      },
    });
  }
}

const withTimeout = (work: Promise<void>) =>
  Promise.race([work, new Promise<void>((resolve) => setTimeout(resolve, START_TIMEOUT_MS).unref())]);

const SERVER_SETTINGS = ["RUSTCODE_SMART_UI", "RUSTCODE_ASK_USER", "RUSTCODE_DEFER_TOOLS", "RUSTCODE_ORCHESTRATION_VERSION"];

export default async function (pi: any) {
  const command = take("RUSTCODE_PI_MCP_COMMAND");
  const args: string[] = JSON.parse(take("RUSTCODE_PI_MCP_ARGS") ?? "[]");
  const token = take("RUSTCODE_ORCHESTRATOR_TOKEN");
  const servers: ServerSpec[] = JSON.parse(take("RUSTCODE_PI_MCP_SERVERS") ?? "[]");
  const skillPaths: string[] = JSON.parse(take("RUSTCODE_PI_SKILLS") ?? "[]");
  pi.on("resources_discover", async () => ({ skillPaths }));
  const settings = Object.fromEntries(SERVER_SETTINGS.flatMap((name) => (process.env[name] ? [[name, process.env[name]]] : [])));

  const starts: Promise<void>[] = [];
  if (command && token) {
    const env = { ...process.env, ...settings, RUSTCODE_ORCHESTRATOR_TOKEN: token };
    starts.push(register(pi, "rustcode", stdioClient(command, args, env)));
  }
  for (const server of servers) {
    const client = server.url
      ? httpClient(server.url, server.headers ?? {})
      : stdioClient(server.command ?? "", server.args ?? [], { ...process.env, ...server.env });
    starts.push(register(pi, server.name, client));
  }
  await Promise.allSettled(starts.map(withTimeout));
}
