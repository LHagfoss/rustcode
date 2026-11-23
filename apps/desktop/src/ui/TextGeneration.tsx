// Settings › Harnesses › Text generation: an optional separate model that writes thread titles
// and git drafts (docs/desktop.md › Text generation). One picker chooses it; saved models list
// under it with a remove icon, and the add form stays folded until "Add model" opens it.

import { type FormEvent, useEffect, useState } from "react";
import { opError, opRunning, useOp } from "../lib/op";
import { request } from "../state/store";
import { Trash2 } from "./icons";
import { cx, Tooltip } from "./primitives";
import { Action, fieldClass, Group, Row, Select } from "./settingsKit";

type Generator = { id: string; provider: string; model: string; baseUrl: string | null };
type Config = { generators: Generator[]; selected: string | null };
const EMPTY: Config = { generators: [], selected: null };

const PROVIDERS = [
  { value: "anthropic", label: "Anthropic" },
  { value: "openai", label: "OpenAI" },
  { value: "openrouter", label: "OpenRouter" },
  { value: "custom", label: "OpenAI-compatible" },
];
const providerName = (id: string) => PROVIDERS.find((p) => p.value === id)?.label ?? id;

export function TextGeneration() {
  const [listed, runList] = useOp<Config>();
  const [configured, runConfigure] = useOp<Config>();
  const config = configured.data ?? listed.data ?? EMPTY;
  const [adding, setAdding] = useState(false);
  // Once a change was asked, its outcome replaces the first read's.
  const error =
    (configured.status === "idle" && configured.data === undefined ? opError(listed) : opError(configured)) ??
    "";
  const busy = opRunning(configured);
  const configure = async (
    action: "select" | "remove" | "set",
    id: string | null = null,
    add?: { generator: Generator; apiKey: string },
  ) => {
    const done = await runConfigure(
      async () =>
        (await request("_rustcode/textGeneration/configure", {
          action,
          id,
          generator: add?.generator ?? null,
          apiKey: add?.apiKey ?? null,
        })) as Config,
      { describe: () => "Could not update text generation." },
    );
    return done.status === "done";
  };
  useEffect(() => {
    void runList(
      async () => (await request("_rustcode/textGeneration/configure", { action: "list" })) as Config,
      { describe: () => "Could not load text generation." },
    );
  }, [runList]);

  return (
    <section aria-label="Text generation">
      <Group
        title="Text generation"
        action={
          !adding && (
            <Action quiet size="sm" onClick={() => setAdding(true)}>
              Add model
            </Action>
          )
        }
        footer={error || "Optional. Keys stay on this computer."}
      >
        <Row title="Titles and git drafts" subtitle="Written by the thread's own model unless you pick one.">
          <Select
            label="Model for titles and git drafts"
            value={config.selected ?? ""}
            disabled={busy || config.generators.length === 0}
            onChange={(value) => void configure("select", value || null)}
            options={[
              { value: "", label: "Thread's model" },
              ...config.generators.map((g) => ({ value: g.id, label: g.model })),
            ]}
          />
        </Row>
        {config.generators.map((g) => (
          <Row
            key={g.id}
            title={<span className="font-mono text-callout">{g.model}</span>}
            subtitle={g.baseUrl ? `${providerName(g.provider)} · ${g.baseUrl}` : providerName(g.provider)}
          >
            <Tooltip label="Remove">
              <button
                type="button"
                aria-label={`Remove ${g.model}`}
                disabled={busy}
                onClick={() => void configure("remove", g.id)}
                className="inline-flex size-7 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-danger disabled:opacity-40"
              >
                <Trash2 size={14} />
              </button>
            </Tooltip>
          </Row>
        ))}
        {adding && (
          <AddModelForm
            busy={busy}
            onCancel={() => setAdding(false)}
            onAdd={async (generator, apiKey) => {
              if (await configure("set", null, { generator, apiKey })) setAdding(false);
            }}
          />
        )}
      </Group>
    </section>
  );
}

function AddModelForm({
  busy,
  onAdd,
  onCancel,
}: {
  busy: boolean;
  onAdd: (generator: Generator, apiKey: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [provider, setProvider] = useState("anthropic");
  const [model, setModel] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [key, setKey] = useState("");
  const custom = provider === "custom";
  // A hosted provider needs its key; a local OpenAI-compatible server needs its address instead.
  const ready = !!model.trim() && (custom ? !!baseUrl.trim() : !!key.trim());
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!ready || busy) return;
    void onAdd(
      { id: crypto.randomUUID(), provider, model: model.trim(), baseUrl: custom ? baseUrl.trim() : null },
      key,
    );
  };
  const field = cx(fieldClass, "w-full");
  return (
    <form
      onSubmit={submit}
      className="flex flex-col gap-2 px-4 py-3"
      aria-label="Add a text generation model"
    >
      <div className="flex items-center gap-2">
        <Select label="Provider" value={provider} onChange={setProvider} options={PROVIDERS} />
        <input
          aria-label="Model ID"
          placeholder="Model ID"
          className={cx(field, "flex-1 font-mono")}
          value={model}
          onChange={(e) => setModel(e.target.value)}
          autoFocus
        />
      </div>
      {custom && (
        <input
          aria-label="Base URL"
          placeholder="http://localhost:11434/v1"
          className={cx(field, "font-mono")}
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
        />
      )}
      <input
        aria-label="API key"
        type="password"
        autoComplete="new-password"
        placeholder={custom ? "API key (optional)" : "API key"}
        className={field}
        value={key}
        onChange={(e) => setKey(e.target.value)}
      />
      <div className="flex justify-end gap-2 pt-1">
        <Action size="sm" quiet onClick={onCancel}>
          Cancel
        </Action>
        <Action size="sm" type="submit" prominent disabled={!ready || busy}>
          Add
        </Action>
      </div>
    </form>
  );
}
