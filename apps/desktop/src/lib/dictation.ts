// Voice dictation, the pure parts: the Voice settings this machine keeps, the
// recorder's state machine, where a transcript lands in the draft and the
// engine request it becomes. No React and no DOM, so each is testable on its
// own; `ui/Dictation.tsx` drives the microphone and the composer.
//
// Nothing is recorded or sent until a transcription provider is chosen: the
// provider is a text generator entry (Settings › Agents › Text generation) on
// an OpenAI-compatible endpoint, so its key stays in the engine's credential
// store and is never read here.

import { readStorage, writeStorage } from "./storage";

export type DictationMode = "toggle" | "hold";

export interface VoiceSettings {
  /** The text generator entry that transcribes; null is Off. */
  generator: string | null;
  model: string;
  /** ISO-639-1 hint ("en", "nb"); "" lets the provider detect it. */
  language: string;
  mode: DictationMode;
}

export const DEFAULT_TRANSCRIPTION_MODEL = "gpt-4o-mini-transcribe";
/** The app stops a recording here; the engine refuses anything longer. */
export const MAX_RECORDING_MS = 5 * 60 * 1000;
/** Encoded audio above this is refused before it is sent (the engine's own limit). */
export const MAX_AUDIO_BYTES = 10 * 1024 * 1024;
/** Text generator providers with an OpenAI-compatible `/audio/transcriptions` endpoint. */
export const TRANSCRIBING_PROVIDERS = ["openai", "custom"] as const;

const KEY = "rustcode.voice";
export const VOICE_DEFAULTS: VoiceSettings = {
  generator: null,
  model: DEFAULT_TRANSCRIPTION_MODEL,
  language: "",
  mode: "toggle",
};

export function readVoiceSettings(): VoiceSettings {
  try {
    const raw = JSON.parse(readStorage(KEY) ?? "{}") as Partial<VoiceSettings>;
    return {
      generator: typeof raw.generator === "string" && raw.generator ? raw.generator : null,
      model: typeof raw.model === "string" && raw.model.trim() ? raw.model : DEFAULT_TRANSCRIPTION_MODEL,
      language: typeof raw.language === "string" ? raw.language : "",
      mode: raw.mode === "hold" ? "hold" : "toggle",
    };
  } catch {
    return { ...VOICE_DEFAULTS };
  }
}

export function writeVoiceSettings(next: VoiceSettings) {
  writeStorage(KEY, JSON.stringify(next));
}

// ---------------------------------------------------------------- the recorder's states

export type DictationState =
  | { kind: "idle" }
  | { kind: "starting" }
  | { kind: "recording"; startedAt: number }
  | { kind: "transcribing" }
  | { kind: "error"; message: string };

export type DictationEvent =
  | { type: "start" }
  | { type: "started"; at: number }
  | { type: "finish" }
  | { type: "discard" }
  | { type: "transcribed" }
  | { type: "fail"; message: string };

/** The next state; an event that does not apply leaves the state as it is. */
export function dictationStep(state: DictationState, event: DictationEvent): DictationState {
  switch (event.type) {
    case "start":
      return state.kind === "idle" || state.kind === "error" ? { kind: "starting" } : state;
    case "started":
      return state.kind === "starting" ? { kind: "recording", startedAt: event.at } : state;
    case "finish":
      return state.kind === "recording" ? { kind: "transcribing" } : state;
    case "discard":
      return state.kind === "starting" || state.kind === "recording" || state.kind === "error"
        ? { kind: "idle" }
        : state;
    case "transcribed":
      return state.kind === "transcribing" ? { kind: "idle" } : state;
    case "fail":
      return state.kind === "idle" ? state : { kind: "error", message: event.message };
  }
}

/** "0:07", "1:23": a recording's elapsed time. */
export function formatElapsed(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

// ---------------------------------------------------------------- the transcript in the draft

/**
 * The draft with `text` put in at the selection (replacing it), and where the
 * caret goes after it. A space is added on a side that touches a word, so a
 * transcript never glues itself to the text around it.
 */
export function insertTranscript(
  draft: string,
  text: string,
  start: number,
  end: number = start,
): { value: string; caret: number } {
  const clean = text.trim();
  const from = Math.max(0, Math.min(start, draft.length));
  const to = Math.max(from, Math.min(end, draft.length));
  if (!clean) return { value: draft, caret: to };
  const before = draft.slice(0, from);
  const after = draft.slice(to);
  const lead = before && !/\s$/.test(before) ? " " : "";
  const trail = after && !/^\s/.test(after) ? " " : "";
  const inserted = `${lead}${clean}${trail}`;
  return { value: before + inserted + after, caret: before.length + lead.length + clean.length };
}

// ---------------------------------------------------------------- the engine request

export const TRANSCRIBE_METHOD = "_rustcode/voice/transcribe";

/** The container this webview can record in: WebView2 gives WebM, WebKit MP4, WebKitGTK Ogg. */
export function pickMimeType(supported: (type: string) => boolean): string | null {
  const candidates = [
    "audio/webm;codecs=opus",
    "audio/webm",
    "audio/mp4",
    "audio/ogg;codecs=opus",
    "audio/ogg",
  ];
  return candidates.find(supported) ?? null;
}

/** Why a recording cannot be sent, or null when it can. */
export function recordingProblem(bytes: number, durationMs: number): string | null {
  if (bytes === 0) return "Nothing was recorded.";
  if (durationMs > MAX_RECORDING_MS + 2000)
    return "The recording is too long to transcribe (5 minutes at most).";
  if (bytes > MAX_AUDIO_BYTES) return "The recording is too large to transcribe.";
  return null;
}

export function transcribeParams(
  settings: VoiceSettings,
  audioBase64: string,
  mimeType: string,
  durationMs: number,
): {
  generatorId: string;
  model: string;
  language: string | null;
  audio: string;
  mimeType: string;
  durationMs: number;
} | null {
  if (!settings.generator) return null;
  return {
    generatorId: settings.generator,
    model: settings.model.trim() || DEFAULT_TRANSCRIPTION_MODEL,
    language: settings.language.trim() || null,
    audio: audioBase64,
    mimeType,
    durationMs: Math.round(durationMs),
  };
}

/** What a failure says, without the engine's wrapping. */
export function dictationErrorText(error: unknown): string {
  const raw = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  if (/NotAllowedError|Permission|denied/i.test(raw))
    return "Microphone access was denied. Allow RustCode in System Settings › Privacy › Microphone.";
  if (/NotFoundError|no microphone|Requested device not found/i.test(raw)) return "No microphone was found.";
  if (/reach|network|fetch/i.test(raw))
    return "Could not reach the transcription provider. Your draft is kept.";
  return raw ? `Dictation failed: ${raw}` : "Dictation failed. Your draft is kept.";
}

// ---------------------------------------------------------------- the shortcut

type Listener = () => void;
let target: Listener | null = null;

/** The composer that answers the dictation shortcut: the last one focused or mounted. */
export function setDictationTarget(listener: Listener | null) {
  target = listener;
}
export function clearDictationTarget(listener: Listener) {
  if (target === listener) target = null;
}
/** Runs the shortcut; false when no composer is there to dictate into. */
export function toggleDictation(): boolean {
  if (!target) return false;
  target();
  return true;
}
