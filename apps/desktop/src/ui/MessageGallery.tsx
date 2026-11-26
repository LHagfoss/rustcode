import { useEffect, useState } from "react";
import { storedImage, storedImageNow } from "../state/attachmentImages";
import { Image as ImageIcon, ImageOff } from "./icons";
import { type LightboxImage, openChatLightbox, picturesAttr } from "./Lightbox";
import { cx } from "./primitives";

/** A picture of a message: `src` is undefined while it loads and null once its file is gone. */
export interface GalleryPicture {
  src: string | null | undefined;
  name: string;
}

/** How many tiles a message shows before the last one turns into "+N". */
export const GALLERY_TILES = 3;

/** The tiles to draw and how many pictures the last one stands for beyond itself. */
export function galleryLayout(count: number): { tiles: number; more: number } {
  const tiles = Math.min(count, GALLERY_TILES);
  return { tiles, more: count - tiles };
}

export function ImageUnavailable() {
  return (
    <span
      title="The image file is no longer on the computer this thread ran on"
      className="flex h-[26px] items-center gap-1.5 self-center rounded-full bg-fill-subtle px-2 text-callout text-fg-muted"
    >
      <ImageOff size={12} /> Image unavailable
    </span>
  );
}

/** The pictures a replayed message names, read back from the computer the thread runs on. */
export function useStoredImages(threadId: string, paths: readonly string[]): GalleryPicture[] {
  const key = paths.join("\n");
  const [urls, setUrls] = useState(() => paths.map((p) => storedImageNow(threadId, p)));
  useEffect(() => {
    const list = key ? key.split("\n") : [];
    let live = true;
    setUrls(list.map((p) => storedImageNow(threadId, p)));
    list.forEach((path, i) => {
      if (storedImageNow(threadId, path) !== undefined) return;
      void storedImage(threadId, path).then((url) => {
        if (!live) return;
        setUrls((prev) => prev.map((u, j) => (j === i ? url : u)));
      });
    });
    return () => {
      live = false;
    };
  }, [threadId, key]);
  return paths.map((path, i) => ({ src: urls[i], name: path.split(/[\\/]/).pop() || "Image" }));
}

/**
 * A message's pictures: one keeps its shape, several sit as equal square tiles, and past
 * `GALLERY_TILES` the last tile reads "+N". Every tile is 120px tall, the height
 * `estimatedRowHeight` reserves (ui/transcriptHeight.ts `THUMBNAIL_ROW`). A click opens the
 * lightbox on that picture, among every picture of the chat.
 */
export function MessageGallery({ pictures }: { pictures: readonly GalleryPicture[] }) {
  if (pictures.length === 0) return null;
  const ready: LightboxImage[] = pictures.flatMap((p) => (p.src ? [{ src: p.src, alt: p.name }] : []));
  const { tiles, more } = galleryLayout(pictures.length);
  const single = pictures.length === 1;
  return (
    <div {...picturesAttr(ready)} className="flex max-w-full flex-wrap justify-end gap-1.5">
      {pictures.slice(0, tiles).map((picture, i) => {
        const key = `${i}:${picture.name}`;
        if (picture.src === null) return <ImageUnavailable key={key} />;
        if (picture.src === undefined) {
          return (
            <span
              key={key}
              role="img"
              aria-label={`Loading ${picture.name}`}
              className={cx(
                "flex h-[120px] items-center justify-center rounded-lg bg-fill-subtle text-fg-faint",
                single ? "w-[160px]" : "w-[120px]",
              )}
            >
              <ImageIcon size={16} />
            </span>
          );
        }
        const src = picture.src;
        const overflow = i === tiles - 1 && more > 0;
        return (
          <button
            key={key}
            type="button"
            aria-label={overflow ? `Open ${picture.name} and ${more} more` : `Open ${picture.name}`}
            className="group/tile relative shrink-0 cursor-zoom-in overflow-hidden rounded-lg shadow-1"
            onClick={(e) =>
              openChatLightbox(
                e.currentTarget,
                ready,
                ready.findIndex((x) => x.src === src),
              )
            }
          >
            <img
              src={src}
              alt={picture.name}
              title={picture.name}
              draggable={false}
              className={cx(
                "h-[120px] object-cover transition-[filter] duration-150 group-hover/tile:brightness-110",
                single ? "w-auto max-w-[240px]" : "w-[120px]",
              )}
            />
            {overflow && (
              <span className="absolute inset-0 flex items-center justify-center bg-bg/75 font-semibold text-fg text-title3 tabular">
                +{more}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}
