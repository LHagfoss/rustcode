/**
 * The model a short name stands for: Claude Code records its `opus`, `sonnet`, `fable` and `haiku`
 * aliases, and Pi and the built-in harness write some ids their own way. Pricing reads the model
 * these name, and labels show it.
 */
export const MODEL_ALIASES: Record<string, string> = {
  opus: "claude-opus-5-5",
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
 * ` 1M`; any other id unchanged.
 */
export function modelTitle(model: string): string {
  const context = /\[(\d+)m\]$/i.exec(model);
  const bare = context ? model.slice(0, context.index) : model;
  const id = MODEL_ALIASES[bare.toLowerCase()] ?? bare;
  const match = /^(?:claude-)?([a-z]+)-(\d+)(?:[-.](\d{1,2}))?(?:-\d{8})?$/i.exec(id);
  const family = match?.[1]?.toLowerCase();
  if (!match || !family || !FAMILIES.has(family)) return model;
  const version = match[3] ? `${match[2]}.${match[3]}` : match[2];
  const name = `${family[0]?.toUpperCase()}${family.slice(1)} ${version}`;
  return context ? `${name} ${context[1]}M` : name;
}
