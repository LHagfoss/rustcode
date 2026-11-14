/** Untrusted external links can stage text and navigation only. */
export const PROMPT_LIMIT = 8000;
export type DeepLink =
  | { kind: "new"; prompt: string; repo?: string; harness?: string; model?: string }
  | { kind: "thread"; id: string }
  | { kind: "connect"; code: string };

export function parseDeepLink(raw: string): DeepLink | null {
  if (raw.length > 100000 || /%(?![\da-f]{2})/i.test(raw)) return null;
  try {
    decodeURIComponent(raw);
    const url = new URL(raw);
    if (url.protocol !== "rustcode:" || url.username || url.password || url.port || url.hash) return null;
    if (url.hostname === "new" && (url.pathname === "" || url.pathname === "/")) {
      const prompt = url.searchParams.get("prompt");
      if (prompt === null) return null;
      const repo = url.searchParams.get("repo") ?? "";
      const pick = (key: string) => {
        const value = url.searchParams.get(key);
        return value &&
          value.length <= 200 &&
          !Array.from(value).some((char) => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127)
          ? value
          : undefined;
      };
      return {
        kind: "new",
        prompt: prompt.slice(0, PROMPT_LIMIT),
        repo: /^[\w.-]+\/[\w.-]+$/.test(repo) ? repo : undefined,
        harness: pick("harness"),
        model: pick("model"),
      };
    }
    const path = decodeURIComponent(url.pathname.slice(1));
    if (url.hostname === "thread" && /^[\w.-]+(?:\/[\w.-]+)?$/.test(path))
      return { kind: "thread", id: path };
    if (url.hostname === "connect" && /^[\w-]+$/.test(path) && path.length <= 16000)
      return { kind: "connect", code: `rustcode://connect/${path}` };
  } catch {
    /* Invalid URLs are inert. */
  }
  return null;
}

export function threadLink(id: string): string {
  return `rustcode://thread/${id.split("/").map(encodeURIComponent).join("/")}`;
}
export function promptLink(prompt: string): string {
  return `rustcode://new?${new URLSearchParams({ prompt: prompt.slice(0, PROMPT_LIMIT) })}`;
}
