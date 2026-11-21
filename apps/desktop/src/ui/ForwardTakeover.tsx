// A machine reached through a forwarded port whose tunnel, run outside RustCode (`ssh -L`), lost
// its link while this computer slept. The row says what to run, and offers to let RustCode
// manage the tunnel instead: an SSH route to the same host, which opens a fresh `ssh -W` for every
// connection and so comes back by itself after a wake (`docs/remote.md` › SSH routes).

import { useState } from "react";
import { opError, opRunning, useOp } from "../lib/op";
import { takeOverForward } from "../state/machineRoutes";
import type { Machine, MachineForward } from "../state/machines";
import { toast } from "../state/store";
import { cx, noTextHelpers, Spinner } from "./primitives";
import { Action, fieldClass, withCode } from "./settingsKit";

export function ForwardTakeover({ machine, forward }: { machine: Machine; forward: MachineForward }) {
  const [open, setOpen] = useState(false);
  const [host, setHost] = useState(forward.ssh ?? "");
  const [taking, runTake, resetTake] = useOp();
  const busy = opRunning(taking);
  const error = opError(taking);
  const take = async () => {
    if (busy || !host.trim()) return;
    const done = await runTake(async () => {
      const failed = await takeOverForward(machine, forward, host);
      if (failed) throw new Error(failed);
    });
    if (done.status === "done") toast(`RustCode now manages the tunnel to ${host.trim()}.`, "success");
  };
  return (
    <span className="mt-1 flex flex-col items-start gap-2" data-forward-takeover>
      {machine.detail && (
        <span className="selectable block break-words text-fg-muted">{withCode(machine.detail)}</span>
      )}
      {!open ? (
        <Action prominent onClick={() => setOpen(true)}>
          Let RustCode manage this tunnel
        </Action>
      ) : (
        <span className="flex w-full flex-col gap-2">
          <span className="text-caption text-fg-muted">
            RustCode signs in with your own ssh (its config and agent) and opens a fresh tunnel for each
            connection, so the machine comes back by itself after a wake. {forward.address} is turned off once
            the host answers; turn it back on in this machine's settings to undo.
          </span>
          <span className="flex w-full flex-wrap items-center gap-2">
            <input
              autoFocus
              value={host}
              onChange={(event) => {
                setHost(event.target.value);
                if (error) resetTake();
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") void take();
                if (event.key === "Escape") setOpen(false);
              }}
              placeholder="lars@box or an alias from ~/.ssh/config"
              aria-label="SSH host behind the tunnel"
              disabled={busy}
              {...noTextHelpers}
              className={cx(fieldClass, "min-w-0 flex-1 font-mono")}
            />
            <Action prominent disabled={busy || !host.trim()} onClick={() => void take()}>
              {busy ? <Spinner /> : null}
              {busy ? "Testing…" : "Manage tunnel"}
            </Action>
            <Action quiet disabled={busy} onClick={() => setOpen(false)}>
              Cancel
            </Action>
          </span>
          {error && (
            <span role="alert" className="text-callout text-danger leading-normal">
              {error}
            </span>
          )}
        </span>
      )}
    </span>
  );
}
