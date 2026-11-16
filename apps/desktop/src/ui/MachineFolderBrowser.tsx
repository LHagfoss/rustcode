// Browse Folders… for a paired machine (the project picker's last row there). That machine's disk
// is not this app's to open in a file dialog, and a fresh server has no project for the picker to
// list, so this walks its folders one at a time through its engine (state/machineFolders.ts):
// open a folder to go in, Up to go out, New Project to make a folder there with `git init`, and
// Open to start the next thread in (or move the open one to) the folder shown.

import { ArrowUp, Folder, FolderGit2, FolderPlus, House } from "lucide-react";
import { Dialog } from "radix-ui";
import { useCallback, useEffect, useRef, useState } from "react";
import { projectName } from "../acp/types";
import {
  browseFolders,
  createProjectFolder,
  type FolderListing,
  folderNameProblem,
} from "../state/machineFolders";
import { inputClass, noTextHelpers, Pill, Spinner, Tooltip } from "./primitives";

export function MachineFolderBrowser({
  open,
  onOpenChange,
  host,
  machine,
  start,
  action,
  onPick,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  host: string;
  machine: string;
  /** Where the dialog opens: the thread's folder, else the machine's home. */
  start?: string;
  /** The primary button's verb: "Open" for a new thread, "Move Here" for an open one. */
  action: string;
  onPick: (path: string) => void;
}) {
  const [listing, setListing] = useState<FolderListing | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [naming, setNaming] = useState(false);
  const [name, setName] = useState("");
  const [creating, setCreating] = useState(false);
  // A slow answer for a folder already left is dropped, not shown over the one asked after it.
  const asked = useRef(0);

  // `orHome`: a thread's folder that is gone (a deleted worktree) opens home instead of nothing.
  const go = useCallback(
    async (path?: string, orHome = false) => {
      const ask = ++asked.current;
      setLoading(true);
      setError("");
      try {
        const next = await browseFolders(host, machine, path);
        if (ask === asked.current) setListing(next);
      } catch (failure) {
        if (ask !== asked.current) return;
        if (path && orHome) return void go(undefined);
        setError(failure instanceof Error ? failure.message : String(failure));
      } finally {
        if (ask === asked.current) setLoading(false);
      }
    },
    [host, machine],
  );

  useEffect(() => {
    if (!open) return;
    setListing(null);
    setNaming(false);
    setName("");
    void go(start && start !== "~" ? start : undefined, true);
  }, [open, start, go]);

  const problem = naming ? folderNameProblem(name) : null;
  const create = async () => {
    if (!listing || problem) return;
    setCreating(true);
    setError("");
    try {
      const made = await createProjectFolder(host, machine, listing.path, name);
      onPick(made);
      onOpenChange(false);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setCreating(false);
    }
  };

  const here = listing?.path ?? "";
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
          <Dialog.Content
            aria-describedby={undefined}
            className="elevated pointer-events-auto flex max-h-[min(560px,calc(100%-48px))] w-[min(460px,calc(100vw-48px))] flex-col gap-3 p-5"
          >
            <Dialog.Title className="font-semibold text-body">Folders on {machine}</Dialog.Title>
            <div className="flex items-center gap-1.5">
              <Tooltip label="Enclosing folder">
                <Pill
                  quiet
                  size="sm"
                  aria-label="Enclosing folder"
                  disabled={!listing?.parent || loading}
                  onClick={() => listing?.parent && void go(listing.parent)}
                >
                  <ArrowUp size={13} />
                </Pill>
              </Tooltip>
              <Tooltip label="Home folder">
                <Pill
                  quiet
                  size="sm"
                  aria-label="Home folder"
                  disabled={loading || (!!listing?.home && listing.home === here)}
                  onClick={() => void go()}
                >
                  <House size={13} />
                </Pill>
              </Tooltip>
              <span className="min-w-0 flex-1 truncate text-caption text-fg-muted" title={here}>
                {here || "…"}
              </span>
              {loading && <Spinner />}
            </div>
            <div
              role="group"
              aria-label={`Folders in ${here ? projectName(here) : machine}`}
              className="scroll-stable -mx-2 min-h-[180px] flex-1 overflow-y-auto"
            >
              {listing?.folders.map((folder) => (
                <button
                  key={folder.path}
                  type="button"
                  onClick={() => void go(folder.path)}
                  className="flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-body text-fg hover:bg-fill focus-visible:bg-fill"
                >
                  {folder.isGit ? (
                    <FolderGit2 size={14} className="shrink-0 text-fg-muted" />
                  ) : (
                    <Folder size={14} className="shrink-0 text-fg-faint" />
                  )}
                  <span className="min-w-0 flex-1 truncate">{folder.name}</span>
                </button>
              ))}
              {listing && !listing.folders.length && !loading && (
                <p className="px-2 py-2 text-callout text-fg-muted">No folders in here</p>
              )}
              {listing?.truncated && (
                <p className="px-2 py-2 text-caption text-fg-faint">Showing the first 500 folders</p>
              )}
            </div>
            {naming && (
              <form
                className="flex items-center gap-2"
                onSubmit={(event) => {
                  event.preventDefault();
                  void create();
                }}
              >
                <input
                  {...noTextHelpers}
                  autoFocus
                  aria-label="New project folder name"
                  placeholder="New project name"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  onKeyDown={(event) => {
                    // Escape leaves the name, not the dialog.
                    if (event.key === "Escape") {
                      event.stopPropagation();
                      setNaming(false);
                    }
                  }}
                  className={`${inputClass} min-w-0 flex-1`}
                />
                <Pill type="submit" prominent disabled={!!problem || creating || !listing}>
                  {creating ? <Spinner /> : null} Create
                </Pill>
              </form>
            )}
            {naming && name && problem && <p className="text-caption text-fg-muted">{problem}</p>}
            {naming && !problem && here && (
              <p className="text-caption text-fg-faint">
                Makes {here.replace(/[\\/]+$/, "")}/{name.trim()} with git init, then opens it
              </p>
            )}
            {error && (
              <p role="alert" className="text-callout text-danger">
                {error}
              </p>
            )}
            <div className="flex items-center gap-2">
              {!naming && (
                <Pill quiet disabled={!listing} onClick={() => setNaming(true)}>
                  <FolderPlus size={13} /> New Project
                </Pill>
              )}
              <span className="flex-1" />
              <Pill onClick={() => onOpenChange(false)}>Cancel</Pill>
              <Pill
                prominent
                disabled={!listing || loading}
                onClick={() => {
                  if (!here) return;
                  onPick(here);
                  onOpenChange(false);
                }}
              >
                {action}
                {here ? ` “${projectName(here)}”` : ""}
              </Pill>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
