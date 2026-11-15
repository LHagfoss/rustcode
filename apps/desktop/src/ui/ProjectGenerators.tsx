// Settings › Agents with one project chosen in the scope bar: the project's own text generator
// and Voice settings, kept as the `text-generator` and `voice` project overrides on this
// machine's engine (docs/desktop.md › Settings a project overrides). Each row shows where its
// value comes from, as the other project rows do, and "Use the machine's value" removes the
// project's field so the machine's Text generation and Voice settings apply again.

import { type ChangeEvent, useEffect, useState } from "react";
import type { Json, JsonObject } from "../acp/jsonrpc";
import { DEFAULT_TRANSCRIPTION_MODEL, readVoiceSettings, TRANSCRIBING_PROVIDERS } from "../lib/dictation";
import { type ProjectOverride, patchProjectOverride } from "../state/settingsRegistry";
import type { ProjectRowState } from "../state/settingsScope";
import { request, toast } from "../state/store";
import { inputClass, Select } from "./primitives";
import { ScopedValue } from "./SettingsScope";
import { Choice, Group, Row } from "./settingsKit";

type Generator = { id: string; provider: string; model: string; baseUrl: string | null };
type Overrides = { "text-generator"?: ProjectOverride; voice?: ProjectOverride };
type Key = keyof Overrides;

const MACHINE = "__machine__";
const OFF = "__off__";

function overridesOf(answer: unknown): Overrides {
  const project = (answer as { project?: { overrides?: Record<string, Json> } } | null)?.project;
  const pick = (value: Json | undefined) =>
    value && typeof value === "object" && !Array.isArray(value) ? (value as ProjectOverride) : undefined;
  return {
    "text-generator": pick(project?.overrides?.["text-generator"]),
    voice: pick(project?.overrides?.voice),
  };
}

