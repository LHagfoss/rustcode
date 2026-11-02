// A link as a compact chip, like the composer's slash-command token: favicon
// and title on a tint of the page's own colour. The metadata comes from the
// shell (never fetched by the webview); until it arrives, or without it, the
// chip shows the host on a neutral fill.

import { Globe, X } from "lucide-react";
import { useEffect, useState } from "react";
import { type LinkMeta, linkMetaCached, loadLinkMeta } from "../lib/links";
import { openExternal } from "../lib/native";
import { cx, Tooltip } from "./primitives";

function hostOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return url;
  }
}

export function useLinkMeta(url: string): LinkMeta | null {
  const [meta, setMeta] = useState<LinkMeta | null>(() => linkMetaCached(url));
  useEffect(() => {
    let live = true;
    setMeta(linkMetaCached(url));
    void loadLinkMeta(url).then((m) => live && setMeta(m));
    return () => {
      live = false;
    };
  }, [url]);
  return meta;
}

export function LinkChip({
  url,
  onRemove,
  className,
}: {
  url: string;
  onRemove?: () => void;
  className?: string;
}) {
  const meta = useLinkMeta(url);
  const host = hostOf(url);
  const title = meta?.title?.trim() || host;
  const tint = meta?.color;
  return (
    <Tooltip label={<span className="break-all">{url}</span>}>
      <span
        className={cx(
          "inline-flex h-[20px] max-w-[280px] shrink-0 items-center gap-1 rounded-full pr-2 pl-1 align-[-1px] text-callout leading-none",
          !tint && "bg-fill-strong shadow-[inset_0_0_0_0.5px_var(--color-border)]",
          onRemove && "pr-1",
          className,
        )}
        style={
          tint
            ? {
                background: `color-mix(in srgb, ${tint} 18%, var(--color-fill-strong))`,
                boxShadow: `inset 0 0 0 0.5px color-mix(in srgb, ${tint} 35%, var(--color-border))`,
              }
            : undefined
        }
      >
        <a
          href={url}
          onClick={(e) => {
            e.preventDefault();
            openExternal(url);
          }}
          className="inline-flex min-w-0 cursor-pointer items-center gap-1 text-fg no-underline"
        >
          {meta?.icon ? (
            <img src={meta.icon} alt="" className="size-[14px] shrink-0 rounded-xs" draggable={false} />
          ) : (
            <Globe size={12} className="ml-px shrink-0 text-fg-muted" />
          )}
          <span className="truncate font-medium">{title}</span>
          {meta?.title && <span className="shrink-0 text-fg-muted">· {host}</span>}
        </a>
        {onRemove && (
          <button
            type="button"
            aria-label={`Remove link ${host}`}
            onClick={onRemove}
            className="ml-0.5 flex shrink-0 text-fg-faint hover:text-fg"
          >
            <X size={11} />
          </button>
        )}
      </span>
    </Tooltip>
  );
}
