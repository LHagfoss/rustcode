/**
 * The mark beside a model's name: its maker's, drawn by `HarnessIcon` (Claude models the Claude
 * mark, GPT and o-series models OpenAI's, Gemini models Gemini's), a letter in a square for any
 * other model, and a sparkle for a harness's default model.
 */

import { HarnessIcon } from "./HarnessIcon";
import { Sparkles } from "./icons";
import { cx } from "./primitives";

/** The harness id whose mark a model's maker shares, or the model itself for a letter. */
export function modelMaker(name: string): string {
  const key = name.toLowerCase();
  if (/\b(claude|opus|sonnet|haiku|fable)\b/.test(key)) return "claude";
  if (/\b(gpt|o\d|codex|chatgpt|openai)\b/.test(key)) return "codex";
  if (key.includes("gemini")) return "gemini";
  return key;
}

export function ModelIcon({
  name,
  size = 14,
  className,
}: {
  name: string;
  size?: number;
  className?: string;
}) {
  if (name === "Default model")
    return <Sparkles size={size} aria-hidden className={cx("shrink-0 text-fg-muted", className)} />;
  return <HarnessIcon id={modelMaker(name)} name={name} size={size} className={className} />;
}
