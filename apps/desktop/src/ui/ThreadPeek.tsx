// A thread next to the one you are in: the panel's Thread tab reads another
// thread live, so you can watch a thread you started without leaving yours.
// To write in it, open it.

import { ExternalLink } from "lucide-react";
import { modelName } from "../acp/types";
import { selectThread, useApp } from "../state/store";
import { HarnessIcon } from "./HarnessIcon";
import { Pill, Spinner } from "./primitives";
import { Transcript } from "./Transcript";

export function ThreadPeek() {
  const id = useApp((s) => s.peekId);
  const thread = useApp((s) => (s.peekId ? s.threads[s.peekId] : undefined));
  const session = useApp((s) => s.sessions.find((x) => x.id === s.peekId));
  if (!id) return <div className="flex-1" />;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-border border-b px-3">
        {session?.harness && <HarnessIcon id={session.harness} size={13} />}
        <span className="min-w-0 flex-1 truncate font-medium text-callout">
          {session?.title || thread?.title || "Thread"}
        </span>
        {session?.model && (
          <span className="min-w-0 max-w-[40%] shrink truncate text-caption text-fg-faint">
            {modelName(session.model)}
          </span>
        )}
        <Pill quiet onClick={() => selectThread(id)}>
          Open
          <ExternalLink size={11} />
        </Pill>
      </div>
      <div className="relative min-h-0 flex-1">
        {thread && !thread.loading ? (
          <Transcript key={id} thread={thread} bottomInset={16} peek />
        ) : (
          <div role="status" aria-label="Loading" className="flex h-full items-center justify-center">
            <Spinner size={14} />
          </div>
        )}
      </div>
    </div>
  );
}
