// The files a message carries, shared by the composer and the question form (AttentionCards.tsx
// QuestionCard): reading pasted, dropped and picked files into attachments under the composer's
// limits (state/compose.ts), and the strip of thumbnails and chips that shows them with Remove.

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
import { FileText, X } from "./icons";
import { openLightbox } from "./Lightbox";
import { PasteChip } from "./PasteChip";
import { cx, Tooltip } from "./primitives";

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

/**
 * How many thumbnails a draft shows before the rest become one count.
 *
 * Past two or three pictures the band is a wall of thumbnails that pushes the field down for
 * nothing: the count says `+3` and opens the whole set, the way a full attachment row collapses in
 * T3 Code (`getRestingComposerImagePreviewCounts`). Files keep their own chips, after them.
 */
const IMAGE_PREVIEW_LIMIT = 3;

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
  const pictures = attachments.flatMap((a) =>
    a.kind === "image" ? [{ src: `data:${a.mimeType};base64,${a.data}`, alt: a.name }] : [],
  );
  const images = attachments.filter((a) => a.kind === "image");
  const shown = images.slice(0, IMAGE_PREVIEW_LIMIT);
  const more = images.slice(IMAGE_PREVIEW_LIMIT);
  return (
    // Its own band above the field, inset from the box's rounder corner and never shrinking, so the
    // thumbnails clear the curve and only the field below scrolls.
    <div className={cx("flex shrink-0 gap-2 overflow-x-auto", className)}>
      {shown.map((a) =>
        a.kind === "image" ? (
          <div key={a.id} className="group relative shrink-0 rc-in">
            <button
              type="button"
              aria-label={`Open ${a.name}`}
              onClick={() =>
                openLightbox(
                  pictures,
                  pictures.findIndex((p) => p.alt === a.name),
                )
              }
              className="block rounded-lg"
            >
              <img
                src={`data:${a.mimeType};base64,${a.data}`}
                alt={a.name}
                title={a.name}
                className="size-16 rounded-lg object-cover shadow-1"
              />
            </button>
            {/* Inside the corner, so the strip's scroll never crops it. */}
            <button
              type="button"
              aria-label={`Remove ${a.name}`}
              onClick={() => onRemove(a.id)}
              className="rc-glass absolute top-1.5 right-1.5 flex size-[20px] items-center justify-center rounded-full text-fg opacity-0 shadow-1 hover:bg-surface-3 group-hover:opacity-100 focus-visible:opacity-100"
            >
              <X size={12} strokeWidth={2.5} />
            </button>
          </div>
        ) : null,
      )}
      {more.length > 0 && (
        // The count is a way in, not a dead label: it opens the whole set at the first one it hides.
        <button
          key="more"
          type="button"
          aria-label={`Show all ${images.length} images`}
          onClick={() => openLightbox(pictures, IMAGE_PREVIEW_LIMIT)}
          className="group relative size-16 shrink-0 rounded-lg bg-fill-strong text-callout text-fg-muted shadow-1 rc-in hover:text-fg"
        >
          +{more.length}
        </button>
      )}
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
  );
}
