// The RustCode server's warnings (`rustcode server status`), as a callout under
// its row in Settings › Remote access and › Machines: each one in readable
// text with what fixes it, when the app can (point a terminal's `rustcode` at
// this app, install the server from this app), else the command to run, and a
// Dismiss that hides it until its words change (state/settingsAttention.ts).

import { installServer, linkCliToApp, useMachines } from "../state/machines";
import {
  dismissProblem,
  problemCommand,
  type ServerProblem,
  useDismissedProblems,
  visibleProblems,
} from "../state/settingsAttention";
import { Copy, Server, SquareTerminal, TriangleAlert, X } from "./icons";
import { copyText, Spinner } from "./primitives";
import { Action, withCode } from "./settingsKit";

function Fix({ problem }: { problem: ServerProblem }) {
  const linking = useMachines((s) => s.serverLink.status === "running");
  const installing = useMachines((s) => s.serverInstall.status === "running");
  if (problem.code === "path-version-mismatch") {
    return (
      <Action size="sm" disabled={linking} onClick={() => void linkCliToApp()}>
        {linking ? <Spinner /> : <SquareTerminal size={12} />}
        Use this app's version
      </Action>
    );
  }
  if (problem.code === "path-version-mismatch-server") {
    return (
      <Action size="sm" disabled={installing} onClick={() => void installServer()}>
        {installing ? <Spinner /> : <Server size={12} />}
        Run the server from this app
      </Action>
    );
  }
  const command = problemCommand(problem.message);
  if (!command) return null;
  return (
    <Action size="sm" aria-label={`Copy ${command}`} onClick={() => void copyText(command)}>
      <Copy size={12} />
      Copy command
    </Action>
  );
}

export function ServerProblems({ problems }: { problems: readonly ServerProblem[] | undefined }) {
  const dismissed = useDismissedProblems((s) => s.keys);
  const shown = visibleProblems(problems ?? [], dismissed);
  if (shown.length === 0) return null;
  return (
    <ul className="mt-2 flex list-none flex-col gap-1.5 p-0" aria-label="Server warnings">
      {shown.map((problem) => (
        <li
          key={problem.code}
          className="flex items-start gap-2 rounded-lg bg-warning/10 px-3 py-2 text-callout text-fg"
        >
          <TriangleAlert size={14} className="mt-0.5 shrink-0 text-warning" aria-hidden />
          <div className="flex min-w-0 flex-1 flex-col gap-1.5">
            <p className="m-0 break-words leading-normal">{withCode(problem.message)}</p>
            <div className="flex flex-wrap items-center gap-1.5">
              <Fix problem={problem} />
              <Action size="sm" quiet onClick={() => dismissProblem(problem)}>
                <X size={12} />
                Dismiss
              </Action>
            </div>
          </div>
        </li>
      ))}
    </ul>
  );
}
