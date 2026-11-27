// The files a message carries, shared by the composer and the question form (AttentionCards.tsx
// QuestionCard): reading pasted, dropped and picked files into attachments under the composer's
// limits (state/compose.ts), and the strip of thumbnails and chips that shows them with Remove.

import { useLayoutEffect, useRef, useState } from "react";
import { readImage } from "../lib/native";
import { basename } from "../lib/paths";
import {
  type Attachment,
  IMAGE_TYPES,
  imageMime,
  MAX_IMAGE_BYTES,
  MAX_INLINE_FILE_BYTES,
} from "../state/compose";
import { toast } from "../state/store";
import { localId } from "../state/thread";
import { ChevronUp, FileText, X } from "./icons";
import { openLightbox } from "./Lightbox";
import { PasteChip } from "./PasteChip";
import { cx, Pill, Tooltip } from "./primitives";

function readFileAs(file: File, as: "base64" | "text"): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error("Could not read the file."));
    reader.onload = () => {
      const result = String(reader.result ?? "");
      resolve(as === "base64" ? result.slice(result.indexOf(",") + 1) : result);
    };
    if (as === "base64") reader.readAsDataURL(file);
    else reader.readAsText(file);
  });
}

/** Browser files (paste, drop, the file input) as attachments. */
export async function attachmentsFromFiles(
  files: File[],
  images: boolean,
  harnessName: string,
): Promise<Attachment[]> {
  const out: Attachment[] = [];
  for (const file of files) {
    const mime = Object.values(IMAGE_TYPES).includes(file.type) ? file.type : imageMime(file.name);
    if (mime) {
      if (!images) {
        toast(`${harnessName} does not take images`, "info");
        continue;
      }
      if (file.size > MAX_IMAGE_BYTES) {
        toast(`${file.name || "The image"} is over 20 MB`, "info");
        continue;
      }
      out.push({
        id: localId("image"),
        kind: "image",
        name: file.name || "Pasted image",
        mimeType: mime,
        data: await readFileAs(file, "base64"),
      });
    } else if (file.size <= MAX_INLINE_FILE_BYTES) {
      out.push({ id: localId("file"), kind: "file", name: file.name, text: await readFileAs(file, "text") });
    } else {
      toast(`${file.name} is too big to send inline here; the app attaches it by path`, "info");
    }
  }
  return out;
}

/** Paths from the open panel or a drop in the app: images are read, other files go by path. */
export async function attachmentsFromPaths(
  paths: string[],
  images: boolean,
  harnessName: string,
): Promise<Attachment[]> {
  const out: Attachment[] = [];
  for (const path of paths) {
    const name = basename(path) || path;
    if (imageMime(name) && images) {
      try {
        const image = await readImage(path);
        out.push({ id: localId("image"), kind: "image", ...image });
      } catch (error) {
        toast(`Could not attach ${name}: ${error instanceof Error ? error.message : String(error)}`, "error");
      }
    } else {
      if (imageMime(name)) toast(`${harnessName} does not take images; ${name} goes as a path`);
      out.push({ id: localId("file"), kind: "file", name, path });
    }
  }
  return out;
}

