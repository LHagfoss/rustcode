// The update row at the foot of the sidebar and the What's New dialog
// (state/updates.ts). The row shows only when a newer release is out; the
// dialog once, after the app updated, with that release's notes.

import { Dialog } from "radix-ui";
import { useMemo, useState } from "react";
import { openExternal } from "../lib/native";
import { parseReleaseNotes, splitPullRefs } from "../lib/releaseNotes";
import { toast, useApp } from "../state/store";
import {
  busyThreads,
  closeWhatsNew,
  installNow,
  installNowLabel,
  installProgress,
  isPrerelease,
  offersUpdate,
  requestUpdate,
  skipBeta,
  updateHint,
  useUpdates,
  type WhatsNew,
  waitingReason,
} from "../state/updates";
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
} from "./icons";
import { Markdown } from "./Markdown";
import { cx, Pill, Skeleton, Spinner } from "./primitives";

/** The repository the release pages and pull requests live in. */
const REPO = "LHagfoss/rustcode";

export function UpdateRow() {
  const info = useUpdates((s) => s.info);
  const channel = useUpdates((s) => s.channel);
  const installState = useUpdates((s) => s.install);
  const installing = installState.phase === "installing";
  const progress = installState.phase === "installing" ? installState.progress : null;
  const installError = installState.phase === "failed" ? installState.error : null;
  const waiting = installState.phase === "waiting";
  // What the wait is for, counted, so a waiting update reads as one.
  const busy = useApp((s) => (waiting ? waitingReason(busyThreads(s.threads)) : null));
  const resume = useApp((s) => s.resumeAfterRestart);
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
          void requestUpdate().then(
            (error) =>
              error &&
              toast(`The update did not install: ${error}`, "error", {
                action: { label: "Release page", run: () => info.page && openExternal(info.page) },
              }),
          )
        }
        className="flex w-full items-center gap-2 rounded-md bg-accent/12 px-2 py-2 text-left text-callout hover:bg-accent/18 disabled:opacity-70"
      >
        {installing ? <Spinner size={13} /> : <ArrowUpCircle size={16} className="shrink-0 text-accent" />}
        <span className="min-w-0 flex-1">
          <span className="block font-medium">
            {installing
              ? `Updating to ${info.latest}…`
              : installError
                ? `Retry update to ${info.latest}`
                : `Update to ${info.latest}`}
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
          <span
            className={cx(
              "block text-caption tabular",
              installError && !installing ? "text-warning" : "text-fg-muted",
            )}
          >
            {installing
              ? step
              : installError
                ? `Did not install: ${installError}`
                : waiting
                  ? `${busy}, then reopens`
                  : `${beta ? "Beta · " : ""}${updateHint(info)}`}
          </span>
        </span>
      </button>
      {waiting && (
        <button
          type="button"
          onClick={() =>
            void installNow().then(
              (error) =>
                error &&
                toast(`The update did not install: ${error}`, "error", {
                  action: { label: "Release page", run: () => info.page && openExternal(info.page) },
                }),
            )
          }
          className="mt-1 w-full rounded-chip px-2 py-1 text-left text-caption text-fg-muted hover:text-fg"
        >
          {installNowLabel(resume)}
        </button>
      )}
      {beta && !installing && (
        <button
          type="button"
          onClick={skip}
          className="mt-1 w-full rounded-chip px-2 py-1 text-left text-caption text-fg-muted hover:text-fg"
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
        <Dialog.Content className="elevated rc-whatsnew fixed overflow-hidden top-1/2 left-1/2 z-50 flex max-h-[min(640px,calc(100vh-80px))] w-[min(560px,calc(100vw-56px))] -translate-x-1/2 -translate-y-1/2 flex-col">
          <header className="relative shrink-0 overflow-hidden px-5 pt-5 pb-4">
            <ReleaseBanner />
            <span className="relative flex size-9 items-center justify-center rounded-lg bg-accent/20 text-accent shadow-1">
              <Sparkles size={16} />
            </span>
            <Dialog.Title className="relative mt-3 font-semibold text-title2">
              What's new in {shown.version}
            </Dialog.Title>
            <Dialog.Description className="relative text-sub text-fg-muted">
              RustCode updated. Here is what changed.
            </Dialog.Description>
          </header>
          <div
            aria-busy={shown.state === "loading"}
            className="selectable min-h-0 flex-1 overflow-y-auto px-5 pb-3"
          >
            <Notes whatsNew={shown} page={page} />
          </div>
          <div className="rule-t [--rc-rule-inset:1.25rem] flex items-center gap-2 px-5 py-3">
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
              <Note key={j} item={item} />
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
          <GitCompareArrows size={12} aria-hidden />
          {notes.compare.label}
        </button>
      )}
    </div>
  );
}

/** One note in the sub size, its pull requests trailing it as quiet `#N` links. */
function Note({ item }: { item: string }) {
  const { text, refs } = useMemo(() => splitPullRefs(item, REPO), [item]);
  return (
    <li className="pl-0.5">
      <Markdown text={text} className="inline! text-sub! leading-snug! [&_p]:inline [&_p]:leading-snug" />
      {refs.map((ref) => (
        <button
          key={ref.number}
          type="button"
          onClick={() => openExternal(ref.url)}
          className="ml-1.5 cursor-pointer align-baseline font-mono text-caption text-fg-faint hover:text-accent"
        >
          #{ref.number}
        </button>
      ))}
    </li>
  );
}

/**
 * The header's picture: a dot grid in the accent that fades out toward the
 * notes, over a soft accent wash, drawn in SVG so it follows the theme.
 */
function ReleaseBanner() {
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0 text-accent">
      <div className="absolute inset-0 bg-linear-to-b from-accent/18 to-transparent" />
      <svg
        className="absolute inset-0 size-full [mask-image:radial-gradient(120%_100%_at_100%_0%,black,transparent_70%)]"
        role="presentation"
      >
        <defs>
          <pattern id="rc-whatsnew-dots" width="12" height="12" patternUnits="userSpaceOnUse">
            <circle cx="2" cy="2" r="1" fill="currentColor" opacity="0.6" />
          </pattern>
        </defs>
        <rect width="100%" height="100%" fill="url(#rc-whatsnew-dots)" />
      </svg>
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
      <Icon size={12} aria-hidden />
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
