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
 * ` 1M`. Known GPT, Gemini and DeepSeek ids use their family casing and a readable version.
 * Unknown ids and catalog names keep their spelling.
 */
export function modelTitle(model: string): string {
  const gpt = /^gpt[- ](\d+(?:\.\d+)?)(?:[- ]([a-z]+(?:[- ][a-z]+)*))?$/i.exec(model);
  if (gpt) {
    const suffix = gpt[2] ? ` ${gpt[2].split(/[- ]+/).map(titleWord).join(" ")}` : "";
    return `GPT ${gpt[1]}${suffix}`;
  }

  const gemini = /^gemini[- ](\d+(?:\.\d+)?)[- ]([a-z]+(?:[- ][a-z]+)*)$/i.exec(model);
  if (gemini?.[2]) {
    const variant = gemini[2]
      .split(/[- ]+/)
      .map(titleWord)
      .join(" ")
      .replace(/\bFlash Lite\b/g, "Flash-Lite");
    return `Gemini ${gemini[1]} ${variant}`;
  }

  const deepseek = /^deepseek[- ](?:v(\d+(?:\.\d+)?)(?:[- ]|$))?([a-z]+(?:[- ][a-z]+)*)?$/i.exec(model);
  if (deepseek && (deepseek[1] || deepseek[2])) {
    const version = deepseek[1] ? ` V${deepseek[1]}` : "";
    const variant = deepseek[2] ? ` ${deepseek[2].split(/[- ]+/).map(titleWord).join(" ")}` : "";
    return `DeepSeek${version}${variant}`;
  }

  const context = /\[(\d+)m\]$/i.exec(model);
  const bare = context ? model.slice(0, context.index) : model;
  const id = MODEL_ALIASES[bare.toLowerCase()] ?? bare;
  const match = /^(?:claude-)?([a-z]+)-(\d+)(?:[-.](\d{1,2}))?(?:-\d{8})?(?:-(thinking))?$/i.exec(id);
  const family = match?.[1]?.toLowerCase();
  if (!match || !family || !FAMILIES.has(family)) return model;
  const version = match[3] ? `${match[2]}.${match[3]}` : match[2];
  const suffix = match[4] ? ` ${titleWord(match[4])}` : "";
  const name = `${titleWord(family)} ${version}${suffix}`;
  return context ? `${name} ${context[1]}M` : name;
}

/**
 * A model id formatted for display using its harness catalog entry when available, else `modelTitle`
 * on the bare id (without any `provider/` prefix). Legacy `"default"` catalog entries are ignored so
 * `"Default (recommended)"` never replaces a concrete model name.
 */
export function catalogModelTitle(
  model: string | undefined,
  harness?: { id: string; models: readonly { id: string; name: string }[] },
): string | undefined {
  if (!model) return undefined;
  const normalized = harness ? (usageModelId(harness.id, model) ?? model) : model;
  const entry = harness?.models.find(
    (item) =>
      item.id !== "default" &&
      (item.id === model ||
        (harness.id === "claude-code" && usageModelId(harness.id, item.id) === normalized)),
  );
  const catalogName = entry?.name?.trim().replace(/\s+/g, " ");
  if (catalogName && catalogName !== entry?.id) return modelTitle(catalogName);
  const bare = normalized.split("/").pop();
  return bare ? modelTitle(bare) : bare;
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
