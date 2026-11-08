// A phone asking to connect, answered from anywhere in the app (docs/remote.md › Connection
// requests). The phone found this Mac on the network and tapped Connect; the gateway holds the
// request and this asks: "<device> wants to connect", Deny or Accept. Accept shows the code the
// phone now asks for, large, with Copy and its countdown, until the phone pairs with it or it runs
// out. Glass cards in the window's bottom-right corner, over Settings too, like the toasts above.
//
// The machine's remote status is read here for the whole app: every few seconds while the
// gateway runs, rarely while it does not, so a request reaches the user whichever page is open.

import { Copy, Smartphone, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { RemoteConnectRequest } from "../acp/remote";
import {
  type ConnectCode,
  clock,
  connectPollMs,
  endedMessage,
  platformLabel,
  promptedRequests,
  settleCode,
} from "../state/connectRequests";
import { answerConnectRequest, closeConnectCode, refreshRemote, toast, useApp } from "../state/store";
import { copyText, Pill, Spinner } from "./primitives";

/** At most this many requests are asked about at once; the gateway holds four. */
const SHOWN = 4;

export function ConnectRequests() {
  const engine = useApp((s) => s.status);
  const remote = useApp((s) => s.remote);
  const code = useApp((s) => s.connectCode);
  const [now, setNow] = useState(() => Date.now());
  const [readAt, setReadAt] = useState(() => Date.now());
  const [answering, setAnswering] = useState<string | null>(null);

  useEffect(() => {
    if (engine !== "ready") return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      const read = await refreshRemote();
      if (stopped) return;
      timer = setTimeout(() => void tick(), connectPollMs(read ?? useApp.getState().remote));
    };
    void tick();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [engine]);

  // The countdowns run from the moment of the read they came with.
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new answer is a new read
  useEffect(() => setReadAt(Date.now()), [remote]);

  const prompts = promptedRequests(remote).slice(0, SHOWN);
  const showing = prompts.length > 0 || code !== null;
  useEffect(() => {
    if (!showing) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [showing]);

  // The code goes once the phone used it, or once it can no longer be used.
  useEffect(() => {
    if (!code) return;
    const settled = settleCode(code, remote, now);
    if (settled.kind === "open") return;
    closeConnectCode();
    if (settled.kind === "paired") toast(`${settled.deviceName} is paired.`, "success");
    else toast(endedMessage(settled.end, settled.deviceName), "info");
  }, [code, remote, now]);

  if (!showing) return null;
  const answer = async (id: string, accept: boolean) => {
    setAnswering(id);
    await answerConnectRequest(id, accept);
    setAnswering(null);
  };
  return (
    <section
      aria-label="Devices asking to connect"
      className="pointer-events-none fixed right-4 bottom-4 z-50 flex w-[340px] max-w-[calc(100vw-32px)] flex-col gap-2"
    >
      {code && <CodeCard code={code} now={now} />}
      {prompts.map((request) => (
        <RequestCard
          key={request.id}
          request={request}
          left={request.expiresInSecs - (now - readAt) / 1000}
          busy={answering === request.id}
          onAnswer={(accept) => void answer(request.id, accept)}
        />
      ))}
    </section>
  );
}

function RequestCard({
  request,
  left,
  busy,
  onAnswer,
}: {
  request: RemoteConnectRequest;
  left: number;
  busy: boolean;
  onAnswer: (accept: boolean) => void;
}) {
  return (
    <div
      role="group"
      aria-label={`${request.deviceName} wants to connect`}
      data-connect-request={request.id}
      className="elevated rc-toast-in pointer-events-auto p-4"
    >
      <div className="flex items-start gap-3">
        <Smartphone size={16} className="mt-1 shrink-0 text-fg-muted" aria-hidden />
        <div className="min-w-0 flex-1">
          <div className="truncate font-medium text-body text-fg">{request.deviceName} wants to connect</div>
          <div className="pt-1 text-callout text-fg-muted">
            {platformLabel(request.platform)}
            {request.remoteAddr ? ` at ${request.remoteAddr}` : ""}
          </div>
          <p className="m-0 pt-2 text-caption text-fg-faint leading-normal">
            Accept only a device you have with you. This Mac then shows a code to enter on it.
          </p>
        </div>
      </div>
      <div className="flex items-center gap-2 pt-3">
        <span className="tabular flex-1 text-caption text-fg-faint">{clock(left)} left</span>
        <Pill disabled={busy} onClick={() => onAnswer(false)}>
          Deny
        </Pill>
        <Pill prominent disabled={busy} onClick={() => onAnswer(true)}>
          {busy && <Spinner />}
          Accept
        </Pill>
      </div>
    </div>
  );
}

function CodeCard({ code, now }: { code: ConnectCode; now: number }) {
  const left = (code.expiresAt - now) / 1000;
  return (
    <div
      role="group"
      aria-label={`Pairing code for ${code.deviceName}`}
      data-connect-code={code.requestId}
      className="elevated rc-toast-in pointer-events-auto p-4"
    >
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="truncate font-medium text-body text-fg">Enter this code on {code.deviceName}</div>
          <div className="pt-1 text-callout text-fg-muted">The phone asks for it now. It works once.</div>
        </div>
        <button
          type="button"
          aria-label="Hide the code"
          onClick={closeConnectCode}
          className="flex size-6 shrink-0 items-center justify-center rounded-full text-fg-faint hover:bg-fill hover:text-fg active:bg-fill-strong"
        >
          <X size={12} />
        </button>
      </div>
      <div className="flex items-center gap-3 pt-3">
        <span className="tabular flex-1 select-text font-mono font-semibold text-title text-fg tracking-[0.08em]">
          {code.code}
        </span>
        <Pill aria-label="Copy the pairing code" onClick={() => void copyText(code.code)}>
          <Copy size={12} /> Copy
        </Pill>
      </div>
      <div className="tabular pt-2 text-caption text-fg-faint">
        Expires in {clock(left)}
        {code.lockedForSecs
          ? ` · Pairing is locked for ${clock(code.lockedForSecs)} after too many wrong codes`
          : ""}
      </div>
    </div>
  );
}
