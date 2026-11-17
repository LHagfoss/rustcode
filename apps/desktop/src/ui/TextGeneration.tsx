import { useEffect, useState } from "react";
import { opError, opRunning, useOp } from "../lib/op";
import { request } from "../state/store";
import { inputClass, Pill, Select } from "./primitives";

type Generator = { id: string; provider: string; model: string; baseUrl: string | null };
type Config = { generators: Generator[]; selected: string | null };
const EMPTY: Config = { generators: [], selected: null };
export function TextGeneration() {
  const [listed, runList] = useOp<Config>();
  const [configured, runConfigure] = useOp<Config>();
  const config = configured.data ?? listed.data ?? EMPTY;
  const [provider, setProvider] = useState("openai");
  const [model, setModel] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [key, setKey] = useState("");
  // Once a change was asked, its outcome replaces the first read's.
  const error =
    (configured.status === "idle" && configured.data === undefined ? opError(listed) : opError(configured)) ??
    "";
  const busy = opRunning(configured);
  async function configure(action: string, id: string | null = null) {
    const done = await runConfigure(
      async () =>
        (await request("_rustcode/textGeneration/configure", {
          action,
          id,
          generator:
            action === "set"
              ? { id: crypto.randomUUID(), provider, model, baseUrl: provider === "custom" ? baseUrl : null }
              : null,
          apiKey: action === "set" ? key : null,
        })) as Config,
      { describe: () => "Could not update text generators." },
    );
    if (done.status === "done" && action === "set") {
      setKey("");
      setModel("");
    }
  }
  useEffect(() => {
    void runList(
      async () => (await request("_rustcode/textGeneration/configure", { action: "list" })) as Config,
      {
        describe: () => "Could not load text generators.",
      },
    );
  }, [runList]);
  return (
    <section className="flex flex-col gap-3" aria-label="Text generation">
      <h3 className="font-semibold">Text generation</h3>
      <p className="text-callout text-fg-muted">
        Use a separate model for titles and git drafts. Keys stay on this computer.
      </p>
      <Select
        label="Selected text generator"
        value={config.selected ?? ""}
        disabled={busy}
        onChange={(value) => void configure("select", value || null)}
        options={[
          { value: "", label: "Use existing drafts and thread model" },
          ...config.generators.map((g) => ({ value: g.id, label: `${g.provider} / ${g.model}` })),
        ]}
      />
      {config.generators.map((g) => (
        <div key={g.id} className="flex items-center gap-2">
          <span className="flex-1">
            {g.provider} / {g.model}
          </span>
          <Pill disabled={busy} onClick={() => void configure("remove", g.id)}>
            Remove
          </Pill>
        </div>
      ))}
      <Select
        label="Generator provider"
        value={provider}
        onChange={setProvider}
        options={["openai", "anthropic", "openrouter", "custom"].map((value) => ({ value, label: value }))}
      />
      <input
        aria-label="Generator model"
        placeholder="Model ID"
        className={inputClass}
        value={model}
        onChange={(e) => setModel(e.target.value)}
      />
      {provider === "custom" && (
        <input
          aria-label="Generator base URL"
          placeholder="http://localhost:11434/v1"
          className={inputClass}
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
        />
      )}
      <input
        aria-label="Generator API key"
        type="password"
        autoComplete="new-password"
        className={inputClass}
        placeholder="API key (write-only)"
        value={key}
        onChange={(e) => setKey(e.target.value)}
      />
      <Pill
        disabled={busy || !model.trim() || (provider !== "custom" && !key.trim())}
        onClick={() => void configure("set")}
      >
        Add generator
      </Pill>
      {error && (
        <p role="status" className="text-caption text-fg-muted">
          {error}
        </p>
      )}
    </section>
  );
}
