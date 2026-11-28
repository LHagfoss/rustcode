import { useCallback, useEffect, useRef, useState } from "react";
import type { JsonObject } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { machineRequest } from "../state/settingsScope";
import { errorText, toast } from "../state/store";
import { openSettings } from "../state/viewActions";
import { type Capabilities, type McpServer, parseCapabilities, SIGN_IN_TIMEOUT } from "./capabilitiesData";
import {
  CONNECTOR_CATEGORIES,
  CONNECTORS,
  type Connector,
  type ConnectorCategory,
  configuredAs,
  connectorName,
  matchesConnector,
} from "./connectorCatalog";
import { CONNECTOR_LOGOS } from "./connectorLogos";
import { Search } from "./icons";
import { cx, noTextHelpers, Segments, Spinner } from "./primitives";
import { Action, Chip, fieldClass, SectionTitle } from "./settingsKit";

type Step = "adding" | "signing-in" | "removing";
type Busy = { id: string; step: Step } | null;

/**
 * Settings › Connections › Apps: a gallery of hosted MCP servers. Connect adds one to this
 * computer's user config and, when it asks for an account, runs the engine's browser sign-in;
 * it then lists under Capabilities › MCP servers like any other.
 */
export function ConnectorGallery() {
  const [data, setData] = useState<Capabilities | null>(null);
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<"all" | ConnectorCategory>("all");
  const [busy, setBusy] = useState<Busy>(null);
  const request = useCallback(async (method: string, params: JsonObject = {}, timeout = 30_000) => {
    const next = parseCapabilities(await machineRequest("", method, params, timeout));
    setData(next);
    return next;
  }, []);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    request(Method.capabilities).catch((e) => toast(errorText(e), "error"));
    return () => {
      mounted.current = false;
    };
  }, [request]);

  const announce = (connector: Connector, server: McpServer | undefined) => {
    if (server?.connection === "connected") toast(`${connector.name} is connected.`, "success");
    else if (server?.error) toast(`${connector.name}: ${server.error}`, "error");
  };

  /** Adding a server that needs an account already starts its browser sign-in: wait for it to land. */
  const awaitSignIn = async (connector: Connector, name: string) => {
    setBusy({ id: connector.id, step: "signing-in" });
    toast(`Finish signing in to ${connector.name} in your browser.`, "info");
    const deadline = Date.now() + SIGN_IN_TIMEOUT;
    while (mounted.current && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 1500));
      const server = (await request(Method.capabilities)).mcpServers.find((s) => s.name === name);
      if (!server || server.connection === "connected" || server.connection === "failed" || server.error) {
        announce(connector, server);
        return;
      }
    }
  };

  const signIn = async (connector: Connector, name: string) => {
    setBusy({ id: connector.id, step: "signing-in" });
    toast(`Finish signing in to ${connector.name} in your browser.`, "info");
    const next = await request(Method.mcpRestart, { name }, SIGN_IN_TIMEOUT);
    announce(
      connector,
      next.mcpServers.find((s) => s.name === name),
    );
  };

  const connect = async (connector: Connector) => {
    setBusy({ id: connector.id, step: "adding" });
    try {
      const name = connectorName(
        connector,
        (data?.mcpServers ?? []).map((s) => s.name),
      );
      const next = await request(
        Method.mcpAdd,
        { name, target: [connector.url], transport: "http", headers: [] },
        45_000,
      );
      const server = next.mcpServers.find((s) => s.name === name);
      if (server && server.connection !== "connected" && !server.error) await awaitSignIn(connector, name);
      else announce(connector, server);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const remove = async (connector: Connector, server: McpServer) => {
    setBusy({ id: connector.id, step: "removing" });
    try {
      await request(Method.mcpRemove, { name: server.name }, 15_000);
      toast(`${connector.name} was removed.`, "info");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const reconnect = async (connector: Connector, server: McpServer) => {
    try {
      await signIn(connector, server.name);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const servers = data?.mcpServers ?? [];
  const shown = sortConnected(
    CONNECTORS.filter((c) => (category === "all" || c.category === category) && matchesConnector(c, query)),
    servers,
  );
  const connectedCount = CONNECTORS.filter(
    (c) => configuredAs(c, servers)?.connection === "connected",
  ).length;
  return (
    <section className="mb-6" data-setting="Apps">
      <SectionTitle
        title="Apps"
        action={
          <Action size="sm" quiet onClick={() => openSettings("capabilities")}>
            Manage servers
          </Action>
        }
      />
      <p className="mb-3 max-w-[70ch] px-4 text-callout text-fg-muted">
        Connect an app to give every thread its tools. Sign-ins open in your browser and stay on this
        computer.
        {connectedCount > 0 ? ` ${connectedCount} connected.` : ""}
      </p>
      <div className="mb-3 flex flex-wrap items-center gap-2 px-1">
        <label
          className={cx(
            fieldClass,
            "flex min-w-[180px] flex-1 items-center gap-2 focus-within:bg-fill-strong",
          )}
        >
          <Search size={14} className="shrink-0 text-fg-faint" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search apps"
            aria-label="Search apps"
            {...noTextHelpers}
            className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-fg-faint"
          />
        </label>
        <Segments label="Category" value={category} options={CONNECTOR_CATEGORIES} onChange={setCategory} />
      </div>
      {shown.length === 0 ? (
        <p className="px-4 py-6 text-callout text-fg-muted">No apps match.</p>
      ) : (
        <ul className="m-0 grid list-none grid-cols-1 gap-2 p-0 sm:grid-cols-2">
          {shown.map((connector) => (
            <ConnectorCard
              key={connector.id}
              connector={connector}
              server={data ? configuredAs(connector, data.mcpServers) : undefined}
              loading={!data}
              busy={busy?.id === connector.id ? busy.step : null}
              disabled={busy !== null && busy.id !== connector.id}
              onConnect={() => void connect(connector)}
              onSignIn={(server) => void reconnect(connector, server)}
              onRemove={(server) => void remove(connector, server)}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

function ConnectorCard({
  connector,
  server,
  loading,
  busy,
  disabled,
  onConnect,
  onSignIn,
  onRemove,
}: {
  connector: Connector;
  server: McpServer | undefined;
  loading: boolean;
  busy: Step | null;
  disabled: boolean;
  onConnect: () => void;
  onSignIn: (server: McpServer) => void;
  onRemove: (server: McpServer) => void;
}) {
  const state = cardState(server);
  const status = busy ? BUSY_TEXT[busy] : cardStatus(connector, server, state);
  const problem = !busy && state !== "connected" && Boolean(server?.error);
  return (
    <li
      className={cx(
        "flex min-h-[68px] items-center gap-3 rounded-xl px-3 py-2",
        state === "connected" ? "bg-success/6" : state === "failed" ? "bg-danger/6" : "bg-surface",
      )}
      data-connector={connector.id}
      data-state={busy ?? state}
    >
      <ConnectorLogo connector={connector} />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5">
          <span className="truncate font-medium text-body">{connector.name}</span>
          {state === "connected" && !busy ? (
            <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-success" />
          ) : null}
        </span>
        <span
          title={problem ? server?.error : undefined}
          className={cx("block truncate text-caption", problem ? "text-danger" : "text-fg-muted")}
        >
          {status}
        </span>
      </span>
      {busy ? (
        <Spinner size={14} />
      ) : state === "connected" ? (
        <Chip tone="ok">Connected</Chip>
      ) : state === "signed-out" && server ? (
        <Action size="sm" disabled={disabled} onClick={() => onSignIn(server)}>
          Sign in
        </Action>
      ) : state === "failed" && server ? (
        <span className="flex shrink-0 items-center gap-1">
          <Action size="sm" quiet disabled={disabled} onClick={() => onRemove(server)}>
            Remove
          </Action>
          <Action size="sm" disabled={disabled} onClick={() => onSignIn(server)}>
            Retry
          </Action>
        </span>
      ) : state === "off" ? (
        <Chip tone="idle">Off</Chip>
      ) : state === "starting" ? (
        <Chip tone="warn">Starting</Chip>
      ) : (
        <Action size="sm" disabled={loading || disabled} onClick={onConnect}>
          {connector.signIn ? "Connect" : "Add"}
        </Action>
      )}
    </li>
  );
}

type CardState = "available" | "connected" | "signed-out" | "failed" | "off" | "starting";

const BUSY_TEXT: Record<Step, string> = {
  adding: "Adding…",
  "signing-in": "Waiting for the browser sign-in…",
  removing: "Removing…",
};

/** Where a catalog app stands on this computer, from its configured server (if any). */
export function cardState(server: McpServer | undefined): CardState {
  if (!server) return "available";
  if (server.connection === "connected") return "connected";
  if (!server.enabled) return "off";
  if (server.auth === "signed_out") return "signed-out";
  if (server.error || server.connection === "failed") return "failed";
  return "starting";
}

function cardStatus(connector: Connector, server: McpServer | undefined, state: CardState): string {
  if (!server) return connector.description;
  switch (state) {
    case "connected":
      return `${server.toolCount} tool${server.toolCount === 1 ? "" : "s"}${server.auth === "signed_in" ? " · Signed in" : ""}`;
    case "failed":
      return server.error ? firstLine(server.error) : "Could not connect";
    case "signed-out":
      return server.error ? firstLine(server.error) : "Sign in to finish connecting";
    case "off":
      return "Turned off in Manage servers";
    default:
      return "Connecting…";
  }
}

/** The reason, without the engine's `MCP OAuth for '<name>':` prefix the card already shows. */
export const firstLine = (text: string) => {
  const line = text.split("\n")[0]?.trim() || text;
  return line.replace(/^MCP (?:OAuth )?for '[^']*':\s*/, "");
};

const RANK: Record<CardState, number> = {
  connected: 0,
  "signed-out": 1,
  failed: 1,
  starting: 1,
  off: 2,
  available: 3,
};

/** Apps already set up on this computer first, then the rest in catalog order. */
function sortConnected(connectors: Connector[], servers: readonly McpServer[]): Connector[] {
  return connectors
    .map((connector, index) => ({
      connector,
      index,
      rank: RANK[cardState(configuredAs(connector, servers))],
    }))
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .map((entry) => entry.connector);
}

function ConnectorLogo({ connector }: { connector: Connector }) {
  const path = CONNECTOR_LOGOS[connector.id];
  return (
    <span
      aria-hidden
      className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-fill font-semibold text-body text-fg-muted"
    >
      {path ? (
        <svg viewBox="0 0 24 24" width={18} height={18} fill="currentColor" className="text-fg" aria-hidden>
          <path d={path} />
        </svg>
      ) : (
        connector.name.charAt(0)
      )}
    </span>
  );
}
