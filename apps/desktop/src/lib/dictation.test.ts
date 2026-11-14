import { describe, expect, test } from "bun:test";
import {
  cleanupNote,
  clearDictationTarget,
  type DictationState,
  dictationErrorText,
  dictationStep,
  formatElapsed,
  insertTranscript,
  MAX_AUDIO_BYTES,
  pickMimeType,
  readVoiceOverride,
  recordingProblem,
  resolveVoiceSettings,
  setDictationTarget,
  toggleDictation,
  transcribeParams,
  VOICE_DEFAULTS,
} from "./dictation";

const idle: DictationState = { kind: "idle" };

describe("dictation states", () => {
  test("start, record, finish, transcribe", () => {
    const starting = dictationStep(idle, { type: "start" });
    expect(starting).toEqual({ kind: "starting" });
    const recording = dictationStep(starting, { type: "started", at: 5 });
    expect(recording).toEqual({ kind: "recording", startedAt: 5 });
    const transcribing = dictationStep(recording, { type: "finish" });
    expect(transcribing).toEqual({ kind: "transcribing" });
    expect(dictationStep(transcribing, { type: "transcribed" })).toEqual(idle);
  });

  test("discard while starting or recording goes back to idle; not while transcribing", () => {
    expect(dictationStep({ kind: "starting" }, { type: "discard" })).toEqual(idle);
    expect(dictationStep({ kind: "recording", startedAt: 1 }, { type: "discard" })).toEqual(idle);
    const transcribing: DictationState = { kind: "transcribing" };
    expect(dictationStep(transcribing, { type: "discard" })).toBe(transcribing);
  });

  test("an error keeps its message and a new start leaves it", () => {
    const failed = dictationStep({ kind: "starting" }, { type: "fail", message: "denied" });
    expect(failed).toEqual({ kind: "error", message: "denied" });
    expect(dictationStep(failed, { type: "start" })).toEqual({ kind: "starting" });
    expect(dictationStep(idle, { type: "fail", message: "late" })).toBe(idle);
  });

  test("events that do not apply change nothing", () => {
    expect(dictationStep(idle, { type: "finish" })).toBe(idle);
    const recording: DictationState = { kind: "recording", startedAt: 1 };
    expect(dictationStep(recording, { type: "start" })).toBe(recording);
    expect(dictationStep(idle, { type: "started", at: 3 })).toBe(idle);
  });

  test("elapsed time reads as m:ss", () => {
    expect(formatElapsed(0)).toBe("0:00");
    expect(formatElapsed(7_900)).toBe("0:07");
    expect(formatElapsed(83_000)).toBe("1:23");
  });
});

describe("insertion at the caret", () => {
  test("into an empty draft", () => {
    expect(insertTranscript("", " hello ", 0)).toEqual({ value: "hello", caret: 5 });
  });

  test("between words, with spaces where it touches them", () => {
    expect(insertTranscript("fix bug", "the login", 3)).toEqual({ value: "fix the login bug", caret: 13 });
    expect(insertTranscript("fix ", "it", 4)).toEqual({ value: "fix it", caret: 6 });
  });

  test("replaces a selection", () => {
    expect(insertTranscript("say WORD now", "hello", 4, 8)).toEqual({ value: "say hello now", caret: 9 });
  });

  test("an empty transcript or an out-of-range caret is harmless", () => {
    expect(insertTranscript("draft", "  ", 2)).toEqual({ value: "draft", caret: 2 });
    expect(insertTranscript("draft", "more", 99)).toEqual({ value: "draft more", caret: 10 });
  });
});

describe("the engine request", () => {
  test("is shaped from the settings, and absent while Off", () => {
    expect(transcribeParams(VOICE_DEFAULTS, "QUJD", "audio/webm", 1234.6)).toBeNull();
    expect(
      transcribeParams(
        { generator: "g1", model: " ", language: " nb ", mode: "toggle", cleanup: true },
        "QUJD",
        "audio/mp4",
        1234.6,
        "/work/app",
      ),
    ).toEqual({
      generatorId: "g1",
      model: "gpt-4o-mini-transcribe",
      language: "nb",
      audio: "QUJD",
      mimeType: "audio/mp4",
      durationMs: 1235,
      cleanup: true,
      project: "/work/app",
    });
  });

  test("cleanup that did not happen leaves a short note, and the raw text stays", () => {
    expect(cleanupNote("applied")).toBeNull();
    expect(cleanupNote(undefined)).toBeNull();
    expect(cleanupNote("unavailable")).toContain("as dictated");
    expect(cleanupNote("failed")).toContain("as dictated");
  });

  test("picks the first container the webview records", () => {
    expect(pickMimeType((t) => t === "audio/mp4")).toBe("audio/mp4");
    expect(pickMimeType((t) => t.startsWith("audio/webm"))).toBe("audio/webm;codecs=opus");
    expect(pickMimeType(() => false)).toBeNull();
  });

  test("refuses empty, too long and too large recordings", () => {
    expect(recordingProblem(0, 1000)).toContain("Nothing");
    expect(recordingProblem(10, 10 * 60_000)).toContain("too long");
    expect(recordingProblem(MAX_AUDIO_BYTES + 1, 1000)).toContain("too large");
    expect(recordingProblem(10, 1000)).toBeNull();
  });

  test("errors say what happened", () => {
    expect(dictationErrorText(new Error("NotAllowedError: Permission denied"))).toContain("denied");
    expect(dictationErrorText(new Error("Could not reach the transcription provider"))).toContain("reach");
  });
});

describe("the shortcut", () => {
  test("runs the registered composer, and nothing when none is there", () => {
    let runs = 0;
    const listener = () => {
      runs += 1;
    };
    expect(toggleDictation()).toBe(false);
    setDictationTarget(listener);
    expect(toggleDictation()).toBe(true);
    clearDictationTarget(listener);
    expect(toggleDictation()).toBe(false);
    expect(runs).toBe(1);
  });
});

describe("a project's voice override", () => {
  const machine = { ...VOICE_DEFAULTS, generator: "openai", language: "en", cleanup: false };

  test("cleanup is off by default", () => {
    expect(VOICE_DEFAULTS.cleanup).toBe(false);
  });

  test("a field the project set wins; one it left out falls through to the machine", () => {
    expect(resolveVoiceSettings(machine, null)).toEqual(machine);
    expect(resolveVoiceSettings(machine, {})).toEqual(machine);
    expect(resolveVoiceSettings(machine, { cleanup: true, language: "nb" })).toEqual({
      ...machine,
      cleanup: true,
      language: "nb",
    });
    // A project that sets `false` still wins over a machine that cleans up.
    expect(resolveVoiceSettings({ ...machine, cleanup: true }, { cleanup: false }).cleanup).toBe(false);
    // Off for this project only.
    expect(resolveVoiceSettings(machine, { generator: null }).generator).toBeNull();
    expect(resolveVoiceSettings(machine, { generator: "local" }).generator).toBe("local");
    // The mode stays the app's.
    expect(resolveVoiceSettings({ ...machine, mode: "hold" }, { cleanup: true }).mode).toBe("hold");
  });

  test("reads only the fields a voice override may hold", () => {
    expect(readVoiceOverride(undefined)).toEqual({});
    expect(readVoiceOverride("on")).toEqual({});
    expect(
      readVoiceOverride({ generator: null, model: " ", language: "nb", cleanup: true, mode: "hold" }),
    ).toEqual({ generator: null, language: "nb", cleanup: true });
  });
});
