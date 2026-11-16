// Why the grant card appears for a thread whose access covers the work (state/host.ts
// `permitted`): only a named account to spend on asks then. Its own file, so the card in
// Links.tsx gains one line.

import type { GrantAsk } from "../state/orchestration";
import { ACCOUNT_PROMPT_WHY, delegationAllowed } from "../state/permissions";
import { permissionModeOf, useApp } from "../state/store";
import { Info } from "./icons";

export function GrantWhy({ grant }: { grant: GrantAsk }) {
  const covered = useApp((s) => delegationAllowed(permissionModeOf(s, grant.caller), grant.kind));
  if (!covered) return null;
  return (
    <div className="flex items-start gap-1.5 rounded-lg bg-fill-subtle px-3 py-2 text-callout text-fg-muted">
      <Info size={13} className="mt-[3px] shrink-0" />
      <span>{ACCOUNT_PROMPT_WHY}</span>
    </div>
  );
}
