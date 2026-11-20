// The row a Settings card shows while a saved setting waits for a server restart. One component,
// so the wording, the icon and the "not while a thread runs" rule are the same on every page.
import { restartEngine, useApp } from "../state/store";
import { isRunning } from "../state/thread";
import { RefreshCw } from "./icons";
import { Action, Row } from "./settingsKit";

export function RestartToApply({ plural, onRestart }: { plural?: boolean; onRestart?: () => void }) {
  const anyRunning = useApp((s) => Object.values(s.threads).some((t) => isRunning(t)));
  return (
    <Row
      title="Restart the server to apply"
      subtitle={`The server reads ${plural ? "these" : "this setting"} when it starts.`}
    >
      <Action
        prominent
        disabled={anyRunning}
        onClick={() => {
          onRestart?.();
          void restartEngine();
        }}
      >
        <RefreshCw size={13} />
        Restart server
      </Action>
    </Row>
  );
}
