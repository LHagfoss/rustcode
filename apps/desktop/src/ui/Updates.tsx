// The update row at the foot of the sidebar and the What's New dialog
// (state/updates.ts). The row shows only when a newer release is out; the
// dialog once, after the app updated, with that release's notes.

import { ArrowUpCircle, Sparkles } from "lucide-react";
import { Dialog } from "radix-ui";
import { openExternal } from "../lib/native";
import { toast } from "../state/store";
import {
  applyUpdate,
  closeWhatsNew,
  installProgress,
  isPrerelease,
  offersUpdate,
  skipBeta,
  useUpdates,
} from "../state/updates";
import { Markdown } from "./Markdown";
import { cx, Pill, Spinner } from "./primitives";

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
        className="flex w-full items-center gap-2 rounded-md bg-accent/12 px-2.5 py-2 text-left text-callout hover:bg-accent/18 disabled:opacity-70"
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
          className="mt-1 w-full rounded px-2.5 py-1 text-left text-caption text-fg-muted hover:text-fg"
        >
          Skip this beta
        </button>
      )}
    </div>
  );
}

export function WhatsNewDialog() {
  const whatsNew = useUpdates((s) => s.whatsNew);
  if (!whatsNew) return null;
  const page = `https://github.com/LHagfoss/rustcode/releases/tag/v${whatsNew.version}`;
  return (
    <Dialog.Root open onOpenChange={(open) => !open && closeWhatsNew()}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <Dialog.Content className="elevated fixed top-1/2 left-1/2 z-50 flex max-h-[min(640px,calc(100vh-80px))] w-[min(560px,calc(100vw-56px))] -translate-x-1/2 -translate-y-1/2 flex-col rc-in">
          <div className="flex items-center gap-3 px-5 pt-5 pb-3">
            <span className="flex size-9 items-center justify-center rounded-lg bg-accent/15 text-accent">
              <Sparkles size={17} />
            </span>
            <div className="min-w-0 flex-1">
              <Dialog.Title className="font-semibold text-title3">
                What's new in {whatsNew.version}
              </Dialog.Title>
              <Dialog.Description className="text-callout text-fg-muted">
                RustCode updated. Here is what changed.
              </Dialog.Description>
            </div>
          </div>
          <div className="selectable min-h-0 flex-1 overflow-y-auto px-5 pb-2">
            {whatsNew.notes ? (
              <Markdown text={whatsNew.notes} className="gap-2 text-callout" />
            ) : (
              <p className="text-callout text-fg-muted">
                The notes for this release are on its release page.
              </p>
            )}
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
