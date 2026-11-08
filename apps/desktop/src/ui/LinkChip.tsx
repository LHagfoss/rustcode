// A link as a compact chip, like the composer's slash-command token: the
// site's favicon, or a globe when it has none, and the title on a tint of the
// page's own colour. The metadata comes from the shell (never fetched by the
// webview); until it arrives, or without it, the chip shows the host on a
// neutral fill.

import { ArrowUpRight, Globe, X } from "lucide-react";
import { useEffect, useState } from "react";
import { type LinkMeta, linkChipLabel, linkMetaCached, loadLinkMeta } from "../lib/links";
import { openExternal } from "../lib/native";
import { cx, Tooltip } from "./primitives";

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
  variant = "inline",
}: {
  url: string;
  onRemove?: () => void;
  className?: string;
  /**
   * `inline` sits in a paragraph and takes no more room than the text's own line box;
   * `token` is one line of the composer's row, level with the field's first line and with
   * the slash-command chip beside it.
   */
  variant?: "inline" | "token";
}) {
  const meta = useLinkMeta(url);
  const { title, host, showsHost } = linkChipLabel(url, meta);
  const tint = meta?.color;
  const inline = variant === "inline";
  // A favicon that will not decode is the same as no favicon: a globe, never a gap. The URL is
  // what is remembered, so a chip that later gets another icon tries that one.
  const [brokenIcon, setBrokenIcon] = useState<string | null>(null);
  const icon = meta?.icon === brokenIcon ? undefined : meta?.icon;
  return (
    <Tooltip label={<span className="break-all">{url}</span>}>
      <span
        // Never a drag source: a press on a chip and a drag across it select the words around
        // it, instead of starting a drag of the URL that no text selection survives.
        draggable={false}
        className={cx(
          // `align-middle` centres the chip on the x-height rather than on a flex baseline, and
          // that keeps it inside the line box: on the reply's 14px/22.75px line an 18px chip
          // reaches `9 - 3.65 = 5.35px` below the baseline, under the line's 6.2px of descent, so
          // a line with a chip is exactly as tall as one without. A taller chip would push every
          // line it sits on apart, which is what made the chip look off its baseline.
          // The composer's row is a flex line, where `align-middle` does nothing: the chip takes
          // the field's own line box (22.75px) and the call site gives it the field's 6px of top
          // padding, so it sits on the first line and not above it.
          "group/link inline-flex max-w-[340px] shrink-0 items-center gap-1 rounded-sm pr-1 pl-1.5 text-callout leading-none select-text hover:brightness-125",
          inline ? "h-[18px] align-middle" : "h-[22.75px]",
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
          draggable={false}
          onClick={(e) => {
            e.preventDefault();
            openExternal(url);
          }}
          className="inline-flex min-w-0 cursor-pointer items-center gap-1.5 text-fg no-underline"
        >
          {/* A favicon sits on its own light tile: sites draw them dark as often as light, and a
              dark mark straight on the chip's grey disappears in the dark theme. `bg-knob` is the
              one white in every theme, so both kinds read on it. */}
          {icon ? (
            <span className="flex size-[14px] shrink-0 items-center justify-center rounded-xs bg-knob p-px">
              <img
                src={icon}
                alt=""
                draggable={false}
                onError={() => setBrokenIcon(icon)}
                className="size-full rounded-xs object-contain"
              />
            </span>
          ) : (
            <Globe size={12} aria-hidden className="ml-0.5 shrink-0 text-fg-muted" />
          )}
          {/* One text run, so a selection across the chip copies "title host" and not
              "titlehost": the host is the faint tail of the same words, and the whole run
              truncates to the chip's cap. */}
          <span className="truncate">
            <span className="font-medium">{title}</span>
            {showsHost && <span className="text-caption text-fg-faint"> {host}</span>}
          </span>
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
            className="ml-0.5 flex shrink-0 items-center text-fg-faint hover:text-fg"
          >
            <X size={11} />
          </button>
        )}
      </span>
    </Tooltip>
  );
}
