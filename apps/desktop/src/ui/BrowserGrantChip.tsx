import { stopBrowser } from "../state/approvals";
import { browserChipLabel } from "../state/grants";
import { useApp } from "../state/store";
import { Globe, X } from "./icons";
import { Tooltip } from "./primitives";

/**
 * The thread header's "Browser: staging.example.com" chip, shown while the thread holds a browser
 * site grant. Clicking it is the stop control: the grant ends, the thread's headless
 * browser closes, and a browser card still waiting is declined.
 */
export function BrowserGrantChip({ threadId }: { threadId: string }) {
  const hosts = useApp((s) => s.browserGrants[threadId]);
  const label = browserChipLabel(hosts);
  if (!label || !hosts) return null;
  return (
    <Tooltip label={`Allowed without asking: ${hosts.join(", ")}. Click to revoke and close the browser.`}>
      <button
        type="button"
        aria-label={`${label}. Revoke and close the browser`}
        onClick={() => void stopBrowser(threadId)}
        className="group flex h-6 min-w-0 max-w-[240px] shrink items-center gap-1 rounded-sm bg-fill-subtle px-1.5 text-caption text-fg-muted hover:bg-fill hover:text-fg"
      >
        <Globe size={12} className="shrink-0" />
        <span className="min-w-0 truncate">{label}</span>
        <X size={10} className="shrink-0 text-fg-faint group-hover:text-fg" />
      </button>
    </Tooltip>
  );
}
