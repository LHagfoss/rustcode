import { realpathSync } from "node:fs";
import { register } from "node:module";
import { pathToFileURL } from "node:url";
import { isMainThread } from "node:worker_threads";

if (isMainThread && /(?:pi-acp|index\.js)$/.test(process.argv[1] ?? "")) {
  const entry = realpathSync(process.argv[1]);
  if (entry?.replaceAll("\\", "/").endsWith("/node_modules/pi-acp/dist/index.js")) {
    register(import.meta.url, { data: pathToFileURL(entry).href });
  }
}

let adapterUrl;
export function initialize(url) {
  adapterUrl = url;
}

export async function load(url, context, nextLoad) {
  const result = await nextLoad(url, context);
  if (url !== adapterUrl) return result;

  const source = result.source.toString();
  const initialize = "  async initialize(params) {";
  const protocol = "      protocolVersion: requested === supportedVersion ? requested : supportedVersion,";
  if (!source.includes(initialize) || !source.includes(protocol) || source.includes("async extMethod(")) {
    return result;
  }
  return {
    ...result,
    source: source.replace(initialize, `  async extMethod(method, params) {
    if (method !== "_session/steering") throw RequestError3.methodNotFound(method);
    const session = this.sessions.maybeGet(params.sessionId);
    if (!session?.pendingTurn || session.cancelRequested) {
      return { outcome: "promptRequired", reason: "noRunningTurn" };
    }
    const { message, images } = promptToPiMessage(params.prompt);
    const response = await session.proc.request({ type: "steer", message, images });
    if (!response.success) throw RequestError3.internalError({}, response.error);
    return { outcome: "injected" };
  }
${initialize}`).replace(protocol, `${protocol}
      _meta: { steering: { supported: true } },`),
  };
}
