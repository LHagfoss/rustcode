import { arr, type JsonObject, str } from "./jsonrpc";

const NO_SIGN_IN = /context7|deepwiki|huggingface/;

const signedOut = (server: JsonObject): JsonObject => ({
  ...server,
  connection: "not_connected",
  tools: 0,
  auth: "signed_out",
});

/**
 * The mock's remote MCP servers that sign in with OAuth: the seeded `sentry` row, a `vercel` row
 * whose sign-in failed, and any server added through `_rustcode/capabilities/mcp/add`, which signs itself in a moment later as the
 * engine's browser login would. Reconnecting one signs it in; logging out forgets it.
 */
export class MockAddedMcp {
  servers: JsonObject[] = [
    signedOut({
      name: "sentry",
      transport: "http",
      target: "https://mcp.sentry.dev/mcp",
      enabled: true,
      scope: "user",
      error: null,
    }),
    {
      name: "vercel",
      transport: "http",
      target: "https://mcp.vercel.com",
      enabled: true,
      scope: "user",
      connection: "failed",
      tools: 0,
      error: "MCP OAuth for 'vercel': token request failed (HTTP 401): invalid_client",
    },
  ];

  apply(method: string, params: JsonObject) {
    const name = str(params.name);
    if (method.endsWith("/remove")) this.servers = this.servers.filter((server) => str(server.name) !== name);
    else if (method.endsWith("/logout"))
      this.servers = this.servers.map((server) => (str(server.name) === name ? signedOut(server) : server));
    else this.add(params);
  }

  private add(params: JsonObject) {
    const name = str(params.name) ?? "server";
    const target = arr(params.target).map(String);
    const http = str(params.transport) === "http";
    const ready = !http || NO_SIGN_IN.test(target[0] ?? "");
    const server: JsonObject = {
      name,
      transport: http ? "http" : "stdio",
      target: target.join(" "),
      enabled: true,
      scope: "user",
      connection: "connected",
      tools: 4,
      error: null,
    };
    this.servers = [
      ...this.servers.filter((entry) => str(entry.name) !== name),
      ready ? server : signedOut(server),
    ];
    if (http && !ready) setTimeout(() => this.signIn(name), 2500);
  }

  /** Signs a server in; `false` when it is not one of these. */
  signIn(name: string): boolean {
    if (!this.servers.some((server) => str(server.name) === name)) return false;
    this.servers = this.servers.map((server) =>
      str(server.name) === name
        ? {
            ...server,
            connection: "connected",
            tools: name === "sentry" ? 58 : 12,
            auth: "signed_in",
            error: null,
          }
        : server,
    );
    return true;
  }
}
