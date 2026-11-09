// The overview's Deliverables section: what the thread produced, as pictures a
// reader can open and files they can show in the file manager (ui/ThreadDetails.tsx
// draws the sections, ui/overview.ts finds them). A tile grid by default, a list
// when the reader asks for one, so a thread with eight screenshots stays short.

import { FileText, Image as ImageIcon, LayoutGrid, List } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";
import { revealPath } from "../lib/native";
import type { TranscriptItem } from "../state/thread";
import { MarkdownImage } from "./MarkdownMedia";
import { OverviewSection, SectionButton } from "./OverviewSection";
import {
  absolutePath,
  type DeliverableFile,
  type DeliverableMode,
  type DeliverablePicture,
  deliverables,
  readDeliverableMode,
  writeDeliverableMode,
} from "./overview";
import { cx, Pill } from "./primitives";
import { DiffCount } from "./Rows";

/** How many deliveries show before the rest wait behind one press. */
export const DELIVERABLE_ROWS = 9;

/** A picture, cropped to its tile: the reply's own picture, so a click opens the lightbox. */
const PICTURE_FILL =
  "[&_button]:block [&_button]:size-full [&_img]:size-full [&_img]:max-h-none [&_img]:object-cover";

function PictureTile({ picture }: { picture: DeliverablePicture }) {
  return (
    <span
      title={picture.label}
      className={cx("block size-full overflow-hidden rounded-md bg-fill-subtle", PICTURE_FILL)}
    >
      <MarkdownImage src={picture.src} alt={picture.label} />
    </span>
  );
}

function FileTile({ file, onOpen }: { file: DeliverableFile; onOpen: () => void }) {
  // The tile is the surface and the button is only its hover, the way a picture's is:
  // a file and a screenshot sit in the same quiet square.
  return (
    <div className="aspect-square rounded-md bg-fill-subtle">
      <button
        type="button"
        onClick={onOpen}
        title={`${file.path} — show in the file manager`}
        className="flex size-full flex-col items-center justify-center gap-1 rounded-md px-1 py-1.5 text-fg-muted hover:bg-fill hover:text-fg"
      >
        <FileText size={16} className="shrink-0" />
        <span className="line-clamp-2 w-full break-all text-center text-caption">{file.label}</span>
      </button>
    </div>
  );
}

/** A delivery as one row: its thumbnail or mark, its name, and what it is on the right. */
function DeliverableRow({
  kind,
  label,
  meta,
  onOpen,
  children,
}: {
  kind: "picture" | "file";
  label: string;
  meta?: ReactNode;
  onOpen?: () => void;
  children?: ReactNode;
}) {
  const body = (
    <>
      <span className="flex size-6 shrink-0 items-center justify-center overflow-hidden rounded-xs bg-fill-subtle">
        {children ?? <FileText size={12} className="text-fg-muted" />}
      </span>
      <span className="min-w-0 flex-1 truncate text-body">{label}</span>
      {meta}
    </>
  );
  const shape = "flex h-8 w-full items-center gap-2 rounded-md px-2 text-body";
  if (!onOpen) {
    return (
      <div className={shape} title={label}>
        {body}
      </div>
    );
  }
  return (
    <button
      type="button"
      onClick={onOpen}
      title={kind === "picture" ? `Open ${label}` : `${label} — show in the file manager`}
      className={cx(shape, "hover:bg-fill-subtle")}
    >
      {body}
    </button>
  );
}

export function DeliverablesSection({
  items,
  cwd,
  folded,
  onToggle,
}: {
  items: readonly TranscriptItem[];
  /** The thread's folder, so a diff's own path can be shown from it. */
  cwd: string;
  folded: boolean;
  onToggle: () => void;
}) {
  const { pictures, files } = useMemo(() => deliverables(items), [items]);
  const [mode, setMode] = useState<DeliverableMode>(readDeliverableMode);
  const [all, setAll] = useState(false);
  const total = pictures.length + files.length;
  const shownPictures = all ? pictures : pictures.slice(0, DELIVERABLE_ROWS);
  const shownFiles = all ? files : files.slice(0, Math.max(0, DELIVERABLE_ROWS - shownPictures.length));
  const hidden = total - shownPictures.length - shownFiles.length;

  const pick = (next: DeliverableMode) => {
    setMode(next);
    writeDeliverableMode(next);
  };

  return (
    <OverviewSection
      id="deliverables"
      title="Deliverables"
      count={total}
      actions={
        <SectionButton
          label={mode === "grid" ? "List the deliverables" : "Show the deliverables as a grid"}
          onClick={() => pick(mode === "grid" ? "list" : "grid")}
        >
          {mode === "grid" ? <List size={13} /> : <LayoutGrid size={13} />}
        </SectionButton>
      }
      folded={folded}
      onToggle={onToggle}
    >
      {mode === "grid" ? (
        <div className="grid grid-cols-3 gap-1.5 px-2">
          {shownPictures.map((picture) => (
            <div key={picture.key} className="aspect-square">
              <PictureTile picture={picture} />
            </div>
          ))}
          {shownFiles.map((file) => (
            <FileTile key={file.key} file={file} onOpen={() => revealPath(absolutePath(cwd, file.path))} />
          ))}
        </div>
      ) : (
        <>
          {shownPictures.map((picture) => (
            <DeliverableRow
              key={picture.key}
              kind="picture"
              label={picture.label}
              meta={<ImageIcon size={12} className="shrink-0 text-fg-faint" />}
            >
              <PictureTile picture={picture} />
            </DeliverableRow>
          ))}
          {shownFiles.map((file) => (
            <DeliverableRow
              key={file.key}
              kind="file"
              label={file.label}
              onOpen={() => revealPath(absolutePath(cwd, file.path))}
              meta={<DiffCount added={file.added} removed={file.removed} />}
            />
          ))}
        </>
      )}
      {hidden > 0 && (
        <Pill quiet size="sm" className="mt-1.5 ml-2" onClick={() => setAll(true)}>
          Show {hidden} more
        </Pill>
      )}
      {total === 0 && (
        <p className="px-2 py-1 text-callout text-fg-muted">
          Nothing yet: the pictures its replies show and the files it writes land here.
        </p>
      )}
    </OverviewSection>
  );
}
