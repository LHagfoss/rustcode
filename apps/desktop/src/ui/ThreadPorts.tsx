import { DropdownMenu } from "radix-ui";
import { useState } from "react";
import { openExternal } from "../lib/native";
import { openBrowserPreview } from "../state/browserPreview";
import {
  formatPortLabel,
  formatPortsSummary,
  killListeningPort,
  type ListeningPort,
  useListeningPorts,
} from "../state/ports";
import { useApp } from "../state/store";
import { ChevronDown, ExternalLink, Globe, Square } from "./icons";
import { MenuContent, menuItem, menuItemDanger, Pill, Spinner, Tooltip } from "./primitives";

/**
 * Compact header pill for active listening TCP ports scoped to the current thread.
 * Renders nothing when no process is listening.
 */
export function ThreadPortsChip({ cwd }: { cwd: string | undefined }) {
  const selectedId = useApp((s) => s.selectedId);
  const ports = useListeningPorts(cwd);
  const [killing, setKilling] = useState<string | null>(null);

  if (!cwd || ports.length === 0) return null;

  const stop = async (target: ListeningPort) => {
    const key = `${target.pid}:${target.port}`;
    setKilling(key);
    try {
      await killListeningPort(cwd, target);
    } finally {
      setKilling(null);
    }
  };

  return (
    <DropdownMenu.Root modal={false}>
      <Tooltip label="Active local dev ports for this thread">
        <DropdownMenu.Trigger asChild>
          <Pill size="sm" className="shrink-0">
            <span aria-hidden className="size-1.5 rounded-full bg-success" />
            <span className="font-mono">{formatPortsSummary(ports)}</span>
            <ChevronDown size={12} />
          </Pill>
        </DropdownMenu.Trigger>
      </Tooltip>
      <MenuContent align="start" className="w-64">
        {ports.map((port, index) => {
          const key = `${port.pid}:${port.port}`;
          const busy = killing === key;
          return (
            <div key={key}>
              {index > 0 && <DropdownMenu.Separator className="my-1 h-px bg-border" />}
              <div className="flex items-center justify-between px-2 py-1 text-caption text-fg-muted">
                <span className="font-medium text-fg">{formatPortLabel(port)}</span>
                <span className="truncate font-mono">
                  {port.processName} · {port.pid}
                </span>
              </div>
              {selectedId && (
                <DropdownMenu.Item
                  className={menuItem}
                  onSelect={() => void openBrowserPreview(selectedId, port.url)}
                >
                  <Globe size={12} className="shrink-0" />
                  <span className="truncate">Preview in panel</span>
                </DropdownMenu.Item>
              )}
              <DropdownMenu.Item className={menuItem} onSelect={() => openExternal(port.url)}>
                <ExternalLink size={12} className="shrink-0" />
                <span className="truncate">Open {port.url}</span>
              </DropdownMenu.Item>
              <DropdownMenu.Item
                className={`${menuItem} ${menuItemDanger}`}
                disabled={busy}
                onSelect={() => void stop(port)}
              >
                {busy ? <Spinner size={12} /> : <Square size={12} className="shrink-0" />}
                <span>Stop process</span>
              </DropdownMenu.Item>
            </div>
          );
        })}
      </MenuContent>
    </DropdownMenu.Root>
  );
}

/**
 * Silent-when-empty section in `ThreadDetails` surfacing active listening ports
 * with one-click Open in browser and Kill process actions.
 */
export function ThreadPortsSection({ cwd }: { cwd: string | undefined }) {
  const selectedId = useApp((s) => s.selectedId);
  const ports = useListeningPorts(cwd);
  const [killing, setKilling] = useState<string | null>(null);

  if (!cwd || ports.length === 0) return null;

  const stop = async (target: ListeningPort) => {
    const key = `${target.pid}:${target.port}`;
    setKilling(key);
    try {
      await killListeningPort(cwd, target);
    } finally {
      setKilling(null);
    }
  };

  return (
    <section className="mx-2 mt-2 rounded-lg bg-fill-subtle p-2 text-callout">
      <div className="flex items-center gap-1.5">
        <Globe size={13} className="shrink-0 text-fg-muted" />
        <span className="font-medium text-fg">Listening ports</span>
        <span className="text-caption text-fg-muted tabular">{ports.length}</span>
      </div>
      <div className="mt-2 flex flex-col gap-1.5">
        {ports.map((port) => {
          const key = `${port.pid}:${port.port}`;
          const busy = killing === key;
          return (
            <div key={key} className="flex items-center gap-2 rounded-md bg-bg/60 px-2 py-1.5">
              <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-success" />
              <div className="min-w-0 flex-1">
                <div className="truncate font-mono text-caption font-medium text-fg">
                  {formatPortLabel(port)}
                </div>
                <div className="truncate text-caption text-fg-muted">
                  {port.processName} · PID {port.pid}
                </div>
              </div>
              {selectedId && (
                <Tooltip label={`Preview ${port.url} in side panel`}>
                  <Pill size="sm" quiet onClick={() => void openBrowserPreview(selectedId, port.url)}>
                    <Globe size={12} /> Preview
                  </Pill>
                </Tooltip>
              )}
              <Tooltip label={`Open ${port.url} in browser`}>
                <Pill size="sm" quiet onClick={() => openExternal(port.url)}>
                  <ExternalLink size={12} /> Open
                </Pill>
              </Tooltip>
              <Tooltip label={`Stop ${port.processName} (PID ${port.pid})`}>
                <Pill size="sm" danger disabled={busy} onClick={() => void stop(port)}>
                  {busy ? <Spinner size={12} /> : <Square size={10} />} Kill
                </Pill>
              </Tooltip>
            </div>
          );
        })}
      </div>
    </section>
  );
}
