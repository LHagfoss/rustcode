import { ShieldAlert } from "./icons";

/** What ⏎ does to the approval that waits: the message declines it and says what to do instead. */
export function DeclineNotice({ waiting }: { waiting: number }) {
  return (
    <div
      role="status"
      className="flex min-w-0 items-center gap-1.5 px-5 pt-2 text-caption text-warning rc-in"
    >
      <ShieldAlert size={12} className="shrink-0" />
      <b className="shrink-0">Sending declines the request above</b>
      <span className="min-w-0 truncate text-fg-muted">
        and tells the agent what to do instead{waiting > 1 ? ` · ${waiting - 1} more waiting` : ""}
      </span>
    </div>
  );
}

/** The Send button's tip: it names the decline while an approval waits on a written message. */
export const sendTip = (declines: boolean) => (declines ? "Decline and send (⏎)" : "Send (⏎)");
