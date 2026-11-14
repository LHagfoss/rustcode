import { useState } from "react";
import type { PermissionRequest } from "../acp/types";
import { answerSecret, secretTargets, useApp } from "../state/store";
import { isRemoteThread } from "../state/threadKeys";

/** Local field state is discarded as soon as the response leaves this card. */
export function SecretCard({ request, from }: { request: PermissionRequest; from?: string }) {
  const [value, setValue] = useState("");
  const [show, setShow] = useState(false);
  const thread = useApp((s) => s.threads[request.sessionId]);
  const secret = request.secret!;
  const send = (answer: string | null) => {
    if (answerSecret(request, answer)) setValue("");
  };
  return (
    <form
      className="rounded-xl bg-surface p-4 space-y-3"
      autoComplete="off"
      onSubmit={(e) => {
        e.preventDefault();
        if (value) send(value);
      }}
    >
      <div className="text-xs text-text-muted">
        {from || thread?.title || "Thread"} · {thread?.harness?.id || "Agent"}
      </div>
      <div className="text-sm font-medium">Secret {secret.name}</div>
      <p className="text-sm text-text-muted">{secret.reason}</p>
      {secretTargets(secret).map((target) => (
        <p key={target} className="text-xs text-text-muted break-all first-letter:uppercase">
          {target}
        </p>
      ))}
      {isRemoteThread(request.sessionId) && (
        <p className="text-xs text-warning">The value will be sent to the machine running this thread.</p>
      )}
      <label className="block text-xs text-text-muted">
        {secret.name}
        <input
          className="mt-1 w-full rounded-md bg-surface-2 px-3 py-2 text-sm text-text outline-none focus-visible:ring-2 focus-visible:ring-focus"
          type={show ? "text" : "password"}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          autoComplete="new-password"
          spellCheck={false}
          autoCapitalize="none"
          data-1p-ignore
          data-lpignore="true"
        />
      </label>
      <div className="flex items-center gap-3 text-xs">
        <button type="button" className="text-text-muted" aria-pressed={show} onClick={() => setShow(!show)}>
          {show ? "Hide" : "Show"}
        </button>
        <span className="flex-1" />
        <button type="button" className="rounded-md px-3 py-2 text-text-muted" onClick={() => send(null)}>
          Decline
        </button>
        <button
          type="submit"
          className="rounded-md bg-accent px-3 py-2 text-accent-text disabled:opacity-40"
          disabled={!value}
        >
          Save
        </button>
      </div>
    </form>
  );
}
