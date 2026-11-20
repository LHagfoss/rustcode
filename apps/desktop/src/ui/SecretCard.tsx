import { useState } from "react";
import type { PermissionRequest } from "../acp/types";
import { answerSecret, secretTargets, useApp } from "../state/store";
import { isRemoteThread } from "../state/threadKeys";
import { Pill } from "./primitives";

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
      <div className="text-caption text-fg-muted">
        {from || thread?.title || "Thread"} · {thread?.harness?.id || "Agent"}
      </div>
      <div className="text-callout font-medium text-fg">Secret {secret.name}</div>
      <p className="text-callout text-fg-muted">{secret.reason}</p>
      {secretTargets(secret).map((target) => (
        <p key={target} className="text-caption text-fg-muted break-all first-letter:uppercase">
          {target}
        </p>
      ))}
      {isRemoteThread(request.sessionId) && (
        <p className="text-caption text-warning">
          The value will be sent to the machine running this thread.
        </p>
      )}
      <label className="block text-caption text-fg-muted">
        {secret.name}
        <input
          className="mt-1 h-7 w-full rounded-lg bg-fill px-3 text-callout text-fg outline-none placeholder:text-fg-faint hover:bg-fill-strong focus:bg-fill-strong keyboard:focus-visible:ring-2 keyboard:focus-visible:ring-focus/40"
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
      <div className="flex items-center gap-2">
        <Pill quiet aria-pressed={show} onClick={() => setShow(!show)}>
          {show ? "Hide" : "Show"}
        </Pill>
        <span className="flex-1" />
        <Pill quiet onClick={() => send(null)}>
          Decline
        </Pill>
        <Pill prominent type="submit" disabled={!value}>
          Save
        </Pill>
      </div>
    </form>
  );
}
