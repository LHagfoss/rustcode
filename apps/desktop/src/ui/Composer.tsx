// The composer, after ContentView.swift's Composer: one glass box floating over
// the transcript, holding the attachments, the field (with a slash-command token
// in front of it; pasted links stay inline as text) and its foot: + on the left,
// send/queue/stop on the right. The tray tucked behind that foot carries the rest
// (the folder, the worktree, access, the meter, the model pill). What waits for
// the thread is not here: queued messages are rows at the end of the transcript
// (ui/QueuedMessages.tsx), and one of them is edited in this field. Keys follow
// the SwiftUI app: ⏎ send (or queue while running), ⇧⏎ newline, ⌘⏎ steer, Esc
// closes suggestions, then leaves a queued edit, then clears the draft, then
// stops the turn.

import {
  ArrowUp,
  Check,
  ChevronDown,
  Clock,
  Command,
  Copy,
  Ellipsis,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
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
  SquareTerminal,
  X,
} from "lucide-react";
import { memo, type ReactNode, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { projectName, type SlashCommand } from "../acp/types";
import { suggestBranch, validBranch } from "../acp/v2";
import { FIELD_FADE, type FieldFade, fieldFade } from "../lib/fade";
import { haptic } from "../lib/haptics";
import {
  canOpenTerminal,
  onFileDrop,
  openPath,
  openTerminalAt,
  pickFiles,
  pickFolder,
  projectFiles,
  readImage,
} from "../lib/native";
import { observeSize } from "../lib/resize";
import {
  type Attachment,
  type ComposeExtras,
  emptyExtras,
  hasExtras,
  hasTokens,
  IMAGE_TYPES,
  imageMime,
  MAX_IMAGE_BYTES,
  MAX_INLINE_FILE_BYTES,
  sentMessages,
  splitTokens,
} from "../state/compose";
import { atToken, insertRef, rankFiles } from "../state/fileRefs";
import { controlDisabledReason } from "../state/harness";
import { PERMISSION_MODES, type PermissionMode } from "../state/permissions";
import {
  acceptsImages,
  answerPermission,
  cancel,
  cancelEdit,
  cancelQueuedEdit,
  canSteer,
  commandsFor,
  dropLimitWait,
  endQueuedEdit,
  featureMissing,
  limitTime,
  newThreadChoice,
  permissionModeOf,
  saveQueuedEdit,
  setDraft,
  setExtras,
  setNewWorktree,
  setPermissionMode,
  setProjectCwd,
  startNewThread,
  startQueuedEdit,
  steerOrSubmit,
  submit,
  threadHarness,
  toast,
  useApp,
} from "../state/store";
import { localId, type ThreadState } from "../state/thread";
import { AgentPill } from "./AgentViews";
import { UsageMeter } from "./ComposerUsage";
import { CardMenuItem, CardPopover, menuKeys } from "./cardPopover";
import { openLightbox } from "./Lightbox";
import { LinkChip } from "./LinkChip";
import { ModelPicker } from "./ModelPicker";
import { cx, MenuSearchField, MenuShortcut, Pill, Tooltip } from "./primitives";
import { QueuedAnnouncer } from "./QueuedMessages";

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

// The field's line box: 14px at 1.625 (`leading-relaxed`), the same leading replies read at.
const LINE = 22.75;
// Real padding inside the scroller, so it scrolls with the text: the first and last lines keep their
// room at either end, and the caret on the last line sits above the padding, never on the edge.
const PAD = 6;
// It opens at two lines above the card's foot and scrolls past the eighth. The minimum is whole
// lines plus that padding (`MIN_HEIGHT`), so the field cuts no line in half as it grows; past the
// maximum the fade (`.rc-field-fade`) hides the cut, as it does in any scroller.
const MIN_HEIGHT = Math.round(LINE * 2 + PAD * 2);
const MAX_HEIGHT = 208;

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

const NO_FILES: string[] = [];
/** How long a project's file list is used before `@` lists it again. */
const FILES_FRESH_MS = 30_000;

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
  // This thread's own session list, or else the menu of its harness (the engine's catalog for a new thread).
  const commands = useApp(useShallow((s) => commandsFor(s, thread?.id ?? null)));
  const projectCwd = useApp((s) => s.projectCwd);
  useApp((s) => s.harnesses); // re-render when harness capabilities arrive
  const status = useApp((s) => s.status);
  const field = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const [selected, setSelected] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [caret, setCaret] = useState(0);
  const [files, setFiles] = useState<{ cwd: string; at: number; paths: string[] } | null>(null);
  const [dropping, setDropping] = useState(false);

  const running = thread?.running ?? false;
  const steerable = thread ? canSteer(thread) : false;
  // The queued message the composer is editing (its pencil, or ↑): ⏎ saves it where it waits, and
  // never sends it. What the composer held before comes back when the edit ends.
  const queuedEdit = useApp((s) => (s.queuedEdit?.threadId === key ? s.queuedEdit : null));
  const list = useMemo(
    () => (dismissed === draft || extras.command ? [] : suggestions(draft, commands)),
    [draft, commands, dismissed, extras.command],
  );
  // `@`: the project's files, listed when the first reference opens, and again once it is stale.
  const token = dismissed === draft || list.length ? null : atToken(draft, caret);
  const cwdForFiles = thread?.cwd ?? projectCwd;
  const wantsFiles = !!token && !!cwdForFiles;
  useEffect(() => {
    if (!wantsFiles || (files?.cwd === cwdForFiles && Date.now() - files.at < FILES_FRESH_MS)) return;
    let live = true;
    void projectFiles(cwdForFiles).then((paths) => {
      if (live) setFiles({ cwd: cwdForFiles, at: Date.now(), paths });
    });
    return () => {
      live = false;
    };
  }, [wantsFiles, cwdForFiles, files]);
  const query = token?.query ?? null;
  const fileList = useMemo(
    () => (query !== null && files?.cwd === cwdForFiles ? rankFiles(files.paths, query) : NO_FILES),
    [query, files, cwdForFiles],
  );
  const harness = threadHarness(thread);
  const harnessName = harness?.name ?? "This harness";
  // Who the message goes to: the thread's harness, or for a new thread the one it will start on.
  const speakingTo = useApp((s) => {
    if (harness) return harness.name;
    const id = newThreadChoice(s)?.harness;
    return s.harnesses?.harnesses.find((h) => h.id === id)?.name ?? "RustCode";
  });
  const images = acceptsImages(thread);
  const cancelReason = controlDisabledReason("cancel", thread?.harness?.capabilities, harnessName);
  const cwd = thread?.cwd ?? projectCwd;
  const canSend = !!draft.trim() || hasExtras(extras);
  const mirror = useRef<HTMLDivElement>(null);
  // The control row and the meters it may have to give up: measured, not stepped by a container
  // query (docs/design.md › Composer). `groupWidth` keeps the meters' own width from when they
  // last drew, so the row knows what would have to fit again before they come back.
  const row = useRef<HTMLDivElement>(null);
  const meters = useRef<HTMLDivElement>(null);
  const groupWidth = useRef(0);
  const [tight, setTight] = useState(false);
  const shell = useRef<HTMLDivElement>(null);
  const drawerRef = useRef<HTMLDivElement>(null);
  // Where the field is in what ↑/↓ walks (`sentMessages`), and the draft the walk started from.
  const history = useRef<{ draft: string; index: number } | null>(null);
  // Anything the mirror has to draw: a URL, or a `@file` reference (state/compose.ts `splitTokens`).
  const tokenized = useMemo(() => hasTokens(draft), [draft]);
  // Which edges of the scrolled field fade (lib/fade.ts); nothing to fade while the draft fits.
  const [fade, setFade] = useState<FieldFade>("none");
  // Carried by the field and the mirror alike: the mask (`.rc-field-fade`) takes its two depths.
  const fadeStyle = {
    "--rc-fade-top": fade === "top" || fade === "both" ? `${FIELD_FADE}px` : "0px",
    "--rc-fade-bottom": fade === "bottom" || fade === "both" ? `${FIELD_FADE}px` : "0px",
  } as React.CSSProperties;
  // The field's scroll is read twice: the mirror keeps the highlighted text under the textarea, and
  // the fade follows which edge the draft is cut at.
  const syncField = () => {
    const el = field.current;
    if (!el) return;
    if (mirror.current && mirror.current.scrollTop !== el.scrollTop) mirror.current.scrollTop = el.scrollTop;
    setFade(fieldFade(el.scrollTop, el.scrollHeight, el.clientHeight));
  };

  // A draft taller than the box scrolls, and the browser parks it so the caret's line only just
  // clears the bottom edge, leaving a line's worth of slack under it. The fade then dims the very
  // line being typed on (a new line after ⇧⏎ lands in the shadow). Typing at the end of the draft
  // asks to see its end, so go all the way down: the scroll range is whole lines, so the end is a
  // line boundary too, the last line reads clear of the fade, and the fade itself turns off there.
  const followCaret = () => {
    const el = field.current;
    if (!el || el.scrollHeight <= el.clientHeight) return;
    if (el.selectionStart === el.value.length && el.selectionStart === el.selectionEnd) {
      el.scrollTop = el.scrollHeight;
    }
  };

  // The row's controls fit or they do not: measured, so a narrow window moves the meters into one
  // menu instead of squeezing them, and a wide one puts them back. The meters keep their size (a
  // ring under 30px is unreadable), which is why they overflow rather than shrink.
  useLayoutEffect(() => {
    const el = row.current;
    if (!el) return;
    const measure = () => {
      const line = row.current;
      if (!line) return;
      if (meters.current) groupWidth.current = meters.current.offsetWidth;
      if (!tight) {
        // Everything drawn: a row wider than its box is one control past its edge.
        if (line.scrollWidth > line.clientWidth + 1) setTight(true);
        return;
      }
      // Tight: the meters come back when what is drawn leaves room for what they measured.
      const used = [...line.children].reduce((sum, c) => sum + (c as HTMLElement).offsetWidth, 0);
      const gaps = 8 * Math.max(0, line.children.length - 1);
      if (used + gaps + groupWidth.current + 8 <= line.clientWidth) setTight(false);
    };
    measure();
    return observeSize(el, measure);
  }, [tight]);

  // Ready to type on entering a thread, and again on coming back to the chat from Settings or
  // another view (the draft is kept either way).
  const inChat = useApp((s) => !s.settings && s.view.kind === "chat" && !s.switcherOpen);
  // biome-ignore lint/correctness/useExhaustiveDependencies: refocus on thread change
  useEffect(() => {
    history.current = null;
    if (inChat) field.current?.focus();
  }, [key, inChat]);

  // Grow from one line to the field's height, then scroll.
  // biome-ignore lint/correctness/useExhaustiveDependencies: measure on every draft change
  useLayoutEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "0px";
    // `scrollHeight` is the text plus the padding above and below, which is room the descenders of
    // the last line (g, j, p, y in the rounded font) need inside the box instead of at its edge.
    el.style.height = `${Math.min(Math.max(el.scrollHeight, MIN_HEIGHT), MAX_HEIGHT)}px`;
    followCaret();
    syncField();
    // The browser parks the field at the caret a frame after a new value lands, so ask again then:
    // a pasted draft would otherwise show a hard top edge until the next scroll, and the caret it
    // scrolled to would sit in the fade.
    const frame = requestAnimationFrame(() => {
      followCaret();
      syncField();
    });
    return () => cancelAnimationFrame(frame);
  }, [draft]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new suggestion list starts at the top
  useEffect(() => setSelected(0), [list.length, fileList]);

  // A narrower window rewraps the draft, so the field can start or stop scrolling: measure again.
  // biome-ignore lint/correctness/useExhaustiveDependencies: the field and the mirror are refs
  useEffect(() => {
    const el = field.current;
    if (!el) return;
    return observeSize(el, syncField);
  }, []);

  const update = (text: string) => setDraft(key, text);
  // A message walked back into the field puts the caret at its end, so ↓ comes forward again at once.
  const recall = (text: string) => {
    update(text);
    requestAnimationFrame(() => {
      const el = field.current;
      if (el) el.selectionStart = el.selectionEnd = el.value.length;
    });
  };
  const change = (fn: (e: ComposeExtras) => ComposeExtras) => setExtras(key, fn);

  // A queued message that leaves the queue while it is open here (it started, or it was removed)
  // ends its edit; the store keeps what the user typed only when the composer was empty before it.
  const queue = thread?.queue;
  useEffect(() => {
    if (!queuedEdit) return;
    if (queuedEdit.threadId !== key || !queue?.some((q) => q.id === queuedEdit.queuedId)) endQueuedEdit();
  }, [key, queue, queuedEdit]);
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

  const approval = thread?.permissions.find((p) => !p.question);
  const send = (steer: boolean) => {
    haptic("generic");
    // ⏎ on a queued message being edited saves it in the queue: nothing is sent.
    if (queuedEdit) {
      saveQueuedEdit();
      field.current?.focus();
      return;
    }
    const text = draft;
    const sentExtras = extras;
    if (!text.trim() && !hasExtras(sentExtras) && !(steer && running)) return;
    // A message sent while an approval waits declines it: the message says what to do instead.
    if (approval && (text.trim() || hasExtras(sentExtras))) {
      const decline =
        approval.options.find((o) => o.kind === "reject_once") ??
        approval.options.find((o) => o.kind === "reject_always");
      if (decline) answerPermission(approval, decline);
    }
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

  const reference = (path: string) => {
    if (!token) return;
    const next = insertRef(draft, token, path);
    update(next.text);
    setCaret(next.caret);
    requestAnimationFrame(() => field.current?.setSelectionRange(next.caret, next.caret));
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
    // An IME's Enter (keyCode 229) belongs to the field: it commits a candidate. Taking it would
    // complete a command or send a draft that was still being written.
    if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) return;
    if (fileList.length) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setSelected((i) => (i + (e.key === "ArrowDown" ? 1 : fileList.length - 1)) % fileList.length);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && !e.metaKey)) {
        e.preventDefault();
        const path = fileList[selected];
        if (path) reference(path);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissed(draft);
        return;
      }
    }
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
      // A held Enter sends one message, not one a frame.
      if (e.repeat) return;
      e.preventDefault();
      send(e.metaKey || e.ctrlKey);
      return;
    }
    if (e.key === "Escape") {
      // The topmost thing goes first: an open popover, menu or dialog takes this Escape for
      // itself, and the draft and the running turn stay as they are (issue 2804).
      if (overlayOpen()) return;
      // A queued message open here goes back as it was, and the composer gets its own message back.
      if (queuedEdit) {
        e.preventDefault();
        cancelQueuedEdit();
        return;
      }
      if (draft || hasExtras(extras)) {
        e.preventDefault();
        update("");
        change(() => emptyExtras);
        return;
      }
      // Nothing here to take it: no preventDefault, so an open toast can dismiss (ui/Toaster.tsx).
      if (running) {
        e.preventDefault();
        void cancel(thread?.id);
      }
      return;
    }
    // ↑ and ↓ walk what this thread was sent, newest first, a line at a time: ↑ at the first line
    // goes back, ↓ at the last line comes forward, and past the newest the draft returns. What is
    // queued here opens before the history does (it is edited, never sent again).
    if ((e.key === "ArrowUp" || e.key === "ArrowDown") && !hasExtras(extras) && thread && !queuedEdit) {
      if (el.selectionStart !== el.selectionEnd) return;
      // The first and last line, not the first and last character: a recalled message keeps ↑ and ↓
      // walking from its own ends, and a draft of several lines still moves its caret line by line.
      const atFirst = !el.value.slice(0, el.selectionStart).includes("\n");
      const atLast = !el.value.slice(el.selectionStart).includes("\n");
      const live = useApp.getState().threads[thread.id] ?? thread;
      const walking = history.current;
      if (e.key === "ArrowUp") {
        if (!atFirst) return;
        if (!walking) {
          if (draft) return;
          const queued = [...live.queue].reverse().find((q) => !q.link && !q.wake);
          if (queued) {
            e.preventDefault();
            startQueuedEdit(thread.id, queued.id);
            return;
          }
        }
        const sent = sentMessages(live.items ?? thread.items);
        if (!sent.length) return;
        e.preventDefault();
        const index = walking ? Math.min(walking.index + 1, sent.length - 1) : 0;
        history.current = { draft: walking?.draft ?? draft, index };
        recall(sent[sent.length - 1 - index]!);
        return;
      }
      if (!walking || !atLast) return;
      e.preventDefault();
      if (walking.index === 0) {
        history.current = null;
        recall(walking.draft);
        return;
      }
      const sent = sentMessages(live.items ?? thread.items);
      const index = walking.index - 1;
      history.current = { draft: walking.draft, index };
      recall(sent[sent.length - 1 - index] ?? walking.draft);
    }
  };

  const chooseFiles = () => {
    void pickFiles(cwd).then((paths) => {
      if (paths === null) fileInput.current?.click();
      else void attachmentsFromPaths(paths, images, harnessName).then(attach);
    });
  };

  const placeholder = queuedEdit
    ? "Edit the queued message: ⏎ saves it in the queue, esc leaves it…"
    : extras.command
      ? extras.command.description || "Arguments…"
      : approval
        ? "Send a message to decline and redirect the agent…"
        : running
          ? steerable
            ? "⏎ queues it for the next turn, ⌘⏎ steers this one…"
            : "⏎ queues it for the next turn…"
          : `Message ${speakingTo}…`;

  // The drawer over the box: the commands `/` is looking for, or the files `@` is. A slash that
  // matches nothing still opens it and says so, where the field is about to be typed in.
  const slashTyped = !extras.command && dismissed !== draft && /^\/\S*$/.test(draft);
  const drawer = slashTyped ? "commands" : token && files?.cwd === cwdForFiles ? "files" : null;

  // The drawer reaches up over the transcript, under the box's top edge. It tells the chat column how
  // far it does (`--rc-composer-float`, from the foot of the pane), so the jump-to-latest pill sits
  // above it instead of over it (ui/Transcript.tsx). Nothing floats above the pane otherwise.
  // biome-ignore lint/correctness/useExhaustiveDependencies: measured when the drawer opens and closes
  useLayoutEffect(() => {
    const main = shell.current?.closest<HTMLElement>('main[aria-label="Chat"]');
    if (!main) return;
    const publish = () => {
      const el = drawerRef.current;
      const parent = el?.offsetParent as HTMLElement | null | undefined;
      // `offsetTop`, not the drawer's own rect: the drawer rises in with `rc-in`, and a rect read
      // mid-animation leaves the pill a few px short of the drawer's settled top.
      const top =
        el && parent
          ? main.getBoundingClientRect().bottom - parent.getBoundingClientRect().top - el.offsetTop
          : 0;
      main.style.setProperty("--rc-composer-float", `${Math.max(0, Math.round(top))}px`);
    };
    publish();
    const stop = drawerRef.current ? observeSize(drawerRef.current, publish) : undefined;
    window.addEventListener("resize", publish);
    return () => {
      stop?.();
      window.removeEventListener("resize", publish);
      main.style.removeProperty("--rc-composer-float");
    };
  }, [drawer]);

  return (
    <div ref={shell} className="rc-composer-shell relative @container/composer">
      {thread && <QueuedAnnouncer threadId={thread.id} />}
      {/* The box: the field and how it is sent. The drawer tucks under its top, the tray under its
          foot, so the three read as one floating surface over the transcript. */}
      <div className="relative">
        {drawer && (
          <div
            ref={drawerRef}
            className="rc-composer-drawer rc-in absolute inset-x-6 bottom-[calc(100%-1rem)] z-0 flex max-h-72 flex-col gap-0.5 overflow-y-auto px-1.5 pt-1.5 pb-[calc(1rem+6px)]"
            role="listbox"
            aria-label={drawer === "commands" ? "Commands" : "Files"}
          >
            {drawer === "commands" &&
              list.map((c, i) => (
                <button
                  key={c.name}
                  type="button"
                  role="option"
                  aria-selected={i === selected}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => complete(c, true)}
                  onMouseEnter={() => setSelected(i)}
                  className={cx(
                    "flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left",
                    i === selected && "bg-fill",
                  )}
                >
                  <Command size={16} className="shrink-0 text-fg-muted" />
                  {/* A command that wants arguments says so before it is picked. */}
                  <span className="min-w-[96px] max-w-[45%] shrink-0 truncate font-medium text-fg text-sub">
                    {c.name}
                    {c.takesArguments && <span className="text-fg-faint"> …</span>}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-fg-muted text-sub">{c.description}</span>
                  {/* What answers it: the app itself, not the harness. */}
                  {(c.action !== "execute" || c.report) && (
                    <span className="ml-auto shrink-0 rounded-xs bg-fill px-1.5 text-caption text-fg-muted">
                      App
                    </span>
                  )}
                </button>
              ))}
            {drawer === "files" &&
              fileList.map((path, i) => {
                const cut = path.lastIndexOf("/") + 1;
                return (
                  <button
                    key={path}
                    type="button"
                    role="option"
                    aria-selected={i === selected}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => reference(path)}
                    onMouseEnter={() => setSelected(i)}
                    className={cx(
                      "flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left",
                      i === selected && "bg-fill",
                    )}
                  >
                    <FileText size={16} className="shrink-0 text-fg-muted" />
                    <span className="min-w-[96px] max-w-[45%] shrink-0 truncate font-medium text-fg text-sub">
                      {path.slice(cut)}
                    </span>
                    <span className="min-w-0 flex-1 truncate text-fg-muted text-sub">
                      {path.slice(0, cut)}
                    </span>
                    <span className="ml-auto shrink-0 rounded-xs bg-fill px-1.5 text-caption text-fg-muted">
                      Project
                    </span>
                  </button>
                );
              })}
            {(drawer === "commands" ? !list.length : !fileList.length) && (
              <p className="px-5 pt-3.5 pb-7 text-fg-muted text-sub">
                {drawer === "commands" ? "No matching command." : "No matching files or folders."}
              </p>
            )}
          </div>
        )}
        <div
          className={cx(
            // The field's own card: glass, floating over the transcript. Focus steps the glass;
            // there is no ring or colour. Its radius is its own, larger step (index.css). A draft
            // that gains or loses a line grows the card over 150ms instead of jumping; the
            // transcript, pinned to its end, follows it up.
            "rc-composer relative z-10 flex flex-col gap-1",
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
          {thread && <LimitWaitRow threadId={thread.id} />}
          {queuedEdit && (
            // A queued message is edited here, not in an input of its own: its text, command, links
            // and attachments are the composer's while it is open, and ⏎ saves them where it waits.
            <div className="flex items-center gap-1.5 px-4 pt-2 text-caption text-accent">
              <Pencil size={11} />
              <b>Editing a queued message</b>
              <span className="text-fg-muted">· ⏎ saves it in the queue, esc leaves it</span>
              <Pill quiet size="sm" className="ml-auto" onClick={() => cancelQueuedEdit()}>
                Cancel
              </Pill>
            </div>
          )}
          {editing && (
            <div className="flex items-center gap-1.5 px-4 pt-2 text-caption text-accent">
              <Pencil size={11} />
              <b>Editing a sent message</b>
              <span className="text-fg-muted">· sending takes the thread back to it</span>
              <Pill
                quiet
                size="sm"
                className="ml-auto"
                onClick={() => {
                  cancelEdit(key);
                  update("");
                }}
              >
                Cancel
              </Pill>
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
          {/* The field and what reads in front of it. The chip and the links drop the field's own 6px
              of padding so they sit on its first line box, not above it. Every text row in the card
              starts at the same 16px (`px-4`); the tray is inset 8px because its round controls carry
              8px of their own, which puts their icons on that same edge. */}
          <div className="flex min-w-0 flex-wrap items-start gap-x-1.5 gap-y-1 px-4 pt-3.5 sm:pt-4">
            {extras.command && (
              // One line of the field's own height (22.75px), so the command and its argument read
              // as one entry: the same leading keeps their baselines together.
              <Tooltip label={extras.command.description}>
                <span className="mt-1.5 flex h-[22.75px] shrink-0 items-center rounded-xs bg-accent/12 px-1.5 font-semibold text-accent text-body leading-[22.75px] rc-in">
                  {extras.command.name}
                </span>
              </Tooltip>
            )}
            {extras.links.map((url) => (
              <LinkChip
                key={url}
                url={url}
                className="mt-1.5 rc-in"
                onRemove={() => change((e) => ({ ...e, links: e.links.filter((l) => l !== url) }))}
              />
            ))}
            {/* Tokens in the draft read as tokens: a mirror under the field draws the same text with
                  each URL on an accent pill and each `@file` reference on a quiet one, and the field's
                  own text is transparent so the mirror shows through while the caret, selection and
                  editing stay the field's. The two take the same padding, the same scroll offset and
                  the same mask, so neither drifts from the other; a token's span adds no horizontal
                  padding, which is what keeps the caret on its character. */}
            <div className="relative min-w-[160px] flex-1 basis-[160px]">
              {tokenized && (
                <div
                  ref={mirror}
                  aria-hidden
                  className="rc-field-fade pointer-events-none absolute inset-0 overflow-hidden whitespace-pre-wrap break-words py-1.5 text-body text-fg leading-relaxed"
                  style={fadeStyle}
                >
                  {splitTokens(draft).map((part, i) =>
                    part.url ? (
                      <span
                        key={i}
                        className="rounded-xs bg-accent/15 text-accent [box-decoration-break:clone]"
                      >
                        {part.url}
                      </span>
                    ) : part.ref ? (
                      <span
                        key={i}
                        className="rounded-xs bg-fill-strong font-medium text-fg [box-decoration-break:clone]"
                      >
                        @{part.ref}
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
                onChange={(e) => {
                  history.current = null;
                  setCaret(e.target.selectionStart);
                  onChange(e.target.value);
                }}
                onSelect={(e) => {
                  setCaret(e.currentTarget.selectionStart);
                  // A line wrapped or a ⇧⏎ pressed with the caret at the end: keep its line clear.
                  followCaret();
                }}
                onKeyDown={onKeyDown}
                onPaste={onPaste}
                onScroll={syncField}
                rows={1}
                placeholder={placeholder}
                aria-label="Message"
                spellCheck
                style={fadeStyle}
                className={cx(
                  "rc-field-fade relative block w-full resize-none bg-transparent py-1.5 text-body leading-relaxed outline-none transition-[height] duration-150 ease-snappy placeholder:text-fg-faint",
                  tokenized && "text-transparent caret-fg selection:bg-accent/30",
                )}
              />
            </div>
          </div>
          {/* The card's one control row: what adds to the message and where the thread works on the
              left, what it costs, how it runs and how it is sent on the right. No divider: the
              field's own air, and the row's `pt-0.5`, separate them. One row of one height, so the
              attach button, the folder and the menus line up with the meter, the ring, the model
              pill and the agent pill. Labels drop out with the card's width (the folder falls back
              to its icon); the controls do not, and the two that can still be long (the folder and
              the model) shrink and ellipsize before the row overflows. */}
          <div ref={row} className="rc-composer-tray flex items-center gap-2 px-2 pt-1 pb-2">
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
            <div className="flex min-w-0 items-center gap-0.5">
              <FolderMenu thread={thread} cwd={cwd} />
              {!thread && <WorktreeToggle draft={draft} compact={compact} />}
              {thread?.worktree && (
                <Tooltip side="bottom" label={thread.worktree.path}>
                  <span className="flex h-7 max-w-[220px] items-center gap-1.5 rounded-full px-2.5 text-callout text-fg-muted @max-xl/composer:hidden sm:h-6">
                    <GitBranch size={12} className="shrink-0" />
                    <span className="truncate">{thread.worktree.branch}</span>
                  </span>
                </Tooltip>
              )}
              <PermissionMenu threadId={thread?.id} compact={compact} />
            </div>
            <span className="flex-1" />
            <div className="flex shrink-0 items-center gap-2">
              {/* The meters keep their size: a ring squeezed under 30px is unreadable. A row too
                  narrow for them draws them in one menu instead, measured rather than stepped. */}
              {!tight ? (
                <div ref={meters} className="flex items-center gap-1.5">
                  <span className="flex h-7 shrink-0 items-center empty:hidden">
                    <UsageMeter threadId={thread?.id} />
                  </span>
                  <span className="flex h-7 shrink-0 items-center empty:hidden @max-sm/composer:hidden">
                    <AgentPill threadId={thread?.id} />
                  </span>
                </div>
              ) : (
                <MetersMenu thread={thread} />
              )}
              <ModelPicker thread={thread} compact={compact} />
              {queuedEdit ? (
                <Tooltip label="Save in the queue (⏎)">
                  <button
                    type="button"
                    aria-label="Save the queued message"
                    onClick={() => {
                      saveQueuedEdit();
                      field.current?.focus();
                    }}
                    className="flex size-9 items-center justify-center rounded-full bg-fg text-bg hover:bg-fg/85 sm:size-8"
                  >
                    <Check size={14} strokeWidth={2.75} />
                  </button>
                </Tooltip>
              ) : (
                running &&
                canSend && (
                  <Tooltip label="Queue for when this turn ends (⏎)">
                    <button
                      type="button"
                      aria-label="Queue message"
                      onClick={() => send(false)}
                      className="flex size-9 items-center justify-center rounded-full bg-fg text-bg hover:bg-fg/85 sm:size-8"
                    >
                      <ListEnd size={13} strokeWidth={2.5} />
                    </button>
                  </Tooltip>
                )
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
                    className="flex size-9 items-center justify-center rounded-full bg-danger text-accent-fg hover:bg-danger/85 disabled:opacity-40 sm:size-8"
                  >
                    <Square size={10} fill="currentColor" strokeWidth={0} />
                  </button>
                </Tooltip>
              ) : queuedEdit ? null : (
                <Tooltip label="Send (⏎)">
                  <button
                    type="button"
                    aria-label="Send"
                    disabled={!canSend || status !== "ready" || (!thread && !projectCwd)}
                    onPointerEnter={(e) => !e.currentTarget.disabled && haptic("alignment")}
                    onClick={() => send(false)}
                    className="flex size-9 items-center justify-center rounded-full bg-fg text-bg transition-[background-color,opacity] duration-150 enabled:hover:bg-fg/85 disabled:bg-fill-strong disabled:text-fg-muted sm:size-8"
                  >
                    <ArrowUp size={14} strokeWidth={2.75} />
                  </button>
                </Tooltip>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}, sameComposer);

/**
 * The meters a control row too narrow to hold them: one button, and the same meters inside it, in
 * the order they would have stood. The row is measured, not stepped by a breakpoint.
 */
function MetersMenu({ thread }: { thread?: ThreadState }) {
  return (
    <CardPopover
      align="end"
      role="dialog"
      label="Usage and agents"
      className="w-[min(300px,calc(100vw-40px))]"
      trigger={
        <button
          type="button"
          aria-label="Usage and agents"
          className="flex size-[28px] shrink-0 items-center justify-center rounded-full text-fg-muted hover:bg-fill hover:text-fg data-[state=open]:bg-fill-strong data-[state=open]:text-fg"
        >
          <Ellipsis size={14} />
        </button>
      }
    >
      <MeterRow label="Usage">
        <UsageMeter threadId={thread?.id} />
      </MeterRow>
      <MeterRow label="Agents">
        <AgentPill threadId={thread?.id} />
      </MeterRow>
    </CardPopover>
  );
}

/** One meter in `MetersMenu`: its name on the left, the control itself on the right. A meter with
 * nothing to say hides its row rather than drawing a label over an empty slot. */
function MeterRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-h-8 items-center justify-between gap-3 rounded-sm px-1.5 py-1 empty:hidden has-[>span:empty]:hidden">
      <span className="text-callout text-fg-muted">{label}</span>
      <span className="flex h-7 shrink-0 items-center">{children}</span>
    </div>
  );
}

/** The thread stopped on its plan's limit and carries on by itself when it resets. */
function LimitWaitRow({ threadId }: { threadId: string }) {
  const wait = useApp((s) => s.limitWaits[threadId]);
  // Once it is continuing, the turn says so; the wait is settled when it ends.
  const running = useApp((s) => s.threads[threadId]?.running);
  if (!wait || running) return null;
  return (
    <div className="flex items-center gap-1.5 px-4 pt-2 text-caption text-fg-muted">
      <Clock size={11} className="shrink-0" />
      <span className="min-w-0 truncate">Continues when the limit resets, at {limitTime(wait.until)}</span>
      <Pill quiet size="sm" className="ml-auto shrink-0" onClick={() => dropLimitWait(threadId)}>
        Don't continue
      </Pill>
    </div>
  );
}

/**
 * How many thumbnails a draft shows before the rest become one count.
 *
 * Past two or three pictures the band is a wall of thumbnails that pushes the field down for
 * nothing: the count says `+3` and opens the whole set, the way a full attachment row collapses in
 * T3 Code (`getRestingComposerImagePreviewCounts`). Files keep their own chips, after them.
 */
const IMAGE_PREVIEW_LIMIT = 3;

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
  const images = attachments.filter((a) => a.kind === "image");
  const shown = images.slice(0, IMAGE_PREVIEW_LIMIT);
  const more = images.slice(IMAGE_PREVIEW_LIMIT);
  return (
    // Its own band above the field, inset from the box's rounder corner and never shrinking, so the
    // thumbnails clear the curve and only the field below scrolls.
    <div className="flex shrink-0 gap-2 overflow-x-auto px-4 pt-2.5 pb-1">
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
              <X size={11} strokeWidth={2.5} />
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
      {attachments
        .filter((a) => a.kind !== "image")
        .map((a) => (
          <Tooltip key={a.id} label={a.path ?? `${a.name} (sent inline)`}>
            <span className="flex h-[26px] shrink-0 items-center gap-1.5 rounded-full bg-fill-subtle pr-1.5 pl-2.5 text-callout rc-in">
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
        ))}
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
    // Not `shrink`: the toggle keeps its button's width so it never runs under the mode beside it;
    // the folder name is the one label that gives way.
    <div className="flex min-w-0 shrink-0 items-center gap-1">
      <Tooltip
        side="bottom"
        label={
          missing ?? (wt.on ? "Runs on its own branch and folder" : "Run on a new branch, in its own folder")
        }
      >
        <button
          type="button"
          aria-pressed={wt.on}
          aria-label="Run in a new worktree"
          disabled={!!missing}
          onClick={() => setNewWorktree({ on: !wt.on, branch: suggested, edited: false })}
          className={cx(
            "flex h-7 shrink-0 items-center gap-1.5 rounded-full px-2.5 text-callout disabled:opacity-40 sm:h-6",
            wt.on
              ? "bg-fill-strong text-fg"
              : "text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg enabled:active:bg-fill-strong",
          )}
        >
          <GitBranch size={12} className="shrink-0" />
          {/* The label drops at the same step as the branch chip beside it, so a narrow card keeps
              the toggle, the folder and the mode without the row running out of room. */}
          {!compact && !wt.on && <span className="@max-xl/composer:hidden">Worktree</span>}
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
            "h-[24px] min-w-0 max-w-[220px] rounded-full bg-fill px-2.5 font-mono text-caption rc-in",
            valid ? "focus:bg-fill-strong" : "text-danger",
          )}
        />
      )}
    </div>
  );
}

/**
 * The folder a thread works in: for a new thread a menu of projects to start
 * in; once the thread exists, what to do with its folder (Finder, Terminal,
 * copy the path, another thread there).
 */
function FolderMenu({ thread, cwd }: { thread?: ThreadState; cwd: string }) {
  const sessions = useApp((s) => s.sessions);
  const projects = useMemo(
    () => [...new Set(sessions.map((s) => s.cwd).filter(Boolean))].slice(0, 30),
    [sessions],
  );
  const [filter, setFilter] = useState("");
  const shown = projects.filter(
    (p) =>
      !filter.trim() ||
      projectName(p).toLowerCase().includes(filter.trim().toLowerCase()) ||
      p.toLowerCase().includes(filter.trim().toLowerCase()),
  );
  return (
    <CardPopover
      align="start"
      label={cwd ? `Project folder: ${projectName(cwd)}` : "Choose a folder"}
      onKeyDown={menuKeys}
      tooltip={cwd || "Choose a folder"}
      className="flex w-[300px] flex-col gap-1"
      trigger={
        <button
          type="button"
          // The name is the trigger's own text at full width and its icon's label when it drops
          // out under a container width, so the button keeps a name either way.
          aria-label={cwd ? `Project folder: ${projectName(cwd)}` : "Choose a folder"}
          disabled={!!thread && !cwd}
          className={cx(
            footerButton,
            "min-w-0 shrink data-[state=open]:bg-fill disabled:hover:bg-transparent",
          )}
        >
          <Folder size={12} className="shrink-0" />
          <span className="min-w-0 max-w-[220px] truncate @max-lg/composer:max-w-[96px] @max-sm/composer:hidden">
            {cwd ? projectName(cwd) : "Choose a folder"}
          </span>
          <ChevronDown size={11} className="shrink-0 text-fg-faint" />
        </button>
      }
    >
      {thread ? (
        <FolderActions cwd={cwd} />
      ) : (
        <>
          {/* The projects scroll under a search that filters them; "Choose Folder…" stays pinned. */}
          {projects.length > 4 && (
            <div data-menu-static className="px-0.5 pb-0.5">
              <MenuSearchField
                value={filter}
                onChange={setFilter}
                placeholder="Search folders"
                label="Search folders"
              />
            </div>
          )}
          <div className="scroll-stable min-h-0 flex-1">
            {shown.map((p) => (
              <CardMenuItem
                key={p}
                onSelect={() => setProjectCwd(p)}
                className={cx("flex-col items-start gap-0", p === cwd && "bg-fill")}
              >
                <span className="text-body text-fg">{projectName(p)}</span>
                <span className="w-full truncate text-caption text-fg-muted" dir="rtl">
                  <bdi>{p}</bdi>
                </span>
              </CardMenuItem>
            ))}
            {!shown.length && <p className="px-3 py-2 text-callout text-fg-muted">No folder matches</p>}
          </div>
          {projects.length > 0 && <div className="mx-1.5 my-1 h-px shrink-0 bg-border" />}
          <CardMenuItem onSelect={() => void pickFolder(cwd).then((p) => p && setProjectCwd(p))}>
            <FolderPlus size={13} className="text-fg-muted" /> Choose Folder…
          </CardMenuItem>
        </>
      )}
    </CardPopover>
  );
}

/** What the folder of an existing thread offers. */
function FolderActions({ cwd }: { cwd: string }) {
  return (
    <>
      <div className="truncate px-3 pt-1 pb-1.5 text-caption text-fg-muted" dir="rtl">
        <bdi>{cwd}</bdi>
      </div>
      <CardMenuItem onSelect={() => openPath(cwd)}>
        <FolderOpen size={13} className="text-fg-muted" /> Open in Finder
      </CardMenuItem>
      {canOpenTerminal() && (
        <CardMenuItem onSelect={() => void openTerminalAt(cwd).catch((e) => toast(String(e), "error"))}>
          <SquareTerminal size={13} className="text-fg-muted" /> Open in Terminal
        </CardMenuItem>
      )}
      <CardMenuItem
        onSelect={() => void navigator.clipboard.writeText(cwd).then(() => toast("Path copied", "info"))}
      >
        <Copy size={13} className="text-fg-muted" /> Copy Path
      </CardMenuItem>
      <div className="mx-1.5 my-1 h-px shrink-0 bg-border" />
      <CardMenuItem onSelect={() => startNewThread(cwd)}>
        <SquarePen size={13} className="text-fg-muted" /> New Thread Here
      </CardMenuItem>
    </>
  );
}

// A button in the composer's footer: quiet until hovered, and its fill is its open state. It is
// not `shrink-0`: a long folder name gives way to the controls to its right (see FolderMenu).
const footerButton =
  "flex h-7 items-center gap-1.5 whitespace-nowrap rounded-full px-2.5 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong sm:h-6";

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
    <CardPopover
      align="start"
      role="menu"
      label="Permissions"
      tooltip={current.detail}
      onKeyDown={menuKeys}
      className="w-[280px]"
      trigger={
        <button
          type="button"
          aria-label={`Permissions: ${current.label}`}
          className={cx(
            footerButton,
            "shrink-0 data-[state=open]:bg-fill",
            mode === "full" && "text-warning hover:text-warning",
          )}
        >
          <Icon size={12} />
          {!compact && <span className="@max-md/composer:hidden">{current.label}</span>}
          <ChevronDown size={11} className="text-fg-faint" />
        </button>
      }
    >
      <div role="menu" aria-label="Permissions">
        {PERMISSION_MODES.map((m) => {
          const ItemIcon = PERMISSION_ICON[m.id];
          const checked = m.id === mode;
          return (
            <CardMenuItem
              key={m.id}
              role="menuitemradio"
              aria-checked={checked}
              onSelect={() => setPermissionMode(threadId, m.id)}
              className={cx("items-start gap-2.5 py-1.5", checked && "bg-fill-subtle")}
            >
              <ItemIcon
                size={13}
                className={cx("mt-[3px] shrink-0", m.id === "full" ? "text-warning" : "text-fg-muted")}
              />
              <span className="flex min-w-0 flex-col">
                <span className="text-body text-fg">{m.label}</span>
                <span className="text-caption text-fg-muted">{m.detail}</span>
              </span>
              {checked && <Check size={13} className="ms-auto mt-[3px] shrink-0 text-fg-muted" />}
            </CardMenuItem>
          );
        })}
      </div>
    </CardPopover>
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
  return (
    <CardPopover
      align="start"
      role="menu"
      label="Attach and more"
      tooltip="Attach and more"
      onKeyDown={menuKeys}
      className="min-w-[220px]"
      trigger={
        <button
          type="button"
          aria-label="Attach and more"
          className="flex size-7 items-center justify-center rounded-full bg-fill-subtle text-fg hover:bg-fill active:bg-fill-strong data-[state=open]:bg-fill-strong sm:size-6"
        >
          <Plus size={14} strokeWidth={2.25} />
        </button>
      }
    >
      <CardMenuItem onSelect={onAttach}>
        <Paperclip size={13} className="text-fg-muted" /> {attachLabel}
      </CardMenuItem>
      <div className="mx-1.5 my-1 h-px shrink-0 bg-border" />
      <CardMenuItem onSelect={() => startNewThread()}>
        <SquarePen size={13} className="text-fg-muted" /> New Thread
        <MenuShortcut>⌘N</MenuShortcut>
      </CardMenuItem>
      <CardMenuItem
        onSelect={() =>
          void pickFolder(cwd).then((p) => {
            if (!p) return;
            setProjectCwd(p);
            startNewThread(p);
          })
        }
      >
        <Folder size={13} className="text-fg-muted" /> New Thread in Folder…
      </CardMenuItem>
    </CardPopover>
  );
}
