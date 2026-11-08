// The update row at the foot of the sidebar and the What's New dialog
// (state/updates.ts). The row shows only when a newer release is out; the
// dialog once, after the app updated, with that release's notes.

import {
  ArrowUpCircle,
  BookOpen,
  Bug,
  FlaskConical,
  Gauge,
  GitCompareArrows,
  Hammer,
  type LucideIcon,
  Paintbrush,
  Sparkles,
  Tag,
  TriangleAlert,
  Wrench,
} from "lucide-react";
import { Dialog } from "radix-ui";
import { useMemo, useState } from "react";
import { openExternal } from "../lib/native";
import { parseReleaseNotes } from "../lib/releaseNotes";
import { toast } from "../state/store";
import {
  applyUpdate,
  closeWhatsNew,
  installProgress,
  isPrerelease,
  offersUpdate,
  skipBeta,
  useUpdates,
  type WhatsNew,
} from "../state/updates";
import { Markdown } from "./Markdown";
import { cx, Pill, Skeleton, Spinner } from "./primitives";

/** The repository the release pages and pull requests live in. */
const REPO = "LHagfoss/rustcode";

export function UpdateRow() {
  const info = useUpdates((s) => s.info);
  const channel = useUpdates((s) => s.channel);
  const installing = useUpdates((s) => s.installing);
  const progress = useUpdates((s) => s.progress);
  // The offer is the selected channel's, and a dev build never shows the row.
  if (!info?.latest || info.development || !offersUpdate(info, channel)) return null;
  const beta = isPrerelease(info.latest);
  const { fraction, step } = installProgress(progress);
  const skip = () => {
    const version = skipBeta();
    if (version) toast(`Skipped ${version}. The next beta shows here.`);
  };
  return (
    <div className="shrink-0 px-2 pb-2">
      <button
        type="button"
        disabled={installing}
        onClick={() =>
          void applyUpdate().then((error) => error && toast(`The update did not install: ${error}`, "error"))
        }
        className="flex w-full items-center gap-2 rounded-md bg-accent/12 px-2 py-2 text-left text-callout hover:bg-accent/18 disabled:opacity-70"
      >
        {installing ? <Spinner size={13} /> : <ArrowUpCircle size={15} className="shrink-0 text-accent" />}
        <span className="min-w-0 flex-1">
          <span className="block font-medium">
            {installing ? `Updating to ${info.latest}…` : `Update to ${info.latest}`}
          </span>
          {installing && (
            <span
              role="progressbar"
              aria-label="Update progress"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={fraction === undefined ? undefined : Math.round(fraction * 100)}
              className="my-1 block h-[4px] overflow-hidden rounded-full bg-fill-strong"
            >
              <span
                className={cx(
                  "block h-full rounded-full bg-accent transition-[width] duration-200 ease-snappy",
                  fraction === undefined && "w-1/3 animate-pulse",
                )}
                style={fraction === undefined ? undefined : { width: `max(4px, ${fraction * 100}%)` }}
              />
            </span>
          )}
          <span className="block text-caption text-fg-muted tabular">
            {installing
              ? step
              : `${beta ? "Beta · " : ""}${info.canInstall ? "Installs and reopens RustCode" : "Opens the release page"}`}
          </span>
        </span>
      </button>
      {beta && !installing && (
        <button
          type="button"
          onClick={skip}
          className="mt-1 w-full rounded px-2 py-1 text-left text-caption text-fg-muted hover:text-fg"
        >
          Skip this beta
        </button>
      )}
    </div>
  );
}

