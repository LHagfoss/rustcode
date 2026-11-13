// Settings › Environments › Thread copies: whether this computer keeps its own copy of the threads
// it reads from other environments, and which projects it leaves out (docs/data-store.md › Thread
// mirror, `state/machineMirror.ts`). The copy is read-only and on this computer's disk; the
// environment that runs a thread stays its owner.

import { useEffect, useState } from "react";
import { type MirrorStatus, readMirrorStatus, setMirrorSettings } from "../state/machineMirror";
import { Switch } from "./primitives";
import { fieldClass, Group, Row } from "./settingsKit";

/** What the page says the copy holds: "Holding 12 threads from 2 environments." */
export function mirrorLine(status: MirrorStatus | null): string {
  if (!status) return "Not available: this computer's server did not answer.";
  const hosts = Object.values(status.hosts);
  const threads = hosts.reduce((sum, host) => sum + host.threads, 0);
  if (!threads) return status.enabled ? "Nothing copied yet." : "Off. Nothing is copied.";
  const from = hosts.length === 1 ? "1 environment" : `${hosts.length} environments`;
  const held = `${threads === 1 ? "1 thread" : `${threads} threads`} from ${from}`;
  return status.enabled ? `Holding ${held}.` : `Off. Still holding ${held}; they stay readable.`;
}

/** One folder per line, blanks dropped, as the engine stores them. */
export function parseProjects(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

export function ThreadMirrorGroup() {
  const [status, setStatus] = useState<MirrorStatus | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [projects, setProjects] = useState("");

  useEffect(() => {
    let live = true;
    void readMirrorStatus().then((next) => {
      if (!live) return;
      setStatus(next);
      setProjects(next?.excludedProjects.join("\n") ?? "");
      setLoaded(true);
    });
    return () => {
      live = false;
    };
  }, []);

  const apply = (change: { enabled?: boolean; excludedProjects?: string[] }) =>
    void setMirrorSettings(change).then((next) => {
      if (next) setStatus(next);
    });

  return (
    <Group
      title="Thread copies"
      footer="The environment that runs a thread stays its owner. A copy is read-only and never deleted when the original is."
    >
      <Row
        title="Keep a copy of other environments' threads"
        subtitle="Their threads stay readable here when they are offline, wiped or removed."
        status={loaded ? mirrorLine(status) : "Reading…"}
      >
        <Switch
          label="Keep a copy of other environments' threads"
          checked={status?.enabled ?? true}
          onChange={(enabled) => apply({ enabled })}
        />
      </Row>
      <Row
        title="Projects left out"
        subtitle="One folder per line. Threads in these folders, or inside them, are not copied."
        stacked
      >
        <textarea
          aria-label="Projects left out of the copy"
          className={`${fieldClass} h-20 w-full resize-y py-2 font-mono`}
          placeholder="/Users/me/code/private"
          value={projects}
          disabled={!loaded || !status}
          onChange={(event) => setProjects(event.target.value)}
          onBlur={() => {
            const next = parseProjects(projects);
            if (JSON.stringify(next) !== JSON.stringify(status?.excludedProjects ?? [])) {
              apply({ excludedProjects: next });
            }
          }}
        />
      </Row>
    </Group>
  );
}
