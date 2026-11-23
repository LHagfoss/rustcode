import { Dialog } from "radix-ui";
import { useEffect, useState } from "react";
import { isTauri } from "../acp/transport";
import { openExternal } from "../lib/native";
import { shellCall } from "../lib/shell";
import { errorText, toast, useApp } from "../state/store";
import { splitThreadKey } from "../state/threadKeys";
import { connectionFor, connectionForThread } from "../state/threadMachines";

const SHARE_METHOD = "_rustcode/preview/share";
const action = "rounded-md bg-fill px-3 py-1.5 text-callout hover:bg-fill-strong disabled:opacity-50";

interface Link {
  id: string;
  url: string;
  port: number;
  expiresAt: string;
}

type MachineLink = Link & { host: string };

export function ThreadPreview({ threadId }: { threadId: string }) {
  const [open, setOpen] = useState(false);
  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Trigger className="inline-flex h-7 items-center rounded-full px-3 text-callout text-fg-muted hover:bg-fill hover:text-fg">
        Preview
      </Dialog.Trigger>
      {open && <PreviewDialog key={threadId} threadId={threadId} />}
    </Dialog.Root>
  );
}

function PreviewDialog({ threadId }: { threadId: string }) {
  const [port, setPort] = useState("");
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [links, setLinks] = useState<MachineLink[]>([]);
  const [error, setError] = useState("");
  const { host, id: sessionId } = splitThreadKey(threadId);
  const value = Number(port);
  const validPort = /^[0-9]+$/.test(port) && value > 0 && value <= 65535;
  const privateAvailable = isTauri();
  const hosts = useApp((state) => state.connectedHosts);
  const names = useApp((state) => state.machineNames);

  useEffect(() => {
    let closed = false;
    const refresh = async () => {
      const results = await Promise.allSettled(
        ["", ...hosts].map(async (machine) => {
          const connection = connectionFor(machine);
          if (!connection) return [];
          const result = await connection.request<{ links: Link[] }>(
            SHARE_METHOD,
            { action: "list" },
            10_000,
          );
          return result.links.map((link) => ({ ...link, host: machine }));
        }),
      );
      if (closed) return;
      setLinks(results.flatMap((result) => (result.status === "fulfilled" ? result.value : [])));
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 5_000);
    return () => {
      closed = true;
      clearInterval(timer);
    };
  }, [hosts]);

  async function perform(work: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await work();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  function openPrivate() {
    void perform(async () => {
      const url = host
        ? await shellCall<string>("machines_preview", { host, sessionId, port: value })
        : `http://127.0.0.1:${value}/`;
      openExternal(url);
    });
  }

  function share() {
    setConfirm(false);
    void perform(async () => {
      const connection = connectionForThread(threadId);
      if (!connection) throw new Error("The thread’s machine is offline.");
      const link = await connection.request<Link>(
        SHARE_METHOD,
        { action: "start", sessionId, port: value, approved: true },
        35_000,
      );
      setLinks((current) => [...current.filter((entry) => entry.id !== link.id), { ...link, host }]);
    });
  }

  function stop({ id, host }: MachineLink) {
    void perform(async () => {
      const connection = connectionFor(host);
      if (!connection) throw new Error("The thread’s machine is offline.");
      await connection.request(SHARE_METHOD, { action: "stop", id }, 5_000);
      setLinks((current) => current.filter((entry) => entry.id !== id));
    });
  }

  return (
    <Dialog.Portal>
      <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
      <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
        <Dialog.Content className="elevated pointer-events-auto flex max-h-[calc(100%-48px)] w-[min(520px,calc(100vw-48px))] flex-col gap-4 overflow-y-auto p-5">
          <div className="flex items-center justify-between gap-3">
            <Dialog.Title className="font-semibold text-title3">Preview</Dialog.Title>
            <Dialog.Close className={action}>Close</Dialog.Close>
          </div>
          <Dialog.Description className="text-callout text-fg-muted">
            Enter the port of the HTTP server running on this thread’s machine.
          </Dialog.Description>
          <label className="flex items-center gap-3 text-callout">
            Port
            <input
              type="text"
              inputMode="numeric"
              value={port}
              maxLength={5}
              onChange={(event) => {
                setPort(event.target.value);
                setConfirm(false);
              }}
              className="w-24 rounded-md bg-fill-subtle px-2 py-1.5"
            />
          </label>
          <div className="flex flex-wrap gap-2">
            {privateAvailable && (
              <button type="button" className={action} disabled={busy || !validPort} onClick={openPrivate}>
                Open privately
              </button>
            )}
            <button
              type="button"
              className={action}
              disabled={busy || !validPort}
              onClick={() => setConfirm(true)}
            >
              Share preview
            </button>
          </div>
          {confirm && (
            <div className="flex flex-col gap-2 rounded-md bg-fill-subtle p-3">
              <p className="text-callout">
                Anyone with the link can access port {value} for one hour. Dev servers can expose source
                files. Serve a static build if you only want to share build output.
              </p>
              <div className="flex gap-2">
                <button type="button" className={action} disabled={busy} onClick={share}>
                  Create public link
                </button>
                <button type="button" className={action} onClick={() => setConfirm(false)}>
                  Cancel
                </button>
              </div>
            </div>
          )}
          {error && (
            <p role="alert" className="text-callout text-fg-muted">
              {error}
            </p>
          )}
          <div className="flex flex-col gap-2">
            <h3 className="font-medium text-callout">Active public links</h3>
            {links.length === 0 && <p className="text-callout text-fg-muted">No active links</p>}
            {links.map((link) => (
              <div key={link.id} className="flex flex-col gap-2 rounded-md bg-fill-subtle p-3">
                <button
                  type="button"
                  className="truncate text-left text-callout underline"
                  onClick={() => openExternal(link.url)}
                >
                  {link.url}
                </button>
                <p className="text-caption text-fg-muted">
                  {link.host ? (names[link.host] ?? link.host) : "This computer"} · Port {link.port} · Expires{" "}
                  {new Date(link.expiresAt).toLocaleTimeString()}
                </p>
                <div className="flex gap-2">
                  <button
                    type="button"
                    className={action}
                    onClick={() =>
                      void navigator.clipboard.writeText(link.url).then(
                        () => toast("Preview link copied", "success"),
                        (reason) => toast(errorText(reason), "error"),
                      )
                    }
                  >
                    Copy link
                  </button>
                  <button type="button" className={action} disabled={busy} onClick={() => stop(link)}>
                    Stop
                  </button>
                </div>
              </div>
            ))}
          </div>
        </Dialog.Content>
      </div>
    </Dialog.Portal>
  );
}
