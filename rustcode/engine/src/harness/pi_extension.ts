// Written next to the engine's run files and loaded with `pi --extension`: Pi takes no ACP
// `mcpServers`, so this gives it the `rustcode` thread tools by starting the same
// `rustcode mcp-orchestrator` other harnesses get and registering what it lists.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const take = (name: string) => {
  const value = process.env[name];
  // Not for the shell commands the agent runs.
  delete process.env[name];
  return value;
};

export default async function (pi: any) {
  const command = take("RUSTCODE_PI_MCP_COMMAND");
  const args: string[] = JSON.parse(take("RUSTCODE_PI_MCP_ARGS") ?? "[]");
  const token = take("RUSTCODE_ORCHESTRATOR_TOKEN");
  // The user has Smart UI on: the server also offers the block check.
  const smartUi = take("RUSTCODE_SMART_UI");
  if (!command || !token) return;

  const server = spawn(command, args, {
    stdio: ["pipe", "pipe", "ignore"],
    env: {
      ...process.env,
      RUSTCODE_ORCHESTRATOR_TOKEN: token,
      ...(smartUi ? { RUSTCODE_SMART_UI: smartUi } : {}),
    },
  });
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
  server.on("exit", () => fail("the rustcode tool server stopped"));
  server.on("error", () => fail("the rustcode tool server could not start"));
  process.on("exit", () => server.kill());

  const request = (method: string, params: unknown, signal?: AbortSignal) =>
    new Promise<any>((resolve, reject) => {
      const id = ++lastId;
      const onAbort = () => {
        waiting.delete(id);
        server.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/cancelled", params: { requestId: id } })}\n`);
        reject(new Error("cancelled"));
      };
      signal?.addEventListener("abort", onAbort, { once: true });
      waiting.set(id, (message) => {
        signal?.removeEventListener("abort", onAbort);
        if (message.error) reject(new Error(message.error.message));
        else resolve(message.result);
      });
      server.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    });

  await request("initialize", {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "pi", version: "1" },
  });
  server.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
  const { tools } = await request("tools/list", {});
  for (const tool of tools) {
    pi.registerTool({
      name: `mcp__rustcode__${tool.name}`,
      label: tool.name,
      description: tool.description,
      parameters: tool.inputSchema,
      async execute(_id: string, params: unknown, signal?: AbortSignal) {
        const result = await request("tools/call", { name: tool.name, arguments: params ?? {} }, signal);
        return { content: result.content, details: {}, isError: result.isError === true };
      },
    });
  }
}