export function ProjectGenerators({ project }: { project: { name: string; path: string } }) {
  const [generators, setGenerators] = useState<Generator[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [overrides, setOverrides] = useState<Overrides | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const machineVoice = readVoiceSettings();

  useEffect(() => {
    let live = true;
    setOverrides(null);
    setDrafts({});
    void request<{ generators?: Generator[]; selected?: string | null }>(
      "_rustcode/textGeneration/configure",
      {
        action: "list",
      },
    )
      .then((answer) => {
        if (!live) return;
        setGenerators(answer?.generators ?? []);
        setSelected(answer?.selected ?? null);
      })
      .catch(() => undefined);
    void request("_rustcode/settings/get", { keys: [], project: project.path })
      .then((answer) => live && setOverrides(overridesOf(answer)))
      .catch(() => live && setOverrides({}));
    return () => {
      live = false;
    };
  }, [project.path]);

  const write = async (key: Key, field: string, value: Json | undefined) => {
    const next = patchProjectOverride(overrides?.[key], field, value);
    const params: JsonObject = next
      ? { project: project.path, values: { [key]: next } }
      : { project: project.path, clear: [key] };
    try {
      setOverrides(overridesOf(await request("_rustcode/settings/set", params)));
    } catch {
      toast(`Could not save the setting for ${project.name}.`, "error");
    }
  };

  const own = (key: Key, field: string) => overrides?.[key] !== undefined && field in (overrides[key] ?? {});
  const rowState = (key: Key, field: string): ProjectRowState => ({
    name: project.name,
    readOnly: null,
    source: own(key, field) ? "project" : "machine",
    breakdown: [],
    skipped: [],
    clear: own(key, field) ? () => write(key, field, undefined) : undefined,
  });
  const loading = overrides === null;
  const generator = overrides?.["text-generator"] ?? {};
  const voice = overrides?.voice ?? {};
  const byId = (id: string | null | undefined) => generators.find((g) => g.id === id);
  const label = (g: Generator) => `${g.provider} / ${g.model}`;
  const choice = (value: Json | undefined) =>
    value === undefined ? MACHINE : value === null ? OFF : String(value);
  const fromChoice = (value: string) => (value === MACHINE ? undefined : value === OFF ? null : value);
  const machineGenerator = byId(selected);
  const shownGenerator = "id" in generator ? byId(generator.id as string | null) : machineGenerator;
  const usable = generators.filter((g) => (TRANSCRIBING_PROVIDERS as readonly string[]).includes(g.provider));
  const machineTranscriber = byId(machineVoice.generator);
  const voiceOff = "generator" in voice ? voice.generator === null : !machineVoice.generator;

  const text = (key: Key, field: string, current: unknown) => ({
    value: drafts[`${key}.${field}`] ?? (typeof current === "string" ? current : ""),
    disabled: loading,
    onChange: (e: ChangeEvent<HTMLInputElement>) =>
      setDrafts((d) => ({ ...d, [`${key}.${field}`]: e.target.value })),
    onBlur: () => {
      const draft = drafts[`${key}.${field}`];
      if (draft === undefined) return;
      setDrafts(({ [`${key}.${field}`]: _, ...rest }) => rest);
      if (draft === (typeof current === "string" ? current : "")) return;
      void write(key, field, draft.trim() ? draft.trim() : undefined);
    },
  });

  return (
    <Group
      title={`Text generation and voice in ${project.name}`}
      hint="Fields left on the machine's value follow this machine's Text generation and Voice."
    >
      <Row title="Text generator" subtitle="Writes titles, git drafts and dictation cleanup in this project.">
        <ScopedValue mixed={false} breakdown={[]} project={rowState("text-generator", "id")}>
          <Select
            label="Project text generator"
            value={choice(generator.id)}
            disabled={loading}
            onChange={(value) => void write("text-generator", "id", fromChoice(value))}
            options={[
              {
                value: MACHINE,
                label: machineGenerator ? `Machine's · ${label(machineGenerator)}` : "Machine's · none",
              },
              { value: OFF, label: "Off" },
              ...generators.map((g) => ({ value: g.id, label: label(g) })),
            ]}
          />
        </ScopedValue>
      </Row>
      <Row title="Generator model" subtitle="A model ID for this project; empty uses the generator's own.">
        <ScopedValue mixed={false} breakdown={[]} project={rowState("text-generator", "model")}>
          <input
            aria-label="Project generator model"
            className={inputClass}
            placeholder={shownGenerator?.model ?? "Model ID"}
            {...text("text-generator", "model", generator.model)}
            disabled={loading || !shownGenerator}
          />
        </ScopedValue>
      </Row>
      <Row title="Transcribe with" subtitle="The speech-to-text endpoint dictation uses in this project.">
        <ScopedValue mixed={false} breakdown={[]} project={rowState("voice", "generator")}>
          <Select
            label="Project transcription provider"
            value={choice(voice.generator)}
            disabled={loading}
            onChange={(value) => void write("voice", "generator", fromChoice(value))}
            options={[
              {
                value: MACHINE,
                label: machineTranscriber ? `Machine's · ${label(machineTranscriber)}` : "Machine's · Off",
              },
              { value: OFF, label: "Off" },
              ...usable.map((g) => ({ value: g.id, label: label(g) })),
            ]}
          />
        </ScopedValue>
      </Row>
      <Row title="Transcription model" subtitle="Empty uses the machine's transcription model.">
        <ScopedValue mixed={false} breakdown={[]} project={rowState("voice", "model")}>
          <input
            aria-label="Project transcription model"
            className={inputClass}
            placeholder={machineVoice.model || DEFAULT_TRANSCRIPTION_MODEL}
            {...text("voice", "model", voice.model)}
            disabled={loading || voiceOff}
          />
        </ScopedValue>
      </Row>
      <Row title="Language" subtitle="A language code such as en or nb; empty uses the machine's.">
        <ScopedValue mixed={false} breakdown={[]} project={rowState("voice", "language")}>
          <input
            aria-label="Project dictation language"
            className={inputClass}
            placeholder={machineVoice.language || "Detect"}
            {...text("voice", "language", voice.language)}
            disabled={loading || voiceOff}
          />
        </ScopedValue>
      </Row>
      <Row
        title="Clean up dictated text"
        subtitle="Fix punctuation and drop filler words with the text generator."
      >
        <ScopedValue mixed={false} breakdown={[]} project={rowState("voice", "cleanup")}>
          <Choice
            label="Clean up dictated text in this project"
            value={voice.cleanup === true ? "on" : voice.cleanup === false ? "off" : MACHINE}
            onChange={(value) =>
              void write("voice", "cleanup", value === MACHINE ? undefined : value === "on")
            }
            options={[
              { value: MACHINE, label: machineVoice.cleanup ? "Machine's · On" : "Machine's · Off" },
              { value: "on", label: "On" },
              { value: "off", label: "Off" },
            ]}
          />
        </ScopedValue>
      </Row>
    </Group>
  );
}
