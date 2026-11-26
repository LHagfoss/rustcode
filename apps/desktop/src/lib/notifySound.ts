import type { NotifyKind } from "../state/notify";

/** Built-in Web Audio notification tones, kept small and unobtrusive. */
export type NotifySoundTone = "chime" | "glass" | "soft" | "double";

/** Whether the audio cue signals a thread needing input or a completed turn. */
export type NotifySoundCue = "attention" | "finished";

export interface NotifySoundOption {
  value: NotifySoundTone;
  label: string;
}

export const NOTIFY_SOUND_TONES: readonly NotifySoundOption[] = [
  { value: "chime", label: "Chime" },
  { value: "glass", label: "Glass" },
  { value: "soft", label: "Soft" },
  { value: "double", label: "Double" },
];

export const DEFAULT_NOTIFY_SOUND_ENABLED = false;
export const DEFAULT_NOTIFY_SOUND_TONE: NotifySoundTone = "chime";
export const DEFAULT_NOTIFY_SOUND_VOLUME = 45;

/** One synthesized partial scheduled on a Web Audio context. */
export interface ToneNote {
  freq: number;
  start: number;
  duration: number;
  type: OscillatorType;
  gain: number;
}

const VALID_TONES = new Set<string>(NOTIFY_SOUND_TONES.map((option) => option.value));

/** Parses a stored tone id, falling back to the default tone. */
export function parseNotifySoundTone(raw: string | null | undefined): NotifySoundTone {
  return raw && VALID_TONES.has(raw) ? (raw as NotifySoundTone) : DEFAULT_NOTIFY_SOUND_TONE;
}

/** Clamps a volume percentage to an integer between 0 and 100. */
export function clampNotifySoundVolume(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_NOTIFY_SOUND_VOLUME;
  return Math.max(0, Math.min(100, Math.round(value)));
}

/** Parses a stored volume percentage, falling back to the default when absent or invalid. */
export function parseNotifySoundVolume(raw: string | null | undefined): number {
  if (raw === null || raw === undefined || raw.trim() === "") return DEFAULT_NOTIFY_SOUND_VOLUME;
  const parsed = Number(raw);
  return Number.isFinite(parsed) ? clampNotifySoundVolume(parsed) : DEFAULT_NOTIFY_SOUND_VOLUME;
}

/** Maps a thread notification event kind to either the attention or finished cue. */
export function cueForNotifyKind(kind: NotifyKind): NotifySoundCue {
  return kind === "finished" ? "finished" : "attention";
}

/** Pure note schedule for each tone and cue kind: rising pair for attention, calm cadence for finished. */
export function toneNotes(tone: NotifySoundTone, cue: NotifySoundCue): readonly ToneNote[] {
  const attention = cue === "attention";
  switch (tone) {
    case "chime":
      return attention
        ? [
            { freq: 587.33, start: 0, duration: 0.16, type: "sine", gain: 0.8 },
            { freq: 880, start: 0.09, duration: 0.24, type: "sine", gain: 1 },
          ]
        : [
            { freq: 659.25, start: 0, duration: 0.16, type: "sine", gain: 0.75 },
            { freq: 523.25, start: 0.09, duration: 0.24, type: "sine", gain: 0.9 },
          ];
    case "glass":
      return attention
        ? [
            { freq: 1046.5, start: 0, duration: 0.18, type: "sine", gain: 0.75 },
            { freq: 1318.51, start: 0.08, duration: 0.26, type: "sine", gain: 0.85 },
          ]
        : [
            { freq: 1174.66, start: 0, duration: 0.22, type: "sine", gain: 0.75 },
            { freq: 880, start: 0.08, duration: 0.26, type: "sine", gain: 0.7 },
          ];
    case "soft":
      return attention
        ? [
            { freq: 392, start: 0, duration: 0.18, type: "triangle", gain: 0.85 },
            { freq: 493.88, start: 0.1, duration: 0.24, type: "sine", gain: 0.8 },
          ]
        : [{ freq: 440, start: 0, duration: 0.24, type: "sine", gain: 0.85 }];
    case "double":
      return attention
        ? [
            { freq: 698.46, start: 0, duration: 0.09, type: "triangle", gain: 0.9 },
            { freq: 880, start: 0.12, duration: 0.12, type: "triangle", gain: 0.95 },
          ]
        : [
            { freq: 523.25, start: 0, duration: 0.09, type: "triangle", gain: 0.85 },
            { freq: 523.25, start: 0.11, duration: 0.11, type: "sine", gain: 0.75 },
          ];
  }
}

type AudioContextCtor = typeof AudioContext;

let sharedContext: AudioContext | null = null;

function audioContextConstructor(): AudioContextCtor | null {
  if (typeof window === "undefined") return null;
  const host = window as Window & {
    AudioContext?: AudioContextCtor;
    webkitAudioContext?: AudioContextCtor;
  };
  return host.AudioContext ?? host.webkitAudioContext ?? null;
}

function getAudioContext(): AudioContext | null {
  if (sharedContext && sharedContext.state !== "closed") return sharedContext;
  const Ctor = audioContextConstructor();
  if (!Ctor) return null;
  try {
    sharedContext = new Ctor();
    return sharedContext;
  } catch {
    return null;
  }
}

/** Synthesizes and plays a quiet Web Audio notification cue; returns false when muted or unavailable. */
export async function playNotificationSound(
  tone: NotifySoundTone,
  volume: number,
  cue: NotifySoundCue = "attention",
): Promise<boolean> {
  const level = clampNotifySoundVolume(volume);
  if (level <= 0) return false;
  const ctx = getAudioContext();
  if (!ctx) return false;
  try {
    if (ctx.state === "suspended") await ctx.resume();
    const now = ctx.currentTime + 0.005;
    const masterGain = (level / 100) * 0.22;
    for (const note of toneNotes(tone, cue)) {
      const osc = ctx.createOscillator();
      const env = ctx.createGain();
      const peak = Math.max(0.0002, masterGain * note.gain);
      const start = now + note.start;
      const attack = Math.min(0.012, note.duration * 0.25);
      const end = start + note.duration;
      osc.type = note.type;
      osc.frequency.setValueAtTime(note.freq, start);
      env.gain.setValueAtTime(0.0001, start);
      env.gain.exponentialRampToValueAtTime(peak, start + attack);
      env.gain.exponentialRampToValueAtTime(0.0001, end);
      osc.connect(env);
      env.connect(ctx.destination);
      osc.onended = () => {
        osc.disconnect();
        env.disconnect();
      };
      osc.start(start);
      osc.stop(end + 0.01);
    }
    return true;
  } catch {
    return false;
  }
}
