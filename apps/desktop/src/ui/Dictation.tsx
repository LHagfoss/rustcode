// Voice dictation in the composer: the microphone button, the recording state
// with its elapsed time, and the hook that records, transcribes through the
// engine and hands the text to the composer to put at the caret. It never
// sends: the transcript is draft text like any other. The pure rules (states,
// insertion, request shape) are `lib/dictation.ts`.

import { Loader2, Mic, MicOff, Square, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  cleanupNote,
  clearDictationTarget,
  type DictationEvent,
  type DictationState,
  dictationErrorText,
  dictationStep,
  formatElapsed,
  MAX_RECORDING_MS,
  pickMimeType,
  readVoiceOverride,
  readVoiceSettings,
  recordingProblem,
  resolveVoiceSettings,
  setDictationTarget,
  TRANSCRIBE_METHOD,
  transcribeParams,
  type VoiceSettings,
} from "../lib/dictation";
import { checkMicrophone, micBlocked, micBlockedNotice, openMicrophoneSettings } from "../lib/nativeAccess";
import { openSettings, request, shortcutHint, toast } from "../state/store";
import { cx, Pill, Tooltip } from "./primitives";

function toBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ""));
    reader.onerror = () => reject(reader.error ?? new Error("Recording could not be read"));
    reader.readAsDataURL(blob);
  });
}

export interface Dictation {
  state: DictationState;
  elapsed: number;
  /** The button and the shortcut in toggle mode: start, or finish what is recording. */
  toggle: () => void;
  /** Hold-to-talk: start on press, finish on release. */
  press: () => void;
  release: () => void;
  discard: () => void;
  hold: boolean;
  /** The system refuses the microphone to the app (macOS): the composer says how to allow it. */
  blocked: "denied" | "restricted" | null;
  dismissBlocked: () => void;
}

/**
 * This machine's Voice settings as a thread in `project` reads them: the project's `voice`
 * override from this machine's engine over the app's own (docs/desktop.md › Settings a project
 * overrides). Without a project, or when the engine has no answer, the app's own.
 */
async function voiceSettingsFor(project: string | null): Promise<VoiceSettings> {
  const machine = readVoiceSettings();
  if (!project) return machine;
  try {
    const answer = await request<{ project?: { overrides?: Record<string, unknown> } }>(
      "_rustcode/settings/get",
      { keys: [], project },
      5_000,
    );
    return resolveVoiceSettings(machine, readVoiceOverride(answer?.project?.overrides?.voice));
  } catch {
    return machine;
  }
}

/**
 * Records and transcribes; `insert` puts the text at the caret, `active` says this composer
 * answers the shortcut, and `project` (this machine's folder) picks the project's overrides.
 */