export function WhatsNewDialog() {
  const whatsNew = useUpdates((s) => s.whatsNew);
  // The card keeps its version and its notes while it scales away: closing clears `whatsNew` in
  // the store, and the exit is drawn with the last answer rather than an empty dialog.
  const [shown, setShown] = useState<WhatsNew | null>(whatsNew);
  if (whatsNew && whatsNew !== shown) setShown(whatsNew);
  if (!shown) return null;
  const page = `https://github.com/${REPO}/releases/tag/v${shown.version}`;
  return (
    <Dialog.Root open={whatsNew !== null} onOpenChange={(open) => !open && closeWhatsNew()}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        {/* The dialog rises and scales in through `rc-whatsnew` (index.css), 200ms, and leaves the
            way it came; the class carries both, so the close holds until it has played. */}
        <Dialog.Content className="elevated rc-whatsnew fixed top-1/2 left-1/2 z-50 flex max-h-[min(640px,calc(100vh-80px))] w-[min(560px,calc(100vw-56px))] -translate-x-1/2 -translate-y-1/2 flex-col">
          <div className="flex items-center gap-3 px-5 pt-5 pb-3">
            <span className="flex size-9 items-center justify-center rounded-lg bg-accent/15 text-accent">
              <Sparkles size={17} />
            </span>
            <div className="min-w-0 flex-1">
              <Dialog.Title className="font-semibold text-title3">What's new in {shown.version}</Dialog.Title>
              <Dialog.Description className="text-callout text-fg-muted">
                RustCode updated. Here is what changed.
              </Dialog.Description>
            </div>
          </div>
          <div
            aria-busy={shown.state === "loading"}
            className="selectable min-h-0 flex-1 overflow-y-auto px-5 pb-3"
          >
            <Notes whatsNew={shown} page={page} />
          </div>
          <div className="flex items-center gap-2 border-border border-t px-5 py-3">
            <Pill onClick={() => openExternal(page)}>Release page</Pill>
            <span className="flex-1" />
            <Pill prominent onClick={closeWhatsNew}>
              Continue
            </Pill>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * The dialog's body: the notes' own shape while the shell looks them up, then
 * the notes as one badge per category (lib/releaseNotes.ts), so a release reads
 * as what changed rather than as GitHub's generated pull request list.
 */
function Notes({ whatsNew, page }: { whatsNew: WhatsNew; page: string }) {
  // One parse per answer: a re-render (the pointer moving over the dialog) keeps it.
  const notes = useMemo(
    () => (whatsNew.notes ? parseReleaseNotes(whatsNew.notes, REPO) : null),
    [whatsNew.notes],
  );
  if (whatsNew.state === "loading") return <NotesSkeleton />;
  if (whatsNew.state === "error")
    return (
      <p className="rc-in text-callout text-fg-muted">
        The notes could not be loaded. They are on the release page.
      </p>
    );
  if (!notes?.groups.length)
    return (
      <p className="rc-in text-callout text-fg-muted">The notes for this release are on its release page.</p>
    );
  return (
    <div className="rc-in flex flex-col gap-4">
      {notes.groups.map((group, i) => (
        <section key={`${group.heading}-${i}`} className="flex flex-col gap-2">
          {group.heading && <CategoryHeading name={group.heading} />}
          <ul className="ml-[1.1em] list-disc space-y-2 marker:text-fg-faint">
            {group.items.map((item, j) => (
              <li key={j} className="pl-0.5">
                <Markdown text={item} />
              </li>
            ))}
          </ul>
        </section>
      ))}
      {notes.compare && (
        <button
          type="button"
          onClick={() => openExternal(notes.compare?.url ?? page)}
          className="inline-flex w-fit cursor-pointer items-center gap-1 text-caption text-fg-faint hover:text-fg-muted"
        >
          <GitCompareArrows size={11} aria-hidden />
          {notes.compare.label}
        </button>
      )}
    </div>
  );
}

/** The notes on their way: five lines of the shape they will take, sweeping once. */
function NotesSkeleton() {
  return (
    <div role="status" aria-label="Loading the release notes" className="flex flex-col gap-3">
      {[76, 92, 64, 86, 58].map((width, i) => (
        <Skeleton key={i} className="h-3" style={{ width: `${width}%` }} />
      ))}
    </div>
  );
}

/**
 * A category as a small tinted badge, so Features, Fixes and Breaking changes
 * are told apart at a glance. The tone marks the kind of change; a category the
 * app does not know (a release's own heading) stays on the quiet fill.
 */
function CategoryHeading({ name }: { name: string }) {
  const { icon: Icon, tone } = category(name);
  return (
    <h3
      className={cx(
        "inline-flex w-fit items-center gap-1 rounded-full px-1.5 py-0.5 font-medium text-caption",
        tone,
      )}
    >
      <Icon size={11} aria-hidden />
      {name}
    </h3>
  );
}

const CATEGORIES: [pattern: RegExp, icon: LucideIcon, tone: string][] = [
  [/^feature/i, Sparkles, "bg-accent/12 text-accent"],
  [/^fix|^bug/i, Bug, "bg-success/12 text-success"],
  [/^performance/i, Gauge, "bg-warning/12 text-warning"],
  [/^breaking/i, TriangleAlert, "bg-danger/12 text-danger"],
  [/^refactor/i, Wrench, "bg-fill text-fg-muted"],
  [/^ui|^design/i, Paintbrush, "bg-fill text-fg-muted"],
  [/^doc/i, BookOpen, "bg-fill text-fg-muted"],
  [/^test/i, FlaskConical, "bg-fill text-fg-muted"],
  [/^build|^ci/i, Hammer, "bg-fill text-fg-muted"],
];

function category(name: string): { icon: LucideIcon; tone: string } {
  const hit = CATEGORIES.find(([pattern]) => pattern.test(name));
  return hit ? { icon: hit[1], tone: hit[2] } : { icon: Tag, tone: "bg-fill text-fg-muted" };
}
