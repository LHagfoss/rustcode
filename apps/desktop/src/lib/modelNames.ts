export const CLAUDE_OPUS_MODEL = "claude-opus-5-5";

/**
 * The model a short name stands for: Claude Code records its `opus`, `sonnet`, `fable` and `haiku`
 * aliases, and Pi and the built-in harness write some ids their own way. Pricing reads the model
 * these name, and labels show it.
 */
export const MODEL_ALIASES: Record<string, string> = {
  opus: CLAUDE_OPUS_MODEL,
  sonnet: "claude-sonnet-5",
  fable: "claude-fable-5-2",
  haiku: "claude-haiku-5-5",
  "claude-sonnet-5.5": "claude-sonnet-5-5",
  "deepseek/flash": "deepseek/deepseek-v4.1-flash",
  "chatgpt/gpt-6-luna": "gpt-6-luna",
};

const FAMILIES = new Set(["opus", "sonnet", "haiku", "fable"]);

/**
 * A model as people read it: a Claude id or alias as `Opus 5.5` (`claude-opus-5-5`, `opus`,
 * `claude-sonnet-5.5`, `claude-opus-4-8-20260101`), with a `[1m]` context suffix kept as
 * ` 1M`. Known GPT and Gemini ids use their family casing and a readable version.
 * Unknown ids and catalog names keep their spelling.
 */
export function modelTitle(model: string): string {
  const gpt = /^gpt[- ](\d+(?:\.\d+)?)(?:[- ](astra|luna|sol|terra))?$/i.exec(model);
  if (gpt) return `GPT ${gpt[1]}${gpt[2] ? ` ${titleWord(gpt[2])}` : ""}`;

  const gemini = /^gemini[- ](\d+(?:\.\d+)?)[- ](neon|flash(?:-lite)?|pro)$/i.exec(model);
  if (gemini?.[2]) {
    const variant = gemini[2].toLowerCase() === "flash-lite" ? "Flash-Lite" : titleWord(gemini[2]);
    return `Gemini ${gemini[1]} ${variant}`;
  }

  const context = /\[(\d+)m\]$/i.exec(model);
  const bare = context ? model.slice(0, context.index) : model;
  const id = MODEL_ALIASES[bare.toLowerCase()] ?? bare;
  const match = /^(?:claude-)?([a-z]+)-(\d+)(?:[-.](\d{1,2}))?(?:-\d{8})?$/i.exec(id);
  const family = match?.[1]?.toLowerCase();
  if (!match || !family || !FAMILIES.has(family)) return model;
  const version = match[3] ? `${match[2]}.${match[3]}` : match[2];
  const name = `${titleWord(family)} ${version}`;
  return context ? `${name} ${context[1]}M` : name;
}

function titleWord(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1).toLowerCase();
}

/** Model families with a bundled brand mark in the picker. */
export function modelIconHarness(model: string): string | undefined {
  const id = model.split("/").pop()?.toLowerCase() ?? "";
  if (/^gpt[- ]/.test(id)) return "codex";
  if (/^gemini[- ]/.test(id)) return "gemini";
  if (id.startsWith("claude-") || FAMILIES.has(id)) return "claude-code";
  return undefined;
}

/** The model a Claude Code usage row recorded, including its older shorthand. */
export function usageModelId(harness: string, model: string | undefined): string | undefined {
  if (harness !== "claude-code" || !model) return model;
  const id = model.toLowerCase();
  return MODEL_ALIASES[id === "default" ? "opus" : id] ?? model;
}
