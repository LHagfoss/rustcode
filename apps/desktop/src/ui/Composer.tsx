// The composer, after ContentView.swift's Composer: one floating rounded-xs
// box with the queue, attachments, the field (with a slash-command token in
// front of it; pasted links stay inline as text) and a button row: the folder, Files, the
// harness, the model pill and send/queue/stop. Keys follow the SwiftUI app:
// ⏎ send (or queue while running), ⇧⏎ newline, ⌘⏎ steer, Esc closes
// suggestions, then clears the draft, then stops the turn.

import {
  ArrowUp,
  ChevronDown,
  ChevronUp,
  FileText,
  Folder,
  GitBranch,
  ListEnd,
  Paperclip,
  Pencil,
  Plus,
  Shield,
  ShieldCheck,
  ShieldOff,
  Square,
  SquarePen,
  X,
} from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { projectName, type SlashCommand } from "../acp/types";
import { suggestBranch, validBranch } from "../acp/v2";
import { haptic } from "../lib/haptics";
import { onFileDrop, pickFiles, pickFolder, readImage } from "../lib/native";
import {
  type Attachment,
  type ComposeExtras,
  emptyExtras,
  hasExtras,
  IMAGE_TYPES,
  imageMime,
  MAX_IMAGE_BYTES,
  MAX_INLINE_FILE_BYTES,
  splitUrls,
} from "../state/compose";
import { controlDisabledReason } from "../state/harness";
import { PERMISSION_MODES, type PermissionMode } from "../state/permissions";
import {
  acceptsImages,
  cancel,
  cancelEdit,
  canSteer,
  featureMissing,
  moveQueued,
  permissionModeOf,
  removeQueued,
  sendQueuedNow,
  setDraft,
  setExtras,
  setNewWorktree,
  setPermissionMode,
  setProjectCwd,
  startNewThread,
  steerOrSubmit,
  submit,
  takeQueued,
  threadHarness,
  toast,
  useApp,
} from "../state/store";
import { localId, type QueuedPrompt, type ThreadState } from "../state/thread";
import { AgentPill } from "./AgentViews";
import { ContextRing, PlanMeter } from "./ComposerUsage";
import { HarnessIcon } from "./HarnessIcon";
import { openLightbox } from "./Lightbox";
import { LinkChip } from "./LinkChip";
import { ModelPicker } from "./ModelPicker";
import { cx, Tooltip } from "./primitives";

function suggestions(draft: string, commands: SlashCommand[]): SlashCommand[] {
  if (!draft.startsWith("/") || /\s/.test(draft)) return [];
  const typed = draft.toLowerCase();
  const prefixed = commands.filter((c) => c.name.toLowerCase().startsWith(typed));
  const others =
    typed.length > 1
      ? commands.filter((c) => !prefixed.includes(c) && c.name.toLowerCase().includes(typed.slice(1)))
      : [];
  return [...prefixed, ...others].slice(0, 40);
}

const LINE = 19;

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

/**
 * Whether a popover, menu or dialog is open over the app. Radix closes those
 * on Escape from a document listener, after the field's own handler has run,
 * so the composer checks first. Tooltips do not count.
 */
function overlayOpen(): boolean {
  return !!document.querySelector(
    '[data-radix-popper-content-wrapper]:not(:has([role="tooltip"])), [role="dialog"], [role="alertdialog"]',
  );
}

