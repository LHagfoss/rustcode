import type { RemoteSummary } from "../state/remoteSummary";
import { Globe, Smartphone, WifiOff } from "./icons";
import { copyText, cx } from "./primitives";
import { Action, Chip } from "./settingsKit";

/** Settings › Remote access, top of the page: whether a phone can reach this computer, said once. */
export function RemoteStatusBanner({ summary }: { summary: RemoteSummary }) {
  const Icon = summary.tone === "ok" ? Globe : summary.label === "Local only" ? Smartphone : WifiOff;
  return (
    <section
      aria-label="Remote access status"
      aria-live="polite"
      className="mb-8 flex items-center gap-4 rounded-xl border border-border bg-surface px-4 py-4"
    >
      <span
        aria-hidden
        className={cx(
          "flex size-10 shrink-0 items-center justify-center rounded-full bg-fill",
          summary.tone === "ok" ? "text-success" : summary.tone === "idle" ? "text-fg-faint" : "text-warning",
          summary.tone === "bad" && "text-danger",
        )}
      >
        <Icon size={18} />
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-2 font-medium text-body text-fg">
          <span className="min-w-0 truncate">{summary.title}</span>
          <Chip tone={summary.tone}>{summary.label}</Chip>
        </div>
        {summary.detail && <div className="mt-0.5 text-callout text-fg-muted">{summary.detail}</div>}
      </div>
      {summary.address && (
        <Action aria-label="Copy the gateway address" onClick={() => void copyText(summary.address!)}>
          Copy address
        </Action>
      )}
    </section>
  );
}
