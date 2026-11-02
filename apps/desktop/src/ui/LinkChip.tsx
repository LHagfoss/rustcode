// A link as a compact chip, like the composer's slash-command token: favicon
// and title on a tint of the page's own colour. The metadata comes from the
// shell (never fetched by the webview); until it arrives, or without it, the
// chip shows the host on a neutral fill.

import { ArrowUpRight, Globe, X } from "lucide-react";
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
          "group/link inline-flex h-[22px] max-w-[340px] shrink-0 items-center gap-1 rounded-md pr-1.5 pl-1 align-[-5px] text-callout leading-none hover:brightness-125",
          !tint && "bg-fill shadow-[inset_0_0_0_0.5px_var(--color-border)]",
          className,
        )}
        style={
          tint
            ? {
                background: `color-mix(in srgb, ${tint} 14%, var(--color-fill))`,
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
          className="inline-flex min-w-0 cursor-pointer items-center gap-1.5 text-fg no-underline"
        >
          {meta?.icon ? (
            <img src={meta.icon} alt="" className="size-4 shrink-0 rounded-xs" draggable={false} />
          ) : (
            <Globe size={13} className="ml-0.5 shrink-0 text-fg-muted" />
          )}
          <span className="truncate font-medium">{title}</span>
          {meta?.title && meta.title.trim() !== host && (
            <span className="shrink-0 text-caption text-fg-faint">{host}</span>
          )}
          {!onRemove && (
            <ArrowUpRight
              size={11}
              className="-ml-0.5 shrink-0 text-fg-faint group-hover/link:text-fg-muted"
            />
          )}
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
