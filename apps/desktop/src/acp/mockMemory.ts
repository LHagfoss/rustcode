import { type Json, type JsonObject, str } from "./jsonrpc";

const DAY = 86_400;

/** The mock engine's saved facts, as `_rustcode/memory/list` and `/forget` answer them. */
export class MockMemory {
  private project: JsonObject[] = [];
  private global: JsonObject[] = [];

  constructor() {
    const now = Math.floor(Date.now() / 1000);
    const fact = (
      category: string,
      key: string,
      value: string,
      source: string,
      days: number,
      confidence = 80,
    ) => ({
      category,
      key,
      value,
      source,
      confidence,
      tentative: confidence <= 30,
      updatedAt: now - days * DAY,
    });
    this.project = [
      fact("build", "test-runner", "Run desktop checks with `bun run check` in apps/desktop.", "agent", 0.1),
      fact("convention", "commits", "Conventional Commits; no AI attribution lines.", "user", 2),
      fact("environment", "vps-builds", "Leave Rust compilation to CI on the VPS.", "agent", 5, 20),
    ];
    this.global = [fact("preference", "tone", "Short answers, no recap.", "user", 12)];
  }

  answer(method: string, params: JsonObject): Json {
    const cwd = str(params.cwd);
    if (method === "_rustcode/memory/list") {
      return {
        project: cwd
          ? { path: "~/.config/rustcode/project-memory/rustcode-3f2a.json", facts: this.project }
          : null,
        global: { path: "~/.config/rustcode/global-memory.json", facts: this.global },
      };
    }
    const global = params.global === true;
    const before = global ? this.global.length : this.project.length;
    if (params.all === true) this.project = [];
    else {
      const keep = (fact: JsonObject) =>
        !(fact.key === params.key && (!params.category || fact.category === params.category));
      if (global) this.global = this.global.filter(keep);
      else this.project = this.project.filter(keep);
    }
    return { removed: before - (global ? this.global.length : this.project.length) };
  }
}
