import { useEffect, useState } from "react";
import { request } from "../state/store";
import { inputClass, Pill, Select } from "./primitives";

type Generator = { id: string; provider: string; model: string; baseUrl: string | null };
type Config = { generators: Generator[]; selected: string | null };
export function TextGeneration() {
  const [config, setConfig] = useState<Config>({ generators: [], selected: null });
  const [provider, setProvider] = useState("openai");
  const [model, setModel] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [key, setKey] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function configure(action: string, id: string | null = null) {
    setBusy(true);
    setError("");
    try {
      const value = await request("_rustcode/textGeneration/configure", {
        action,
        id,
        generator:
          action === "set"
            ? { id: crypto.randomUUID(), provider, model, baseUrl: provider === "custom" ? baseUrl : null }
            : null,
        apiKey: action === "set" ? key : null,
      });
      setConfig(value as Config);
      if (action === "set") {
        setKey("");
        setModel("");
      }
    } catch {
      setError("Could not update text generators.");
    } finally {
      setBusy(false);
    }
  }
  useEffect(() => {
    void request("_rustcode/textGeneration/configure", { action: "list" })
      .then((value) => setConfig(value as Config))
      .catch(() => setError("Could not load text generators."));
  }, []);
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