export function useDictation(
  insert: (text: string) => void,
  active: boolean,
  project: string | null = null,
): Dictation {
  const [state, setState] = useState<DictationState>({ kind: "idle" });
  const stateRef = useRef(state);
  const [elapsed, setElapsed] = useState(0);
  const recorder = useRef<MediaRecorder | null>(null);
  const stream = useRef<MediaStream | null>(null);
  const chunks = useRef<Blob[]>([]);
  const discarded = useRef(false);
  const insertRef = useRef(insert);
  insertRef.current = insert;
  const [hold, setHold] = useState(() => readVoiceSettings().mode === "hold");
  const [blocked, setBlocked] = useState<"denied" | "restricted" | null>(null);

  const dispatch = useCallback((event: DictationEvent) => {
    const next = dictationStep(stateRef.current, event);
    stateRef.current = next;
    setState(next);
    return next;
  }, []);

  const release = useCallback(() => {
    for (const track of stream.current?.getTracks() ?? []) track.stop();
    stream.current = null;
    recorder.current = null;
    chunks.current = [];
  }, []);

  const fail = useCallback(
    (error: unknown) => {
      release();
      const message = dictationErrorText(error);
      dispatch({ type: "fail", message });
      toast(message, "error");
    },
    [dispatch, release],
  );

  const projectRef = useRef(project);
  projectRef.current = project;

  const start = useCallback(async () => {
    if (dictationStep(stateRef.current, { type: "start" }) === stateRef.current) return;
    dispatch({ type: "start" });
    const settings = await voiceSettingsFor(projectRef.current);
    setHold(settings.mode === "hold");
    if (stateRef.current.kind !== "starting") return;
    if (!settings.generator) {
      // Nothing is recorded until a provider is chosen: the button leads to the choice.
      dispatch({ type: "discard" });
      toast("Choose a transcription provider to dictate.", "info");
      openSettings("agents");
      return;
    }
    try {
      if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === "undefined")
        throw new Error("This window cannot record audio.");
      // macOS gives the web view a silent stream, or none, when the app may not use the
      // microphone: ask the system first, and say how to allow it instead of recording nothing.
      const access = await checkMicrophone();
      if (stateRef.current.kind !== "starting") return;
      if (micBlocked(access)) {
        setBlocked(access);
        dispatch({ type: "discard" });
        return;
      }
      setBlocked(null);
      const mime = pickMimeType((t) => MediaRecorder.isTypeSupported(t));
      const media = await navigator.mediaDevices.getUserMedia({ audio: true });
      if (stateRef.current.kind !== "starting") {
        for (const track of media.getTracks()) track.stop();
        return;
      }
      stream.current = media;
      const rec = mime ? new MediaRecorder(media, { mimeType: mime }) : new MediaRecorder(media);
      chunks.current = [];
      discarded.current = false;
      rec.ondataavailable = (e) => {
        if (e.data.size) chunks.current.push(e.data);
      };
      recorder.current = rec;
      rec.start(1000);
      dispatch({ type: "started", at: Date.now() });
    } catch (error) {
      fail(error);
    }
  }, [dispatch, fail]);

  const finish = useCallback(() => {
    const rec = recorder.current;
    const current = stateRef.current;
    if (!rec || current.kind !== "recording") return;
    const duration = Date.now() - current.startedAt;
    dispatch({ type: "finish" });
    rec.onstop = async () => {
      const blob = new Blob(chunks.current, { type: rec.mimeType || "audio/webm" });
      release();
      if (discarded.current) return;
      const problem = recordingProblem(blob.size, duration);
      if (problem) return fail(problem);
      try {
        const settings = await voiceSettingsFor(projectRef.current);
        const params = transcribeParams(
          settings,
          await toBase64(blob),
          blob.type,
          duration,
          projectRef.current,
        );
        if (!params) throw new Error("Choose a transcription provider in Settings › Agents › Voice.");
        // Cleanup adds a text generator call (45 s at most) after the transcription.
        const answer = await request<{ text?: string | null; cleanup?: string }>(
          TRANSCRIBE_METHOD,
          params,
          params.cleanup ? 125_000 : 75_000,
        );
        if (stateRef.current.kind !== "transcribing") return;
        const text = answer?.text?.trim() ?? "";
        dispatch({ type: "transcribed" });
        if (text) insertRef.current(text);
        else toast("No speech was recognized.", "info");
        const note = text ? cleanupNote(answer?.cleanup) : null;
        if (note) toast(note, "info");
      } catch (error) {
        fail(error);
      }
    };
    rec.stop();
  }, [dispatch, fail, release]);

  const discard = useCallback(() => {
    discarded.current = true;
    const rec = recorder.current;
    if (rec && rec.state !== "inactive") rec.stop();
    release();
    dispatch({ type: "discard" });
  }, [dispatch, release]);

  const toggle = useCallback(() => {
    if (stateRef.current.kind === "recording") finish();
    else if (stateRef.current.kind === "starting") discard();
    else void start();
  }, [discard, finish, start]);

  // Elapsed time, and the hard stop at the limit (the recording so far is transcribed).
  useEffect(() => {
    if (state.kind !== "recording") return;
    const startedAt = state.startedAt;
    setElapsed(0);
    const timer = window.setInterval(() => {
      const ms = Date.now() - startedAt;
      setElapsed(ms);
      if (ms >= MAX_RECORDING_MS) {
        toast("Dictation stops at 5 minutes; transcribing what was recorded.", "info");
        finish();
      }
    }, 250);
    return () => window.clearInterval(timer);
  }, [state, finish]);

  // Esc discards a recording wherever focus is, before the composer's own Escape runs.
  useEffect(() => {
    if (state.kind !== "recording" && state.kind !== "starting") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      discard();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [state.kind, discard]);

  // The shortcut: toggle mode starts and finishes; hold mode starts on the press and
  // finishes when the key is let go.
  useEffect(() => {
    if (!active) return;
    const listener = () => {
      if (readVoiceSettings().mode !== "hold") return toggle();
      if (stateRef.current.kind === "recording" || stateRef.current.kind === "starting") return;
      void start();
      const up = () => {
        window.removeEventListener("keyup", up, true);
        if (stateRef.current.kind === "starting") discard();
        else finish();
      };
      window.addEventListener("keyup", up, true);
    };
    setDictationTarget(listener);
    return () => clearDictationTarget(listener);
  }, [active, toggle, start, finish, discard]);

  // A composer that goes away stops its microphone.
  useEffect(
    () => () => {
      discarded.current = true;
      const rec = recorder.current;
      if (rec && rec.state !== "inactive") rec.stop();
      for (const track of stream.current?.getTracks() ?? []) track.stop();
    },
    [],
  );

  const press = useCallback(() => {
    if (stateRef.current.kind === "idle" || stateRef.current.kind === "error") void start();
  }, [start]);
  const releaseHold = useCallback(() => {
    if (stateRef.current.kind === "starting") discard();
    else finish();
  }, [discard, finish]);

  const dismissBlocked = useCallback(() => setBlocked(null), []);

  return { state, elapsed, toggle, press, release: releaseHold, discard, hold, blocked, dismissBlocked };
}

