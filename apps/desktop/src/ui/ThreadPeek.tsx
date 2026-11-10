// A thread next to the one you are in: the panel's Thread tab reads another
// thread live, so you can watch a thread you started without leaving yours.
// To write in a thread seen here, open it. A side chat is the exception: it was opened to be
// asked in beside its parent, so its tab has a field of its own (`SideChatField`), and closing
// the panel keeps the thread.

import { ExternalLink, MessageSquareQuote } from "lucide-react";
import { useCallback } from "react";
import { modelName, relationshipOf } from "../acp/types";
import { cancel, runPrompt, selectThread, setDraft, useApp } from "../state/store";
import { PermissionMenu } from "./Composer";
import { HarnessIcon } from "./HarnessIcon";
import { noTextHelpers, Pill, Spinner } from "./primitives";
import { Transcript } from "./Transcript";

/** The peeked thread's composer when it is a side chat: send, or stop the turn it is running. */
function SideChatField({ id }: { id: string }) {
  const draft = useApp((s) => s.drafts[id] ?? "");
  const running = useApp((s) => !!s.threads[id]?.running);
  // Opened from a message, the field starts on its quote: it mounts per thread (`key`), takes the
  // keyboard and puts the caret at the end of the quote.
  const start = useCallback((el: HTMLTextAreaElement | null) => {
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  }, []);
  const send = () => {
    const text = draft.trim();
    if (!text || running) return;
    setDraft(id, "");
    void runPrompt(id, text);
  };
  return (
    <div className="shrink-0 border-border border-t p-2">
      <textarea
        ref={start}
        value={draft}
        rows={Math.min(8, Math.max(2, draft.split("\n").length))}
        placeholder="Ask in the side chat…"
        aria-label="Message the side chat"
        {...noTextHelpers}
        onChange={(e) => setDraft(id, e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            send();
          }
        }}
        className="selectable w-full resize-none rounded-lg bg-fill-subtle px-3 py-2 text-body outline-none placeholder:text-fg-faint"
      />
      <div className="mt-1 flex items-center justify-between gap-2">
        <PermissionMenu threadId={id} compact />
        {running ? (
          <Pill onClick={() => void cancel(id)}>Stop</Pill>
        ) : (
          <Pill onClick={send} aria-disabled={!draft.trim()}>
            Send
          </Pill>
        )}
      </div>
    </div>
  );
}

export function ThreadPeek() {
  const id = useApp((s) => s.peekId);
  const thread = useApp((s) => (s.peekId ? s.threads[s.peekId] : undefined));
  const session = useApp((s) => s.sessions.find((x) => x.id === s.peekId));
  const sideChat = session?.parent ? relationshipOf(session.parent) === "side_chat" : false;
  if (!id) return <div className="flex-1" />;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-border border-b px-3">
        {sideChat && <MessageSquareQuote size={13} className="shrink-0 text-fg-muted" />}
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
      {sideChat && <SideChatField key={id} id={id} />}
    </div>
  );
}
