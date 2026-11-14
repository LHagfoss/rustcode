// Settings › Agents › Voice: which text generator entry transcribes dictation
// (Off by default), the model, a language hint and hold-to-talk versus
// toggle. Kept on this machine (`lib/dictation.ts`); the key is the text
// generator's own, in the engine's credential store.

import { useEffect, useState } from "react";
import {
  DEFAULT_TRANSCRIPTION_MODEL,
  type DictationMode,
  readVoiceSettings,
  TRANSCRIBING_PROVIDERS,
  type VoiceSettings,
  writeVoiceSettings,
} from "../lib/dictation";
import { request } from "../state/store";
import { inputClass, Segments, Select } from "./primitives";
import { Group, Row } from "./settingsKit";

type Generator = { id: string; provider: string; model: string; baseUrl: string | null };

export function VoiceGroup() {
  const [settings, setSettings] = useState<VoiceSettings>(readVoiceSettings);
  const [generators, setGenerators] = useState<Generator[] | null>(null);
  useEffect(() => {
    void request<{ generators?: Generator[] }>("_rustcode/textGeneration/configure", { action: "list" })
      .then((answer) => setGenerators(answer?.generators ?? []))
      .catch(() => setGenerators([]));
  }, []);
  const save = (patch: Partial<VoiceSettings>) => {
    const next = { ...settings, ...patch };
    setSettings(next);
    writeVoiceSettings(next);
  };
  const usable = (generators ?? []).filter((g) =>
    (TRANSCRIBING_PROVIDERS as readonly string[]).includes(g.provider),
  );
  const label = (g: Generator) => (g.provider === "custom" ? `Custom · ${g.baseUrl ?? ""}` : "OpenAI");
  const off = !settings.generator;
  return (
    <Group
      title="Voice"
      footer="Dictation records only while you hold or toggle it, and sends the audio to the provider you choose. Nothing is kept."
    >
      <Row
        title="Transcribe with"
        subtitle={
          usable.length
            ? "An OpenAI or OpenAI-compatible endpoint from Text generation, with its key."
            : "Add an OpenAI or custom OpenAI-compatible generator under Text generation first."
        }
      >
        <Select
          label="Transcribe with"
          value={settings.generator ?? ""}
          disabled={generators === null}
          onChange={(value) => save({ generator: value || null })}
          options={[{ value: "", label: "Off" }, ...usable.map((g) => ({ value: g.id, label: label(g) }))]}
        />
      </Row>
      <Row title="Transcription model" subtitle="The speech-to-text model the endpoint offers.">
        <input
          aria-label="Transcription model"
          className={inputClass}
          disabled={off}
          placeholder={DEFAULT_TRANSCRIPTION_MODEL}
          value={settings.model}
          onChange={(e) => save({ model: e.target.value })}
        />
      </Row>
      <Row title="Language" subtitle="A language code such as en or nb; empty detects it.">
        <input
          aria-label="Dictation language"
          className={inputClass}
          disabled={off}
          placeholder="Detect"
          value={settings.language}
          onChange={(e) => save({ language: e.target.value })}
        />
      </Row>
      <Row title="Microphone button and shortcut" subtitle="Toggle: press to start and again to finish.">
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
    </Group>
  );
}
