/**
 * Settings › Connections › Apps: hosted MCP servers that sign in with OAuth (the engine registers
 * a client, or identifies as RustCode's client metadata document where a server such as Vercel
 * accepts only that, so no client id is needed), and a few that need no sign-in. Connecting one
 * adds it as an `http` MCP server in the user config.
 */

export type ConnectorCategory = "work" | "dev" | "design" | "commerce" | "docs";

export interface Connector {
  /** The MCP server name it is added under. */
  id: string;
  name: string;
  description: string;
  url: string;
  category: ConnectorCategory;
  /** Signs in with OAuth in the browser; `false` connects without an account. */
  signIn: boolean;
}

export const CONNECTOR_CATEGORIES: { value: "all" | ConnectorCategory; label: string }[] = [
  { value: "all", label: "All" },
  { value: "work", label: "Work" },
  { value: "dev", label: "Developer" },
  { value: "design", label: "Design" },
  { value: "commerce", label: "Payments" },
  { value: "docs", label: "Docs" },
];

export const CONNECTORS: Connector[] = [
  {
    id: "linear",
    name: "Linear",
    description: "Find, create and update issues and projects",
    url: "https://mcp.linear.app/mcp",
    category: "work",
    signIn: true,
  },
  {
    id: "notion",
    name: "Notion",
    description: "Search, read and write pages and databases",
    url: "https://mcp.notion.com/mcp",
    category: "work",
    signIn: true,
  },
  {
    id: "atlassian",
    name: "Jira & Confluence",
    description: "Work with Jira issues and Confluence pages",
    url: "https://mcp.atlassian.com/v1/mcp",
    category: "work",
    signIn: true,
  },
  {
    id: "intercom",
    name: "Intercom",
    description: "Look up conversations, tickets and contacts",
    url: "https://mcp.intercom.com/mcp",
    category: "work",
    signIn: true,
  },
  {
    id: "granola",
    name: "Granola",
    description: "Read meeting notes and transcripts",
    url: "https://mcp.granola.ai/mcp",
    category: "work",
    signIn: true,
  },
  {
    id: "zapier",
    name: "Zapier",
    description: "Run actions in thousands of apps",
    url: "https://mcp.zapier.com/api/mcp/mcp",
    category: "work",
    signIn: true,
  },
  {
    id: "sentry",
    name: "Sentry",
    description: "Investigate errors, issues and releases",
    url: "https://mcp.sentry.dev/mcp",
    category: "dev",
    signIn: true,
  },
  {
    id: "vercel",
    name: "Vercel",
    description: "Check deployments, logs and projects",
    url: "https://mcp.vercel.com",
    category: "dev",
    signIn: true,
  },
  {
    id: "cloudflare",
    name: "Cloudflare",
    description: "Manage Workers, KV, R2 and D1",
    url: "https://bindings.mcp.cloudflare.com/mcp",
    category: "dev",
    signIn: true,
  },
  {
    id: "neon",
    name: "Neon",
    description: "Query and branch Postgres databases",
    url: "https://mcp.neon.tech/mcp",
    category: "dev",
    signIn: true,
  },
  {
    id: "posthog",
    name: "PostHog",
    description: "Read analytics, flags and experiments",
    url: "https://mcp.posthog.com/mcp",
    category: "dev",
    signIn: true,
  },
  {
    id: "canva",
    name: "Canva",
    description: "Find, create and export designs",
    url: "https://mcp.canva.com/mcp",
    category: "design",
    signIn: true,
  },
  {
    id: "webflow",
    name: "Webflow",
    description: "Edit sites, pages and CMS collections",
    url: "https://mcp.webflow.com/mcp",
    category: "design",
    signIn: true,
  },
  {
    id: "paypal",
    name: "PayPal",
    description: "Invoices, orders and transactions",
    url: "https://mcp.paypal.com/mcp",
    category: "commerce",
    signIn: true,
  },
  {
    id: "square",
    name: "Square",
    description: "Payments, catalog and customers",
    url: "https://mcp.squareup.com/mcp",
    category: "commerce",
    signIn: true,
  },
  {
    id: "context7",
    name: "Context7",
    description: "Current library docs and code examples",
    url: "https://mcp.context7.com/mcp",
    category: "docs",
    signIn: false,
  },
  {
    id: "deepwiki",
    name: "DeepWiki",
    description: "Ask about any public GitHub repository",
    url: "https://mcp.deepwiki.com/mcp",
    category: "docs",
    signIn: false,
  },
  {
    id: "huggingface",
    name: "Hugging Face",
    description: "Search models, datasets and papers",
    url: "https://huggingface.co/mcp",
    category: "docs",
    signIn: false,
  },
];

const originOf = (url: string) => {
  try {
    const parsed = new URL(url);
    return `${parsed.host}${parsed.pathname.replace(/\/+$/, "")}`;
  } catch {
    return url.trim().replace(/\/+$/, "");
  }
};

/** The configured server a connector already is: same URL, whatever it was named. */
export function configuredAs<T extends { name: string; target: string }>(
  connector: Connector,
  servers: readonly T[],
): T | undefined {
  const want = originOf(connector.url);
  return servers.find((server) => originOf(server.target) === want);
}

/** A free name for a connector: its id, or its id with a number when a different server has it. */
export function connectorName(connector: Connector, taken: readonly string[]): string {
  if (!taken.includes(connector.id)) return connector.id;
  let n = 2;
  while (taken.includes(`${connector.id}-${n}`)) n++;
  return `${connector.id}-${n}`;
}

export function matchesConnector(connector: Connector, query: string): boolean {
  const q = query.trim().toLowerCase();
  return !q || `${connector.name} ${connector.description}`.toLowerCase().includes(q);
}
