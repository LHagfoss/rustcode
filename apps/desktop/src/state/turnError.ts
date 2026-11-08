// A failed turn in words. A harness passes on what the model provider answered,
// which is often a raw JSON body (`Codex: {"detail":"Bad Request"}`): this reads
// the message out of it, names the harness and the HTTP status, and says what to
// do when the conversation is close to full. The raw text stays on the thread
// (`ThreadState.errorDetail`), behind the error's Details. Pure, so it is tested
// on its own.

/** From this share of the context window up, a rejected request is most likely the size. */
export const FULL_CONTEXT_PERCENT = 90;
const MAX_DISPLAY_ERROR_CHARS = 500;
const MAX_PROVIDER_MESSAGE_CHARS = 240;

function clipped(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1).trimEnd()}…` : text;
}

/** The standard reason phrases a provider body often says instead of a code. */
const REASONS: Record<number, string> = {
  400: "Bad Request",
  401: "Unauthorized",
  403: "Forbidden",
  404: "Not Found",
  408: "Request Timeout",
  409: "Conflict",
  413: "Payload Too Large",
  422: "Unprocessable Entity",
  429: "Too Many Requests",
  500: "Internal Server Error",
  502: "Bad Gateway",
  503: "Service Unavailable",
  504: "Gateway Timeout",
};

const statusOfReason = (text: string): number | undefined => {
  const wanted = text.trim().toLowerCase();
  const hit = Object.entries(REASONS).find(([, reason]) => reason.toLowerCase() === wanted);
  return hit ? Number(hit[0]) : undefined;
};

const isStatus = (value: unknown): value is number =>
  typeof value === "number" && Number.isInteger(value) && value >= 400 && value <= 599;

type Body = Record<string, unknown>;

const isBody = (value: unknown): value is Body =>
  !!value && typeof value === "object" && !Array.isArray(value);

/** The first JSON object in `text`, when there is one. */
function jsonIn(text: string): { body: Body; before: string } | undefined {
  const start = text.indexOf("{");
  const end = text.lastIndexOf("}");
  if (start < 0 || end <= start) return undefined;
  try {
    const body: unknown = JSON.parse(text.slice(start, end + 1));
    return isBody(body) ? { body, before: text.slice(0, start) } : undefined;
  } catch {
    return undefined;
  }
}

/** A field as one line of text: a string, or the `msg`/`message` of the first entry of a list. */
function said(value: unknown): string | undefined {
  if (typeof value === "string") return value.trim() || undefined;
  if (Array.isArray(value)) return said(value[0]);
  if (isBody(value)) return said(value.message) ?? said(value.msg) ?? said(value.detail);
  return undefined;
}

/** What a provider's body says went wrong: `detail`, `error.message`, `message`, or `error`. */
function messageOf(body: Body): string | undefined {
  return (
    said(body.detail) ??
    said(isBody(body.error) ? body.error : undefined) ??
    said(body.message) ??
    said(body.error)
  );
}

/** The HTTP status in the body (`status`, `status_code`, `error.code`) or the text before it. */
function statusIn(body: Body | undefined, before: string): number | undefined {
  const inner = isBody(body?.error) ? (body.error as Body) : undefined;
  for (const value of [body?.status, body?.status_code, body?.statusCode, inner?.status, inner?.code]) {
    if (isStatus(value)) return value;
    if (typeof value === "string" && /^[45]\d\d$/.test(value)) return Number(value);
  }
  // `HTTP 400`, `status: 400`, `400 Bad Request`, or a bare code just before the body (`API Error: 429 {…}`).
  const match =
    /\b(?:HTTP|status)(?:\s+code)?\s*:?\s*([45]\d\d)\b/i.exec(before) ??
    /\b([45]\d\d)\s+[A-Z][a-z]/.exec(before) ??
    /(?:^|[\s:])([45]\d\d)\s*$/.exec(before);
  return match ? Number(match[1]) : undefined;
}

/**
 * `raw` said readably, or `raw` itself when there is nothing to read out of it. A provider's
 * rejection becomes `Codex rejected the request (HTTP 400 Bad Request).`; with the context at
 * {@link FULL_CONTEXT_PERCENT} or more, a hint to compact or start over follows, since a full
 * window is the usual reason a request that worked before is refused.
 */
export function readableTurnError(raw: string, contextPercent?: number): string {
  const text = raw.trim();
  // `Codex: …`, `Claude Code: …`: the harness the engine says it came from.
  const prefixed = /^([A-Z][\w .-]{0,30}?):\s+([\s\S]+)$/.exec(text);
  const who = prefixed?.[1];
  const rest = prefixed?.[2] ?? text;
  // Proxy error pages can include a JSON-looking example or script; their markup is still the
  // error body, so do not parse it as a provider envelope or infer context exhaustion from it.
  if (/<(?:!doctype\s+html|html|body)\b/i.test(rest)) return clipped(text, MAX_DISPLAY_ERROR_CHARS);
  const json = jsonIn(rest);
  const body = json?.body;
  const message = body ? messageOf(body) : undefined;
  const status = statusIn(body, json?.before ?? rest) ?? (message ? statusOfReason(message) : undefined);
  const full = contextPercent !== undefined && contextPercent >= FULL_CONTEXT_PERCENT;
  const clientSide = status === undefined || status < 500;
  const hint = `The conversation is ${contextPercent}% full — compact it or start a new thread.`;
  // No body: the text is already words, and stays; a refusal of a full conversation still says why.
  if (!body) {
    if (status !== undefined && status < 500 && full) {
      const prefix = clipped(text.replace(/[.\s]+$/, ""), MAX_DISPLAY_ERROR_CHARS - hint.length - 2);
      return `${prefix}. ${hint}`;
    }
    return clipped(text, MAX_DISPLAY_ERROR_CHARS);
  }
  if (!message && status === undefined) return clipped(text, MAX_DISPLAY_ERROR_CHARS);
  const reason = status !== undefined ? REASONS[status] : undefined;
  const code = status !== undefined ? `HTTP ${status}${reason ? ` ${reason}` : ""}` : undefined;
  const subject = who ?? "The provider";
  const verb =
    status !== undefined && status >= 500 ? "could not answer the request" : "rejected the request";
  const same = !message || (reason !== undefined && message.toLowerCase() === reason.toLowerCase());
  const shortMessage = message
    ? clipped(message.replace(/\s+/g, " "), MAX_PROVIDER_MESSAGE_CHARS)
    : undefined;
  const sentence = same
    ? `${subject} ${verb} (${code ?? message}).`
    : `${subject} ${verb}${code ? ` (${code})` : ""}: ${shortMessage!.replace(/[.\s]+$/, "")}.`;
  return clipped(full && clientSide ? `${sentence} ${hint}` : sentence, MAX_DISPLAY_ERROR_CHARS);
}