/** Browser files (paste, drop, the file input) as attachments. */
async function attachmentsFromFiles(
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
async function attachmentsFromPaths(
  paths: string[],
  images: boolean,
  harnessName: string,
): Promise<Attachment[]> {
  const out: Attachment[] = [];
  for (const path of paths) {
    const name = path.split("/").pop() ?? path;
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

/** Re-render for what the composer shows, not for every streamed chunk (`items` is read live). */
function sameComposer(
  a: { thread?: ThreadState; compact?: boolean },
  b: { thread?: ThreadState; compact?: boolean },
): boolean {
  if (a.compact !== b.compact) return false;
  if (a.thread === b.thread) return true;
  if (!a.thread || !b.thread) return false;
  for (const key of Object.keys(b.thread) as (keyof ThreadState)[]) {
    if (key !== "items" && key !== "toolIndex" && key !== "usage" && a.thread[key] !== b.thread[key])
      return false;
  }
  return Object.keys(a.thread).length === Object.keys(b.thread).length;
}

export const Composer = memo(function Composer({
  thread,
  compact,
}: {
  thread?: ThreadState;
  compact?: boolean;
}) {
  const key = thread?.id ?? "new";
  const draft = useApp((s) => s.drafts[key] ?? "");
  const extras = useApp((s) => s.extras[key] ?? emptyExtras);
  const editing = useApp((s) => s.editing[key]);
  const commands = useApp((s) => s.commands);
  const projectCwd = useApp((s) => s.projectCwd);
  useApp((s) => s.harnesses); // re-render when harness capabilities arrive
  const status = useApp((s) => s.status);
  const field = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const [selected, setSelected] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [dropping, setDropping] = useState(false);

  const running = thread?.running ?? false;
  const steerable = thread ? canSteer(thread) : false;
  const list = useMemo(
    () => (dismissed === draft || extras.command ? [] : suggestions(draft, commands)),
    [draft, commands, dismissed, extras.command],
  );
  const harness = threadHarness(thread);
  const defaultHarness = useApp((s) => s.harnesses?.default.harness ?? "rustcode");
  const harnessName = harness?.name ?? "This harness";
  const images = acceptsImages(thread);
  const cancelReason = controlDisabledReason("cancel", thread?.harness?.capabilities, harnessName);
  const cwd = thread?.cwd ?? projectCwd;
  const canSend = !!draft.trim() || hasExtras(extras);
  const mirror = useRef<HTMLDivElement>(null);
  const hasUrl = useMemo(() => /\bhttps?:\/\//i.test(draft), [draft]);

  // Ready to type on entering a thread, and again on coming back to the chat from Settings or
  // another view (the draft is kept either way).
  const inChat = useApp((s) => !s.settings && s.view.kind === "chat" && !s.switcherOpen);
  // biome-ignore lint/correctness/useExhaustiveDependencies: refocus on thread change
  useEffect(() => {
    if (inChat) field.current?.focus();
  }, [key, inChat]);

  // Grow from 1 to 10 lines, then scroll.
  // biome-ignore lint/correctness/useExhaustiveDependencies: measure on every draft change
  useLayoutEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(Math.max(el.scrollHeight, LINE + 4), LINE * 10)}px`;
  }, [draft]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new suggestion list starts at the top
  useEffect(() => setSelected(0), [list.length]);

  const update = (text: string) => setDraft(key, text);
  const change = (fn: (e: ComposeExtras) => ComposeExtras) => setExtras(key, fn);
  /**
   * Take a queued message back into the composer (↑, or Edit on its row), whole: its text, command,
   * links and attachments. A composer that already has something in it is left alone.
   */
  const takeBack = (q: QueuedPrompt) => {
    if (draft || hasExtras(extras)) {
      toast("Clear the message box first: it already has something in it");
      field.current?.focus();
      return;
    }
    const taken = takeQueued(thread?.id ?? "", q.id);
    if (!taken) {
      toast("This message answers another thread, so it stays queued");
      return;
    }
    update(taken.text);
    if (taken.extras) change(() => taken.extras!);
    field.current?.focus();
  };
  const attach = (more: Attachment[]) => {
    if (more.length) change((e) => ({ ...e, attachments: [...e.attachments, ...more] }));
  };

  // Files dropped on the app window arrive as paths from the shell.
  const dropped = useRef<(paths: string[]) => void>(() => undefined);
  dropped.current = (paths) => void attachmentsFromPaths(paths, images, harnessName).then(attach);
  useEffect(() => {
    let stop: (() => void) | undefined;
    let live = true;
    void onFileDrop((event) => {
      setDropping(event.type === "over");
      if (event.type === "drop" && event.paths.length) dropped.current(event.paths);
    }).then((unlisten) => {
      if (live) stop = unlisten;
      else unlisten();
    });
    return () => {
      live = false;
      stop?.();
    };
  }, []);

  const send = (steer: boolean) => {
    haptic("generic");
    const text = draft;
    const sentExtras = extras;
    if (!text.trim() && !hasExtras(sentExtras) && !(steer && running)) return;
    update("");
    change(() => emptyExtras);
    const sending = steer ? steerOrSubmit(text, sentExtras) : submit(text, sentExtras);
    void sending.then((sent) => {
      // A thread that could not start keeps the message for another try.
      if (!sent && !useApp.getState().drafts[key]) {
        update(text);
        change(() => sentExtras);
      }
    });
  };

  const complete = (command: SlashCommand, run: boolean) => {
    // A command the app answers itself (a picker, a new thread) runs at once, arguments or not.
    if (run && (!command.takesArguments || command.action !== "execute")) {
      update("");
      void submit(command.name);
    } else {
      // A command that wants arguments becomes the token in front of the field.
      change((e) => ({ ...e, command }));
      update("");
    }
  };

  const onChange = (text: string) => {
    // `/name ` typed out becomes the token too, as in the SwiftUI app.
    const typed = /^(\/\S+) (.*)$/s.exec(text);
    const command = typed && !extras.command ? commands.find((c) => c.name === typed[1]) : undefined;
    if (command?.takesArguments) {
      change((e) => ({ ...e, command }));
      update(typed![2] ?? "");
    } else {
      update(text);
    }
    setDismissed(null);
  };

  const onPaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const files = [...e.clipboardData.files];
    if (files.length) {
      e.preventDefault();
      void attachmentsFromFiles(files, images, harnessName).then(attach);
      return;
    }
    // A pasted link stays where it was pasted, as text; the sent message shows it as a chip.
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (list.length) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setSelected((i) => (i + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && !e.metaKey)) {
        e.preventDefault();
        const command = list[selected];
        if (command) complete(command, e.key === "Enter");
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissed(draft);
        return;
      }
    }
    const el = e.currentTarget;
    if (e.key === "Backspace" && el.selectionStart === 0 && el.selectionEnd === 0) {
      // ⌫ at the start takes back the last chip: a link, then the command (as text).
      if (extras.links.length) {
        e.preventDefault();
        change((x) => ({ ...x, links: x.links.slice(0, -1) }));
        return;
      }
      if (extras.command) {
        e.preventDefault();
        const name = extras.command.name;
        change((x) => ({ ...x, command: undefined }));
        update(`${name}${draft ? ` ${draft}` : ""}`);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey && !e.altKey) {
      e.preventDefault();
      send(e.metaKey || e.ctrlKey);
      return;
    }
    if (e.key === "Escape") {
      // The topmost thing goes first: an open popover, menu or dialog takes this Escape for
      // itself, and the draft and the running turn stay as they are (issue 2804).
      if (overlayOpen()) return;
      e.preventDefault();
      if (draft || hasExtras(extras)) {
        update("");
        change(() => emptyExtras);
      } else if (running) void cancel(thread?.id);
      return;
    }
    if (e.key === "ArrowUp" && !draft && !hasExtras(extras) && thread) {
      // The latest queued message comes back first; with nothing queued, the last sent one. One
      // that answers another thread stays queued, so the one before it comes back instead.
      const live = useApp.getState().threads[thread.id] ?? thread;
      const queued = [...live.queue].reverse().find((q) => !q.link);
      if (queued) {
        e.preventDefault();
        takeBack(queued);
        return;
      }
      const items = live.items ?? thread.items;
      const last = [...items].reverse().find((i) => i.kind === "user");
      if (last && last.kind === "user") {
        e.preventDefault();
        update(last.text);
      }
    }
  };

  const chooseFiles = () => {
    void pickFiles(cwd).then((paths) => {
      if (paths === null) fileInput.current?.click();
      else void attachmentsFromPaths(paths, images, harnessName).then(attach);
    });
  };

  const placeholder = extras.command
    ? extras.command.description || "Arguments…"
    : running
      ? steerable
        ? "⏎ queues for the next turn, ⌘⏎ steers this one…"
        : "Queue a message…"
      : "Message RustCode…";

  return (
    <div className="relative">
      {list.length > 0 && (
        <div
          className="elevated absolute right-0 bottom-full left-0 z-20 mb-2 max-h-[280px] overflow-y-auto rounded-xl p-1.5 rc-in"
          role="listbox"
          aria-label="Commands"
        >
          {list.map((c, i) => (
            <button
              key={c.name}
              type="button"
              role="option"
              aria-selected={i === selected}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => complete(c, true)}
              onMouseEnter={() => setSelected(i)}
              className={cx(
                "flex w-full items-baseline gap-3 rounded-lg px-3 py-[7px] text-left",
                i === selected && "bg-fill",
              )}
            >
              <span className="min-w-[110px] font-semibold text-accent text-callout">{c.name}</span>
              <span className="truncate text-callout text-fg-muted">{c.description}</span>
              {c.takesArguments && <span className="ml-auto shrink-0 text-caption text-fg-faint">…</span>}
            </button>
          ))}
        </div>
      )}
      {thread && thread.queue.length > 0 && (
        <QueueStack thread={thread} steerable={steerable} onTakeBack={(q) => takeBack(q)} />
      )}
      <div
        className={cx(
          // Frosted and floating over the transcript; focus steps the surface, no ring or colour.
          "rc-composer relative z-10 flex flex-col gap-1.5 rounded-2xl p-2.5",
          dropping && "outline-2 outline-accent outline-dashed",
        )}
        onMouseDown={(e) => {
          if (e.target === e.currentTarget) {
            e.preventDefault();
            field.current?.focus();
          }
        }}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes("Files")) {
            e.preventDefault();
            setDropping(true);
          }
        }}
        onDragLeave={() => setDropping(false)}
        onDrop={(e) => {
          if (!e.dataTransfer.files.length) return;
          e.preventDefault();
          setDropping(false);
          void attachmentsFromFiles([...e.dataTransfer.files], images, harnessName).then(attach);
        }}
      >
        {editing && (
          <div className="flex items-center gap-1.5 px-2 pt-1 text-caption text-accent">
            <Pencil size={11} />
            <b>Editing a sent message</b>
            <span className="text-fg-muted">· sending takes the thread back to it</span>
            <button
              type="button"
              onClick={() => {
                cancelEdit(key);
                update("");
              }}
              className="ml-auto rounded-full px-2 text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
            >
              Cancel
            </button>
          </div>
        )}
        {extras.attachments.length > 0 && (
          <AttachmentStrip
            attachments={extras.attachments}
            onRemove={(id) =>
              change((e) => ({ ...e, attachments: e.attachments.filter((a) => a.id !== id) }))
            }
          />
        )}
        <div className="flex min-w-0 flex-wrap items-start gap-x-1.5 gap-y-1 px-2 pt-1.5">
          {extras.command && (
            // One line of the field's own height, so the command and its argument read as one entry.
            <Tooltip label={extras.command.description}>
              <span className="flex h-[19px] shrink-0 items-center rounded-xs bg-accent/12 px-1 font-semibold text-body text-accent leading-[19px]">
                {extras.command.name}
              </span>
            </Tooltip>
          )}
          {extras.links.map((url) => (
            <LinkChip
              key={url}
              url={url}
              onRemove={() => change((e) => ({ ...e, links: e.links.filter((l) => l !== url) }))}
            />
          ))}
          {/* Links in the draft read as links: a mirror under the field draws the same text with
              each URL on an accent pill, and the field's own text is transparent so the mirror shows
              through while the caret, selection and editing stay the field's. */}
          <div className="relative min-w-[160px] flex-1 basis-[160px]">
            {hasUrl && (
              <div
                ref={mirror}
                aria-hidden
                className="pointer-events-none absolute inset-0 overflow-hidden whitespace-pre-wrap break-words text-body text-fg leading-[19px]"
              >
                {splitUrls(draft).map((part, i) =>
                  part.url ? (
                    <span
                      key={i}
                      className="rounded-xs bg-accent/15 text-accent [box-decoration-break:clone]"
                    >
                      {part.url}
                    </span>
                  ) : (
                    part.text
                  ),
                )}
                {"\u200b"}
              </div>
            )}
            <textarea
              ref={field}
              value={draft}
              onChange={(e) => onChange(e.target.value)}
              onKeyDown={onKeyDown}
              onPaste={onPaste}
              onScroll={(e) => {
                if (mirror.current) mirror.current.scrollTop = e.currentTarget.scrollTop;
              }}
              rows={1}
              placeholder={placeholder}
              aria-label="Message"
              spellCheck
              className={cx(
                "relative block w-full resize-none bg-transparent text-body leading-[19px] outline-none placeholder:text-fg-faint",
                hasUrl && "text-transparent caret-fg selection:bg-accent/30",
              )}
            />
          </div>
        </div>
        <div className="flex items-center gap-0.5">
          <PlusMenu
            onAttach={chooseFiles}
            attachLabel={images ? "Attach Files or Images…" : "Attach Files…"}
            cwd={cwd}
          />
          <input
            ref={fileInput}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              const files = [...(e.target.files ?? [])];
              e.target.value = "";
              void attachmentsFromFiles(files, images, harnessName).then(attach);
            }}
          />
          <span className="flex-1" />
          <PlanMeter threadId={thread?.id} />
          {thread && <ContextRing threadId={thread.id} />}
          <ModelPicker thread={thread} compact={compact} />
          {running && canSend && (
            <Tooltip label="Queue for when this turn ends (⏎)">
              <button
                type="button"
                aria-label="Queue message"
                onClick={() => send(false)}
                className="ml-1 flex size-[30px] items-center justify-center rounded-full bg-fg text-bg hover:bg-fg/85"
              >
                <ListEnd size={13} strokeWidth={2.5} />
              </button>
            </Tooltip>
          )}
          {running ? (
            <Tooltip label={cancelReason ?? "Stop turn (Esc)"}>
              <button
                type="button"
                aria-label="Stop turn"
                disabled={!!cancelReason}
                onClick={() => {
                  haptic("level");
                  void cancel(thread?.id);
                }}
                className="ml-1 flex size-[30px] items-center justify-center rounded-full bg-danger text-accent-fg hover:bg-danger/85 disabled:opacity-40"
              >
                <Square size={10} fill="currentColor" strokeWidth={0} />
              </button>
            </Tooltip>
          ) : (
            <Tooltip label="Send (⏎)">
              <button
                type="button"
                aria-label="Send"
                disabled={!canSend || status !== "ready" || (!thread && !projectCwd)}
                onPointerEnter={(e) => !e.currentTarget.disabled && haptic("alignment")}
                onClick={() => send(false)}
                className="ml-1 flex size-[30px] items-center justify-center rounded-full bg-fg text-bg transition-[background-color,opacity] duration-150 enabled:hover:bg-fg/85 disabled:bg-fill-strong disabled:text-fg-muted"
              >
                <ArrowUp size={14} strokeWidth={2.75} />
              </button>
            </Tooltip>
          )}
        </div>
      </div>
      {/* The tray under the box: where the thread works and what it brings, after the SwiftUI footer. */}
      <div className="rc-composer-tray mx-3 -mt-[18px] flex min-h-[58px] items-end gap-0.5 rounded-b-2xl px-2 pt-[18px] pb-1.5">
        <FolderMenu thread={thread} cwd={cwd} />
        {!thread && <WorktreeToggle draft={draft} compact={compact} />}
        {thread?.worktree && (
          <Tooltip side="bottom" label={thread.worktree.path}>
            <span className="flex h-[28px] max-w-[220px] items-center gap-1.5 rounded-full px-2.5 text-callout text-fg-muted">
              <GitBranch size={12} className="shrink-0" />
              <span className="truncate">{thread.worktree.branch}</span>
            </span>
          </Tooltip>
        )}
        <Tooltip
          side="bottom"
          label={images ? "Attach files or images" : `Attach files (${harnessName} takes no images)`}
        >
          <button type="button" onClick={chooseFiles} className={trayButton}>
            <Paperclip size={12} />
            {!compact && <span>Files</span>}
          </button>
        </Tooltip>
        <PermissionMenu threadId={thread?.id} compact={compact} />
        <span className="flex-1" />
        <AgentPill thread={thread} />
        {harness && !harness.builtin && harness.id !== defaultHarness && (
          <Tooltip side="bottom" label="Its own sign-in and tools; RustCode's sandbox does not apply">
            <span className="flex h-[28px] items-center gap-1.5 rounded-full px-2.5 text-callout text-fg-muted">
              <HarnessIcon id={harness.id} name={harness.name} size={14} />
              {harness.name}
            </span>
          </Tooltip>
        )}
      </div>
    </div>
  );
}, sameComposer);

/**
 * Messages waiting for the running turn to end, as rows above the composer.
 * Each can be taken back into the composer to change it (↑, or Edit), moved,
 * sent now as a steer when the harness steers live, or removed.
 */
function QueueStack({
  thread,
  steerable,
  onTakeBack,
}: {
  thread: ThreadState;
  steerable: boolean;
  onTakeBack: (q: QueuedPrompt) => void;
}) {
  return (
    <section className="mb-2 flex flex-col gap-1 px-2" aria-label="Queued messages">
      <div className="flex items-center gap-1.5 px-1 text-caption text-accent">
        <ListEnd size={12} />
        <b>Queued</b>
        <span className="text-fg-muted">· sent when this turn ends{steerable ? ", or now with ⌘⏎" : ""}</span>
      </div>
      {thread.queue.map((q, i) => (
        <div
          key={q.id}
          className="elevated group flex min-h-[34px] items-center gap-2 rounded-lg px-2.5 py-1.5 rc-in"
        >
          <span className="w-3 shrink-0 text-center font-mono text-caption text-fg-faint">{i + 1}</span>
          <button
            type="button"
            onClick={() => onTakeBack(q)}
            className="flex min-w-0 flex-1 cursor-text items-center gap-1.5 text-left"
            aria-label="Edit in the composer"
            title="Edit in the composer (↑)"
          >
            {!!q.extras?.attachments.length && (
              <span className="flex shrink-0 items-center gap-0.5 rounded-sm bg-fill px-1 text-caption text-fg-muted">
                <Paperclip size={10} />
                {q.extras.attachments.length}
              </span>
            )}
            <span className={cx("line-clamp-2 text-callout", !q.text && "text-fg-faint")}>
              {q.text || "Attachments only"}
            </span>
          </button>
          <span className="flex shrink-0 items-center gap-1.5 opacity-60 group-hover:opacity-100">
            {i > 0 && (
              <Tooltip label="Move up">
                <button
                  type="button"
                  aria-label="Move up"
                  onClick={() => moveQueued(thread.id, q.id, -1)}
                  className="text-fg-muted hover:text-fg"
                >
                  <ChevronUp size={13} />
                </button>
              </Tooltip>
            )}
            <Tooltip label="Edit in the composer (↑)">
              <button
                type="button"
                aria-label="Edit in the composer"
                onClick={() => onTakeBack(q)}
                className="text-fg-muted hover:text-fg"
              >
                <Pencil size={12} />
              </button>
            </Tooltip>
            {steerable &&
              (q.extras?.attachments.length ? (
                <Tooltip label="Attachments go with the next turn: a running turn takes text only">
                  <span
                    role="img"
                    aria-label="Send now is not available with attachments"
                    className="text-fg-faint"
                  >
                    <ArrowUp size={13} />
                  </span>
                </Tooltip>
              ) : (
                <Tooltip label="Send now: the turn reads it after its current step">
                  <button
                    type="button"
                    aria-label="Send now"
                    onClick={() => void sendQueuedNow(thread.id, q.id)}
                    className="text-fg-muted hover:text-fg"
                  >
                    <ArrowUp size={13} />
                  </button>
                </Tooltip>
              ))}
            <Tooltip label="Remove">
              <button
                type="button"
                aria-label="Remove queued message"
                onClick={() => removeQueued(thread.id, q.id)}
                className="text-fg-faint hover:text-fg"
              >
                <X size={13} />
              </button>
            </Tooltip>
          </span>
        </div>
      ))}
    </section>
  );
}

function AttachmentStrip({
  attachments,
  onRemove,
}: {
  attachments: Attachment[];
  onRemove: (id: string) => void;
}) {
  const pictures = attachments.flatMap((a) =>
    a.kind === "image" ? [{ src: `data:${a.mimeType};base64,${a.data}`, alt: a.name }] : [],
  );
  return (
    <div className="flex gap-1.5 overflow-x-auto px-1 pt-0.5 pb-0.5">
      {attachments.map((a) =>
        a.kind === "image" ? (
          <div key={a.id} className="group relative shrink-0">
            <button
              type="button"
              aria-label={`Open ${a.name}`}
              onClick={() =>
                openLightbox(
                  pictures,
                  pictures.findIndex((p) => p.alt === a.name),
                )
              }
              className="block rounded-xl"
            >
              <img
                src={`data:${a.mimeType};base64,${a.data}`}
                alt={a.name}
                title={a.name}
                className="size-[76px] rounded-xl object-cover shadow-1"
              />
            </button>
            {/* Inside the corner, so the strip's scroll never crops it. */}
            <button
              type="button"
              aria-label={`Remove ${a.name}`}
              onClick={() => onRemove(a.id)}
              className="rc-glass absolute top-1.5 right-1.5 flex size-[20px] items-center justify-center rounded-full text-fg opacity-0 shadow-1 hover:bg-surface-3 group-hover:opacity-100 focus-visible:opacity-100"
            >
              <X size={11} strokeWidth={2.5} />
            </button>
          </div>
        ) : (
          <Tooltip key={a.id} label={a.path ?? `${a.name} (sent inline)`}>
            <span className="flex h-[28px] shrink-0 items-center gap-1.5 rounded-full bg-fill-subtle pr-1.5 pl-2.5 text-callout">
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
        ),
      )}
    </div>
  );
}

/**
 * "New worktree" for a new thread: the thread runs on its own branch in its
 * own folder, so parallel threads never edit the same checkout. The branch
 * name follows the message until the user edits it.
 */
function WorktreeToggle({ draft, compact }: { draft: string; compact?: boolean }) {
  const wt = useApp((s) => s.newWorktree);
  const missing = useApp((s) => (s.features.worktrees ? null : featureMissing("worktrees")));
  const suggested = suggestBranch(draft || "new thread");
  // biome-ignore lint/correctness/useExhaustiveDependencies: follow the draft until edited
  useEffect(() => {
    if (wt.on && !wt.edited && wt.branch !== suggested) setNewWorktree({ branch: suggested });
  }, [suggested, wt.on, wt.edited]);
  const valid = !wt.branch.trim() || validBranch(wt.branch.trim());
  return (
    <div className="flex items-center gap-1">
      <Tooltip
        side="bottom"
        label={
          missing ?? (wt.on ? "Runs on its own branch and folder" : "Run on a new branch, in its own folder")
        }
      >
        <button
          type="button"
          aria-pressed={wt.on}
          disabled={!!missing}
          onClick={() => setNewWorktree({ on: !wt.on, branch: suggested, edited: false })}
          className={cx(
            "flex h-[28px] items-center gap-1.5 rounded-full px-2.5 text-callout disabled:opacity-40",
            wt.on
              ? "bg-fill-strong text-fg"
              : "text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg enabled:active:bg-fill-strong",
          )}
        >
          <GitBranch size={12} />
          {!compact && !wt.on && <span>Worktree</span>}
        </button>
      </Tooltip>
      {wt.on && (
        <input
          value={wt.branch}
          onChange={(e) => setNewWorktree({ branch: e.target.value, edited: true })}
          aria-label="Branch name"
          aria-invalid={!valid}
          spellCheck={false}
          placeholder={suggested}
          size={Math.min(32, Math.max(12, wt.branch.length + 1))}
          className={cx(
            "h-[24px] rounded-full bg-fill px-2.5 font-mono text-caption rc-in",
            valid ? "focus:bg-fill-strong" : "text-danger",
          )}
        />
      )}
    </div>
  );
}

/** The folder a thread works in: a menu for a new thread, a label once it exists. */
function FolderMenu({ thread, cwd }: { thread?: ThreadState; cwd: string }) {
  const sessions = useApp((s) => s.sessions);
  const projects = useMemo(
    () => [...new Set(sessions.map((s) => s.cwd).filter(Boolean))].slice(0, 30),
    [sessions],
  );
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  // At most 360px or half the window, and never over the header: the room above the trigger less the 42px header and gaps.
  const [maxHeight, setMaxHeight] = useState(360);
  const toggle = () => {
    const top = trigger.current?.getBoundingClientRect().top ?? 600;
    setMaxHeight(Math.max(140, Math.min(360, window.innerHeight * 0.5, top - 42 - 16)));
    setOpen(!open);
  };
  return (
    <div className="relative">
      <Tooltip side="bottom" label={cwd || "Choose a folder"}>
        <button
          ref={trigger}
          type="button"
          disabled={!!thread}
          onClick={toggle}
          aria-haspopup="menu"
          aria-expanded={open}
          className={cx(trayButton, "disabled:hover:bg-transparent")}
        >
          <Folder size={12} />
          <span className="max-w-[220px] truncate">{cwd ? projectName(cwd) : "Choose a folder"}</span>
          {!thread && <ChevronDown size={11} className="text-fg-faint" />}
        </button>
      </Tooltip>
      {open && (
        <div
          role="menu"
          className="elevated absolute bottom-full left-0 z-30 mb-2 flex w-[300px] flex-col rounded-xl p-1.5 rc-in"
          style={{ maxHeight }}
          onMouseLeave={() => setOpen(false)}
        >
          {/* The projects scroll; "Choose Folder…" stays pinned under them. */}
          <div className="scroll-stable -mr-1.5 min-h-0 flex-1 pr-1.5">
            {projects.map((p) => (
              <button
                key={p}
                type="button"
                role="menuitem"
                onClick={() => {
                  setProjectCwd(p);
                  setOpen(false);
                }}
                className={cx(
                  "flex w-full flex-col rounded-md px-2.5 py-1.5 text-left hover:bg-fill",
                  p === cwd && "bg-fill",
                )}
              >
                <span className="text-body text-fg">{projectName(p)}</span>
                <span className="truncate text-caption text-fg-muted" dir="rtl">
                  <bdi>{p}</bdi>
                </span>
              </button>
            ))}
          </div>
          {projects.length > 0 && <div className="mx-1.5 my-1 h-px shrink-0 bg-border" />}
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              void pickFolder(cwd).then((p) => p && setProjectCwd(p));
            }}
            className="flex w-full shrink-0 rounded-md px-2.5 py-2 text-left text-body hover:bg-fill"
          >
            Choose Folder…
          </button>
        </div>
      )}
    </div>
  );
}

const trayButton =
  "flex h-[28px] items-center gap-1.5 rounded-full px-2.5 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong";

const PERMISSION_ICON: Record<PermissionMode, typeof Shield> = {
  ask: Shield,
  edits: ShieldCheck,
  full: ShieldOff,
};

/** What the thread may do without asking. Full access is tinted so it is never on unnoticed. */
function PermissionMenu({ threadId, compact }: { threadId?: string; compact?: boolean }) {
  const mode = useApp((s) => permissionModeOf(s, threadId));
  const current = PERMISSION_MODES.find((m) => m.id === mode)!;
  const Icon = PERMISSION_ICON[mode];
  return (
    <DropdownMenu.Root>
      <Tooltip side="bottom" label={current.detail}>
        <DropdownMenu.Trigger asChild>
          <button
            type="button"
            aria-label={`Permissions: ${current.label}`}
            className={cx(
              trayButton,
              "data-[state=open]:bg-fill",
              mode === "full" && "text-warning hover:text-warning",
            )}
          >
            <Icon size={12} />
            {!compact && <span>{current.label}</span>}
            <ChevronDown size={11} className="text-fg-faint" />
          </button>
        </DropdownMenu.Trigger>
      </Tooltip>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          side="top"
          align="start"
          sideOffset={8}
          className="elevated z-50 w-[280px] rounded-lg p-1"
        >
          <DropdownMenu.RadioGroup
            value={mode}
            onValueChange={(value) => setPermissionMode(threadId, value as PermissionMode)}
          >
            {PERMISSION_MODES.map((m) => {
              const ItemIcon = PERMISSION_ICON[m.id];
              return (
                <DropdownMenu.RadioItem
                  key={m.id}
                  value={m.id}
                  className="flex w-full items-start gap-2.5 rounded-md px-2.5 py-[7px] outline-none data-[highlighted]:bg-fill data-[state=checked]:bg-fill-subtle"
                >
                  <ItemIcon
                    size={13}
                    className={cx("mt-[3px] shrink-0", m.id === "full" && "text-warning")}
                  />
                  <span className="flex min-w-0 flex-col">
                    <span className="text-body text-fg">{m.label}</span>
                    <span className="text-caption text-fg-muted">{m.detail}</span>
                  </span>
                </DropdownMenu.RadioItem>
              );
            })}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/** The + menu at the start of the composer's row, after the SwiftUI app's AddPanel. */
function PlusMenu({
  onAttach,
  attachLabel,
  cwd,
}: {
  onAttach: () => void;
  attachLabel: string;
  cwd: string;
}) {
  const item =
    "flex w-full items-center gap-2 rounded-md px-2.5 py-[6px] text-body outline-none data-[highlighted]:bg-fill";
  return (
    <DropdownMenu.Root>
      <Tooltip label="Attach and more">
        <DropdownMenu.Trigger asChild>
          <button
            type="button"
            aria-label="Attach and more"
            className="flex size-[30px] items-center justify-center rounded-full bg-fill-subtle text-fg hover:bg-fill active:bg-fill-strong data-[state=open]:bg-fill-strong"
          >
            <Plus size={16} strokeWidth={2} />
          </button>
        </DropdownMenu.Trigger>
      </Tooltip>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          side="top"
          align="start"
          sideOffset={8}
          className="elevated z-50 min-w-[220px] rounded-lg p-1"
        >
          <DropdownMenu.Item className={item} onSelect={onAttach}>
            <Paperclip size={13} /> {attachLabel}
          </DropdownMenu.Item>
          <DropdownMenu.Separator className="mx-1.5 my-1 h-px bg-border" />
          <DropdownMenu.Item className={item} onSelect={() => startNewThread()}>
            <SquarePen size={13} /> New Thread
          </DropdownMenu.Item>
          <DropdownMenu.Item
            className={item}
            onSelect={() =>
              void pickFolder(cwd).then((p) => {
                if (!p) return;
                setProjectCwd(p);
                startNewThread(p);
              })
            }
          >
            <Folder size={13} /> New Thread in Folder…
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
