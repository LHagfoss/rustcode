// Stand-ins for RustCode's orchestration MCP server, for the thread-tools
// eval: it registers each tool the engine's `rustcode mcp-orchestrator`
// lists under the name a harness sees (`mcp__rustcode__<tool>`), with that
// tool's real description and schema, and answers it from canned data.
// Nothing reaches an engine, the app or a thread.
//
//   EVAL_TOOLS  file with the `tools/list` entries (name, description, inputSchema)
//   EVAL_MOCKS  JSON map tool name -> canned result (default `{}`)
//   EVAL_LOG    file to append `{ tool, args }` to on each call

import { appendFileSync, readFileSync } from "node:fs";

interface McpTool {
  name: string;
  description?: string;
  inputSchema?: Record<string, unknown>;
}

const label = (name: string) =>
  name
    .split("_")
    .filter(Boolean)
    .map((word) => word[0]!.toUpperCase() + word.slice(1))
    .join(" ");

const empty = () => ({ type: "object", properties: {}, required: [], additionalProperties: false });

export default async function (pi: {
  registerTool(tool: {
    name: string;
    label: string;
    description: string;
    parameters: Record<string, unknown>;
    execute(
      id: string,
      params: Record<string, unknown>,
    ): Promise<{ content: { type: "text"; text: string }[]; details: Record<string, unknown> }>;
  }): void;
}) {
  const tools = JSON.parse(readFileSync(process.env.EVAL_TOOLS ?? "", "utf8")) as McpTool[];
  const mocks = JSON.parse(process.env.EVAL_MOCKS ?? "{}") as Record<string, unknown>;
  const log = process.env.EVAL_LOG;

  for (const tool of tools) {
    pi.registerTool({
      name: `mcp__rustcode__${tool.name}`,
      label: label(tool.name),
      description: tool.description ?? tool.name,
      parameters: tool.inputSchema ?? empty(),
      async execute(_id, params) {
        if (log) appendFileSync(log, `${JSON.stringify({ tool: tool.name, args: params })}\n`);
        // A string mock is a tool's prose answer (the block check); anything
        // else is JSON, as the orchestration tools answer.
        const canned = mocks[tool.name] ?? {};
        return {
          content: [
            { type: "text", text: typeof canned === "string" ? canned : JSON.stringify(canned) },
          ],
          details: {},
        };
      },
    });
  }
}
