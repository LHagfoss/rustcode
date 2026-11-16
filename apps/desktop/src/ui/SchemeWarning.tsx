// Settings › General: a warning, only when another app or another copy of RustCode opens
// `rustcode://` links instead of this one (lib/nativeAccess.ts). It never blocks anything; the
// fix makes this copy the handler where the system lets the app do that.

import { useEffect, useState } from "react";
import { claimScheme, readSchemeStatus, type SchemeStatus, schemeWarning } from "../lib/nativeAccess";
import { toast } from "../state/store";
import { TriangleAlert } from "./icons";
import { Action, Group, Row } from "./settingsKit";

export function SchemeWarning() {
  const [status, setStatus] = useState<SchemeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    void readSchemeStatus().then((next) => live && setStatus(next));
    return () => {
      live = false;
    };
  }, []);
  const warning = status && schemeWarning(status);
  if (!warning) return null;
  const fix = async () => {
    setBusy(true);
    try {
      const next = await claimScheme();
      setStatus(next);
      if (next.owner === "this") toast("rustcode:// links open in this copy now.", "success");
      else toast("The system kept the other handler.", "error");
    } catch (error) {
      toast(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Group title="Links">
      <Row
        title={
          <span className="flex items-center gap-1.5">
            <TriangleAlert size={14} className="text-warning" aria-hidden />
            {warning.title}
          </span>
        }
        subtitle={warning.text}
      >
        {warning.fix && (
          <Action onClick={() => void fix()} disabled={busy}>
            {warning.fix}
          </Action>
        )}
      </Row>
    </Group>
  );
}
