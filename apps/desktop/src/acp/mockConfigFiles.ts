import { arr, type JsonObject, obj, str } from "./jsonrpc";

/** One mock machine's `skills/`, `themes/` and `commands/`, keyed `kind/name`, with the machine each copy came from. */
export class MockConfigFiles {
  private entries = new Map<string, { content: string; source?: string }>();

  constructor(seed: Record<string, string> = {}) {
    for (const [key, content] of Object.entries(seed)) this.entries.set(key, { content });
  }

  /** `_rustcode/config/files/*` as `rustcode/engine/src/acp/config_files.rs` answers it. */
  answer(method: string, params: JsonObject): JsonObject {
    if (method === "_rustcode/config/files/list") {
      const kinds = arr(params.kinds).map(String);
      return {
        entries: [...this.entries.entries()]
          .map(([key, value]) => ({ key, ...value }))
          .filter(({ key }) => !kinds.length || kinds.includes(key.split("/")[0] ?? ""))
          .map(({ key, content }) => ({
            kind: key.split("/")[0] ?? "",
            name: key.split("/").slice(1).join("/"),
            hash: content,
            size: content.length,
          })),
      };
    }
    if (method === "_rustcode/config/files/get") {
      const key = `${str(params.kind)}/${str(params.name)}`;
      const entry = this.entries.get(key);
      if (!entry)
        throw Object.assign(new Error(`there is no entry named '${str(params.name)}'`), { code: -32602 });
      return {
        kind: str(params.kind) ?? "",
        name: str(params.name) ?? "",
        files: [{ path: ".", content: entry.content }],
      };
    }
    if (method === "_rustcode/config/files/put") {
      const entry = obj(params.entry);
      const key = `${str(entry?.kind)}/${str(entry?.name)}`;
      const content = str(obj(arr(entry?.files)[0])?.content) ?? "";
      const same = this.entries.get(key)?.content === content;
      this.entries.set(key, { content, source: str(params.source) });
      return { outcome: same ? "unchanged" : "written" };
    }
    const kind = str(params.kind) ?? "";
    const keep = arr(params.keep).map(String);
    const removed: string[] = [];
    for (const [key, value] of this.entries) {
      const name = key.split("/").slice(1).join("/");
      if (key.startsWith(`${kind}/`) && value.source === str(params.source) && !keep.includes(name)) {
        this.entries.delete(key);
        removed.push(name);
      }
    }
    return { removed };
  }
}