export function AttachmentStrip({
  attachments,
  onRemove,
  className = "px-5 pt-5 pb-1",
}: {
  attachments: readonly Attachment[];
  onRemove: (id: string) => void;
  /** Its inset: the composer's clears the card's rounder corner. */
  className?: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const images = attachments.filter((a) => a.kind === "image");
  const pictures = images.map((a) => ({ src: `data:${a.mimeType};base64,${a.data}`, alt: a.name }));
  const grid = useRef<HTMLDivElement>(null);
  const [columns, setColumns] = useState(1);
  const hasImages = images.length > 0;
  useLayoutEffect(() => {
    const element = grid.current;
    if (!hasImages || !element) return;
    const measure = () => {
      const gap = Number.parseFloat(getComputedStyle(element).columnGap);
      const rem = Number.parseFloat(getComputedStyle(document.documentElement).fontSize);
      setColumns(Math.max(1, Math.floor((element.clientWidth + gap) / (4 * rem + gap))));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [hasImages]);
  const hiddenCount = Math.max(0, images.length - columns);
  const shown = expanded || !hiddenCount ? images : images.slice(0, columns);
  const thumbnails = shown.map((a, index) => (
    <div key={a.id} className="group relative min-w-0 rc-in">
      <button
        type="button"
        aria-label={`Open ${a.name}`}
        onClick={() => openLightbox(pictures, index)}
        className="relative block aspect-square w-full overflow-hidden rounded-lg bg-code-bg after:pointer-events-none after:absolute after:inset-0 after:rounded-lg after:ring-1 after:ring-inset after:ring-border hover:after:bg-bg/30 focus-visible:outline-none focus-visible:after:ring-accent"
      >
        <img src={pictures[index]?.src} alt={a.name} title={a.name} className="size-full object-contain" />
      </button>
      {/* Inside the corner, so the strip's scroll never crops it. */}
      <button
        type="button"
        aria-label={`Remove ${a.name}`}
        onClick={() => onRemove(a.id)}
        className="rc-glass absolute top-1.5 right-1.5 flex size-6 items-center justify-center rounded-full text-fg-muted shadow-1 hover:bg-surface-3 hover:text-fg [@media(hover:hover)]:opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
      >
        <X size={12} strokeWidth={2} />
      </button>
    </div>
  ));
  return (
    // Its own band above the field, inset from the box's rounder corner and never shrinking, so the
    // thumbnails clear the curve and only the field below scrolls.
    <div className={cx("flex min-w-0 shrink-0 flex-col gap-2", className)}>
      {hasImages && (
        <div
          ref={grid}
          style={
            !expanded && hiddenCount
              ? { gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr)) minmax(0, 2rem)` }
              : undefined
          }
          className="grid max-h-56 content-start grid-cols-[repeat(auto-fill,minmax(min(100%,4rem),1fr))] gap-2 overflow-y-auto"
        >
          {thumbnails}
          {hiddenCount > 0 && (
            <Pill
              quiet
              aria-label={
                expanded
                  ? "Show fewer images"
                  : `Show ${hiddenCount} more ${hiddenCount === 1 ? "image" : "images"}`
              }
              aria-expanded={expanded}
              onClick={() => setExpanded(!expanded)}
              className={cx(
                "h-auto! w-full rounded-lg! bg-fill-subtle px-0! tabular-nums ring-1 ring-inset ring-border",
                expanded && "aspect-square",
              )}
            >
              {expanded ? <ChevronUp size={16} /> : `+${hiddenCount}`}
            </Pill>
          )}
        </div>
      )}
      {attachments.some((a) => a.kind !== "image") && (
        <div className="flex items-start gap-2 overflow-x-auto">
          {attachments.map((a) =>
            // A long paste keeps its size in the chip, and its text in the panel it opens.
            a.kind === "text" ? (
              <PasteChip key={a.id} text={a.text} onRemove={() => onRemove(a.id)} />
            ) : a.kind === "file" ? (
              <Tooltip key={a.id} label={a.path ?? `${a.name} (sent inline)`}>
                <span className="flex h-[26px] shrink-0 items-center gap-1.5 rounded-full bg-fill-subtle pr-1.5 pl-2 text-callout rc-in">
                  <FileText size={12} className="text-fg-muted" />
                  <span className="max-w-[200px] truncate">{a.name}</span>
                  <button
                    type="button"
                    aria-label={`Remove ${a.name}`}
                    onClick={() => onRemove(a.id)}
                    className="text-fg-faint hover:text-fg"
                  >
                    <X size={12} />
                  </button>
                </span>
              </Tooltip>
            ) : null,
          )}
        </div>
      )}
    </div>
  );
}
