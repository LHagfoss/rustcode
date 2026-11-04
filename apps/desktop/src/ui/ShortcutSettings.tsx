// Settings › Keyboard: every shortcut the app answers, from the one table in
// lib/keys.ts. A command the user can change gets a row with its key in a Kbd:
// click it, press the new combination, and it is kept (Esc cancels). The
// recording rules and the conflict check come from the same module the app's
// key handler matches with, so this page and the handler cannot disagree.
//
// One row records at a time, and while it does the page takes the keyboard
// before anything else: no shortcut fires, and the settings search does not
// see the keys either.
//
// The composer's own keys are listed below, unchangeable in this pass.

import { useCallback, useEffect, useState } from "react";
import {
  APP_COMMANDS,
  type AppCommand,
  bindingFromEvent,
  bindingOf,
  bindingProblem,
  commandHint,
} from "../lib/keys";
import { resetShortcut, setShortcut, useApp } from "../state/store";
import { cx, Kbd } from "./primitives";
import { Group, Row } from "./settingsKit";

export function ShortcutsPage() {
  const overrides = useApp((s) => s.shortcuts);
  /** The command whose key is being recorded, if one is. */
  const [recording, setRecording] = useState<string | null>(null);
  /** Why the last keys pressed were refused, under the row that is recording. */
  const [problem, setProblem] = useState<string | null>(null);

  // Both the recorder and the row's reset end a recording; nothing else does.
  const stop = useCallback(() => {
    setRecording(null);
    setProblem(null);
  }, []);

  // Capture: the key that becomes a binding must not also open the palette,
  // close Settings, or land in the search field.
  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        stop();
        return;
      }
      const next = bindingFromEvent(e);
      // A modifier on its own, or a key the table does not name: keep waiting.
      if (!next) return;
      const why = bindingProblem(overrides, recording, next);
      if (why) {
        setProblem(why);
        return;
      }
      setShortcut(recording, next);
      stop();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [recording, overrides, stop]);

  return (
    <>
      <Group
        title="Shortcuts"
        footer="Click a key to change it, then press the new combination; Esc leaves it as it was. A shortcut needs ⌘ (or ⌥) unless it is a function key, and it may not take a key another command answers."
      >
        {APP_COMMANDS.filter((command) => command.remappable).map((command) => (
          <ShortcutRow
            key={command.id}
            command={command}
            overrides={overrides}
            recording={recording === command.id}
            problem={recording === command.id ? problem : null}
            onToggle={() => {
              if (recording === command.id) {
                stop();
                return;
              }
              setProblem(null);
              setRecording(command.id);
            }}
            onReset={() => {
              stop();
              resetShortcut(command.id);
            }}
          />
        ))}
      </Group>
      <Group
        title="While typing"
        footer="The message box and the approvals it shows keep these: they are not changed here."
      >
        {APP_COMMANDS.filter((command) => !command.remappable).map((command) => (
          <Row key={command.id} title={command.label}>
            <Kbd>{commandHint(command.binding, command)}</Kbd>
          </Row>
        ))}
      </Group>
    </>
  );
}

/** One changeable command: its key, the recorder, and why a key was refused. */
function ShortcutRow({
  command,
  overrides,
  recording,
  problem,
  onToggle,
  onReset,
}: {
  command: AppCommand;
  overrides: Record<string, string>;
  recording: boolean;
  problem: string | null;
  onToggle: () => void;
  onReset: () => void;
}) {
  const binding = bindingOf(overrides, command.id);
  const changed = overrides[command.id] !== undefined;
  return (
    <Row title={command.label} status={problem} reset={changed ? onReset : undefined}>
      <button
        type="button"
        aria-pressed={recording}
        aria-label={
          recording
            ? `${command.label}: press the new keys, Esc to cancel`
            : `${command.label}: ${commandHint(binding, command)}. Change it`
        }
        onClick={onToggle}
        className={cx(
          "flex h-6 min-w-16 items-center justify-center rounded-md border px-1.5",
          recording
            ? "border-accent bg-surface-3 text-fg"
            : "border-border text-fg-muted hover:bg-fill-subtle hover:text-fg",
        )}
      >
        {recording ? (
          <span className="text-caption">Press keys…</span>
        ) : (
          <Kbd>{commandHint(binding, command)}</Kbd>
        )}
      </button>
    </Row>
  );
}