/** Over the composer's tray: the microphone is refused, and where to allow it. */
export function MicrophoneNotice({ dictation }: { dictation: Dictation }) {
  if (!dictation.blocked) return null;
  const { text, canOpenSettings } = micBlockedNotice(dictation.blocked);
  return (
    <div role="status" className="mx-3 mt-1 flex items-start gap-2 rounded-lg bg-fill-subtle px-3 py-2 rc-in">
      <MicOff size={14} className="mt-0.5 shrink-0 text-warning" aria-hidden />
      <p className="m-0 min-w-0 flex-1 text-callout text-fg-muted leading-normal">{text}</p>
      {canOpenSettings && (
        <Pill
          size="sm"
          onClick={() =>
            void openMicrophoneSettings().catch(() =>
              toast("Open System Settings › Privacy & Security › Microphone.", "info"),
            )
          }
        >
          Open System Settings
        </Pill>
      )}
      <Tooltip label="Dismiss">
        <button
          type="button"
          aria-label="Dismiss the microphone notice"
          onClick={dictation.dismissBlocked}
          className="flex size-6 shrink-0 items-center justify-center rounded-sm text-fg-faint hover:bg-fill hover:text-fg"
        >
          <X size={12} strokeWidth={2.25} />
        </button>
      </Tooltip>
    </div>
  );
}

/** The tray's microphone: idle, recording (with its time and a discard), or transcribing. */
export function DictationButton({ dictation }: { dictation: Dictation }) {
  const { state, elapsed, hold } = dictation;
  const binding = shortcutHint("dictate");
  if (state.kind === "recording" || state.kind === "starting") {
    return (
      <div className="flex items-center gap-1 rc-in" role="status" aria-live="polite">
        <Tooltip label="Discard the recording (Esc)">
          <button
            type="button"
            aria-label="Discard the recording"
            onClick={dictation.discard}
            className="flex size-7 items-center justify-center rounded-sm text-fg-faint hover:bg-fill hover:text-fg"
          >
            <X size={12} strokeWidth={2.25} />
          </button>
        </Tooltip>
        <span className="flex items-center gap-1.5 text-caption text-fg-muted tabular-nums">
          <span className="size-1.5 animate-pulse rounded-full bg-danger" aria-hidden />
          {state.kind === "starting" ? "Starting…" : `Recording ${formatElapsed(elapsed)}`}
        </span>
        <Tooltip
          label={hold ? "Release to transcribe" : `Finish and transcribe${binding ? ` (${binding})` : ""}`}
        >
          <button
            type="button"
            aria-label="Finish dictation"
            onClick={hold ? undefined : dictation.toggle}
            onPointerUp={hold ? dictation.release : undefined}
            className="flex size-7 items-center justify-center rounded-full bg-danger text-accent-fg hover:bg-danger/85"
          >
            <Square size={9} fill="currentColor" strokeWidth={0} />
          </button>
        </Tooltip>
      </div>
    );
  }
  const transcribing = state.kind === "transcribing";
  return (
    <Tooltip
      label={
        transcribing
          ? "Transcribing…"
          : `${hold ? "Hold to dictate" : "Dictate"}${binding ? ` (${binding})` : ""}`
      }
    >
      <button
        type="button"
        aria-label={transcribing ? "Transcribing" : "Dictate"}
        disabled={transcribing}
        onClick={hold ? undefined : dictation.toggle}
        onPointerDown={hold ? dictation.press : undefined}
        onPointerUp={hold ? dictation.release : undefined}
        onKeyDown={(e) => {
          if (hold && (e.key === " " || e.key === "Enter") && !e.repeat) dictation.press();
        }}
        onKeyUp={(e) => {
          if (hold && (e.key === " " || e.key === "Enter")) dictation.release();
        }}
        className={cx(
          "flex size-7 items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg disabled:opacity-60",
        )}
      >
        {transcribing ? <Loader2 size={13} className="animate-spin" /> : <Mic size={14} />}
      </button>
    </Tooltip>
  );
}
