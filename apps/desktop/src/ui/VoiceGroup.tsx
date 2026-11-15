// Settings › Voice & computer use › Voice: a switch in the title turns dictation on (off by
// default), then which text generator entry transcribes it, the model, a language hint, hold-to-talk
// versus toggle, and whether the text generator cleans up a transcript. Kept on this machine
// (`lib/dictation.ts`); the key is the text generator's own, in the engine's credential store.

import { useEffect, useState } from "react";
import {
  DEFAULT_TRANSCRIPTION_MODEL,
  type DictationMode,
  transcribingGenerators,
  type VoiceSettings,
  writeVoiceSettings,
} from "../lib/dictation";
import { openSettings, request } from "../state/store";
import { useVoiceSettings } from "./Dictation";
import { inputClass, Segments, Select, Switch } from "./primitives";
import { Action, Group, Row } from "./settingsKit";

type Generator = { id: string; provider: string; model: string; baseUrl: string | null };

export function VoiceGroup() {
  const settings = useVoiceSettings();
  const [generators, setGenerators] = useState<Generator[] | null>(null);
  // On with no provider to pick yet: the rows say what to add first.
  const [wanted, setWanted] = useState(false);
  useEffect(() => {
    void request<{ generators?: Generator[] }>("_rustcode/textGeneration/configure", { action: "list" })
      .then((answer) => setGenerators(answer?.generators ?? []))
      .catch(() => setGenerators([]));
  }, []);
  const save = (patch: Partial<VoiceSettings>) => writeVoiceSettings({ ...settings, ...patch });
  const usable = transcribingGenerators(generators ?? []);
  const first = usable[0]?.id ?? null;
  // Turned on before the list arrived: take its first usable generator once it does.
  // biome-ignore lint/correctness/useExhaustiveDependencies: only when the list or the wish changes
  useEffect(() => {
    if (wanted && !settings.generator && first) {
      save({ generator: first });
      setWanted(false);
    }
  }, [wanted, first]);
  const label = (g: Generator) => (g.provider === "custom" ? `Custom · ${g.baseUrl ?? ""}` : "OpenAI");
  const on = Boolean(settings.generator) || wanted;
  const options = usable.map((g) => ({ value: g.id, label: label(g) }));
  if (settings.generator && generators && !usable.some((g) => g.id === settings.generator))
    options.unshift({ value: settings.generator, label: "A removed generator" });
  return (
    <Group
      title="Voice"
      switchLabel="Dictation"
      hint="Dictate into the composer with the microphone button or its shortcut."
      enabled={on}
      onEnabledChange={(next) => {
        if (!next) {
          setWanted(false);
          save({ generator: null });
        } else if (first) save({ generator: first });
        else setWanted(true);
      }}
      footer="Dictation records only while you hold or toggle it, and sends the audio to the provider you choose. Nothing is kept."
    >
      {!settings.generator ? (
        <Row
          title={generators === null ? "Looking for a transcription provider…" : "No transcription provider"}
          subtitle="Add an OpenAI or custom OpenAI-compatible generator under Text generation first."
        >
          <Action onClick={() => openSettings("agents")}>Text generation…</Action>
        </Row>
      ) : (
        <>
          <Row
            title="Transcribe with"
            subtitle="A generator from Text generation."
            hint="An OpenAI or OpenAI-compatible endpoint, with its key."
          >
            <Select
              label="Transcribe with"
              value={settings.generator}
              disabled={generators === null}
              onChange={(value) => value && save({ generator: value })}
              options={options}
            />
          </Row>
          <Row title="Transcription model" subtitle="The speech-to-text model the endpoint offers.">
            <input
              aria-label="Transcription model"
              className={inputClass}
              placeholder={DEFAULT_TRANSCRIPTION_MODEL}
              value={settings.model}
              onChange={(e) => save({ model: e.target.value })}
            />
          </Row>
          <Row title="Language" subtitle="A code such as en or nb; empty detects it.">
            <input
              aria-label="Dictation language"
              className={inputClass}
              placeholder="Detect"
              value={settings.language}
              onChange={(e) => save({ language: e.target.value })}
            />
          </Row>
          <Row
            title="Clean up dictated text"
            subtitle="Fix punctuation and drop filler words."
            hint="The text generator does it before the text is inserted: the one selected under Text generation, or the project's. Without one, or if it fails, the text goes in as dictated."
          >
            <Switch
              label="Clean up dictated text"
              checked={settings.cleanup}
              onChange={(cleanup) => save({ cleanup })}
            />
          </Row>
          <Row
            title="Microphone button and shortcut"
            hint="Toggle: press to start and again to finish. Hold to talk: record while it is held."
          >
            <Segments<DictationMode>
              label="Dictation mode"
              value={settings.mode}
              onChange={(mode) => save({ mode })}
              options={[
                { value: "toggle", label: "Toggle" },
                { value: "hold", label: "Hold to talk" },
              ]}
            />
          </Row>
        </>
      )}
    </Group>
  );
}
