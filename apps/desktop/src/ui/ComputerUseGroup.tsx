import { useState } from "react";
import { obj } from "../acp/jsonrpc";
import { errorText, request, toast } from "../state/store";
import { addDomain } from "./computerUse";
import { Switch } from "./primitives";
import { Action, Group, Loading, Row, useEngineData } from "./settingsKit";

export function ComputerUseGroup() {
  const { data, error, reload, setData } = useEngineData(
    "_rustcode/settings/get",
    (answer) => obj(obj(answer)?.values)?.["computer-use"] ?? null,
    { keys: ["computer-use"] },
  );
  const [input, setInput] = useState("");
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const settings = obj(data);
  const entries = Array.isArray(settings?.allowed_domains)
    ? settings.allowed_domains.filter((v): v is string => typeof v === "string")
    : [];
  const actions = settings?.grant_covers_actions === true;
  async function save(enabled: boolean, domains: string[], coversActions = actions) {
    setPending(true);
    setFailure(null);
    try {
      const answer = await request("_rustcode/settings/set", {
        values: {
          "computer-use": { enabled, allowed_domains: domains, grant_covers_actions: coversActions },
        },
      });
      setData(obj(obj(answer)?.values)?.["computer-use"] ?? null);
    } catch (e) {
      // Off, the group is one row with no room for the error: say it in a toast.
      if (settings?.enabled === true) setFailure(e instanceof Error ? e.message : String(e));
      else toast(errorText(e), "error");
    } finally {
      setPending(false);
    }
  }
  const current = settings?.enabled === true;
  if (!settings)
    return (
      <Group title="Computer use">
        <Loading error={error} onRetry={() => void reload()} />
      </Group>
    );
  return (
    <Group
      title="Computer use"
      switchLabel="Browser tools"
      hint="Let every harness use an isolated headless browser. Off by default."
      enabled={current}
      onEnabledChange={(value) => {
        if (!pending) void save(value, entries);
      }}
      footer="Every action asks for approval. Allow a site for a thread to let its reads and screenshots run without asking; the thread's Browser chip revokes it. Start a new thread after enabling."
    >
      {settings.browser_available !== true && (
        <Row
          title="Chrome/Chromium not found"
          subtitle="Install Google Chrome or Chromium on this computer."
          hint="For a custom location, set computer_use.executable in config.toml."
        />
      )}
      <Row
        title="Allowed domains"
        subtitle="Exact hosts only; an empty list blocks all sites."
        hint="*.example.org allows subdomains but not example.org."
        stacked
      >
        <div className="flex w-full flex-col gap-2">
          {entries.map((entry) => (
            <div key={entry} className="flex items-center justify-between gap-2">
              <span className="break-all text-body text-fg">{entry}</span>
              <Action
                disabled={pending}
                onClick={() =>
                  void save(
                    current,
                    entries.filter((host) => host !== entry),
                  )
                }
              >
                Remove <span className="sr-only">{entry}</span>
              </Action>
            </div>
          ))}
          <form
            className="flex gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              const next = addDomain(entries, input);
              if (!next) {
                setFailure(
                  "Enter a unique host such as example.com or *.example.org, without a URL or port.",
                );
                return;
              }
              void save(current, next);
              setInput("");
            }}
          >
            <input
              aria-label="Allowed domain"
              className="min-w-0 flex-1 rounded-lg bg-fill px-3 py-2 text-body text-fg"
              placeholder="example.com or *.example.org"
              value={input}
              disabled={pending}
              onChange={(event) => setInput(event.target.value)}
            />
            <Action type="submit" disabled={pending || !input.trim()}>
              Add
            </Action>
          </form>
        </div>
      </Row>
      <Row
        title="Also allow clicks and typing on granted sites"
        subtitle="Off: a site allowed for a thread still asks before each click and each typed text."
      >
        <Switch
          label="Also allow clicks and typing on granted sites"
          checked={actions}
          disabled={pending}
          onChange={(on) => void save(current, entries, on)}
        />
      </Row>
      {failure && (
        <div role="alert" className="px-4 py-3 text-danger text-caption">
          {failure}
        </div>
      )}
    </Group>
  );
}
