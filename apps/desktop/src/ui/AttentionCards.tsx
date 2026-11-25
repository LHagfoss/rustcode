// What the thread is waiting on: an approval is a row in the transcript (ApprovalRow) saying what
// it wants, answered from one slim bar over the composer (ApprovalNotice), which owns the buttons
// and the keys (when no text field has focus): 1–9 pick an option in the engine's order reject → hour → always → allow once, and ⏎ approves
// while esc declines. An answered approval settles into one line where the bar was
// (ResolvedApprovalBar), so the answer has a visible result instead of the card vanishing.
// A question keeps its form in the bar, because the answer is a choice, and
// leaves a row in the transcript once it is answered (Rows.tsx QuestionRow). A request from a
// thread below this one (ChildRequestCard) uses the same cards, under a header naming it.

import { DropdownMenu } from "radix-ui";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import type { PermissionOption, PermissionRequest } from "../acp/types";
import {
  approvalPreview,
  approvalSubject,
  assessApproval,
  grantAllowed,
  optionScope,
  type PreviewRow,
  planTitle,
  proposedPlan,
  type RiskAssessment,
  showsRawInput,
} from "../lib/approval";
import { onFileDrop, pickFiles } from "../lib/native";
import { answerOptions, takesFiles } from "../lib/questionAnswer";
import type { Attachment } from "../state/compose";
import { type QuestionDraft, questionDraft, saveQuestionDraft } from "../state/questionDrafts";
import {
  answerPermission,
  answerQuestion,
  approveAll,
  type ResolvedApproval,
  selectThread,
  threadIsRemote,
  toast,
  useApp,
  useMachineOfThread,
} from "../state/store";
import { AttachmentStrip, attachmentsFromFiles, attachmentsFromPaths } from "./Attachments";
import { overQuestionDrop, QUESTION_DROP_ATTR } from "./folderDropTarget";
import {
  ArrowUpRight,
  CheckCheck,
  ChevronDown,
  CircleArrowUp,
  CircleCheck,
  Clock,
  CornerDownRight,
  Hand,
  ListChecks,
  MessageCircleQuestion,
  Paperclip,
  ShieldAlert,
} from "./icons";
import { CopyButton, Markdown } from "./Markdown";
import {
  cx,
  Kbd,
  MenuContent,
  MenuShortcut,
  menuItem,
  menuLabel,
  menuSeparator,
  noTextHelpers,
  Pill,
  Tooltip,
} from "./primitives";
import { DiffView } from "./Rows";
import { SecretCard } from "./SecretCard";

export function orderedOptions(options: PermissionOption[]): PermissionOption[] {
  const rank = (o: PermissionOption) =>
    o.kind === "reject_once" || o.kind === "reject_always" ? 0 : o.kind === "allow_once" ? 2 : 1;
  return [...options].sort((a, b) => rank(a) - rank(b));
}

function field(input: Json | undefined, key: string): string | undefined {
  if (!input || typeof input !== "object" || Array.isArray(input)) return undefined;
  const v = input[key];
  return typeof v === "string" ? v : undefined;
}

function useDigitKeys(count: number, pick: (index: number) => void) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, [contenteditable]") || e.metaKey || e.ctrlKey || e.altKey) return;
      const n = Number.parseInt(e.key, 10);
      if (n >= 1 && n <= count) {
        e.preventDefault();
        pick(n - 1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [count, pick]);
}

/** ⏎ approves and Esc declines, unless something else has the keyboard: a field, a menu or a dialog. */
function useApprovalKeys(decline?: () => void, approve?: () => void) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" && e.key !== "Enter") return;
      if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey || e.isComposing) return;
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, select, button, a, [contenteditable]")) return;
      if (document.querySelector('[role="menu"], [role="dialog"], [role="listbox"]')) return;
      const run = e.key === "Enter" ? approve : decline;
      if (!run) return;
      e.preventDefault();
      run();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [decline, approve]);
}

/**
 * The answers to one approval, ordered for the digit keys: the three that get a button of their
 * own (reject, approve once, and the grant menu for an hour or always), and whatever else the
 * engine offered (always-reject, "Allow all"). Shared by the timeline row and the bar over the
 * composer, so both offer the same answers in the same order.
 *
 * A destructive call offers no grant at all, whatever the engine listed: the answers are Approve
 * once and Decline, and a digit key cannot pick the hidden ones either.
 */
function useApproval(request: PermissionRequest, risk: RiskAssessment) {
  const options = useMemo(() => orderedOptions(request.options), [request.options]);
  const grants = grantAllowed(risk);
  const isGrant = (option: PermissionOption) =>
    optionScope(option) === "hour" || optionScope(option) === "always";
  const pick = (i: number) => {
    const option = options[i];
    if (!option || (!grants && isGrant(option))) return;
    answerPermission(request, option);
  };
  const byScope = (scope: ReturnType<typeof optionScope>) => options.find((o) => optionScope(o) === scope);
  const decline = byScope("decline");
  // Plan mode's answers are ways to start building (auto-accept edits, ask for each), not grants
  // for a call: they go in the menu under the engine's own names, never as "Always allow".
  const plan = proposedPlan(request) !== undefined;
  const hour = grants && !plan ? byScope("hour") : undefined;
  const always = grants && !plan ? byScope("always") : undefined;
  // Approve answers this call once; an engine that only offers a standing answer gets that.
  const allow = byScope("once") ?? always;
  const other = options.filter(
    (o) => o !== decline && o !== hour && o !== always && o !== allow && (grants || !isGrant(o)),
  );
  return { options, pick, decline, hour, always, allow, other, plan };
}

/**
 * The buttons either of them offers, at the timeline's 24px pill: Decline, Approve (once), and a
 * Grant menu that says how long an approval lasts — For 1 hour, Always — with the engine's other
 * answers and Allow all when the turn waits on a batch.
 */
function ApprovalActions({
  request,
  pending = 1,
  answers,
}: {
  request: PermissionRequest;
  pending?: number;
  answers: ReturnType<typeof useApproval>;
}) {
  const { options, pick, decline, allow, hour, always, other, plan } = answers;
  const grants = [hour, always].filter((o): o is PermissionOption => !!o);
  const menu = grants.length > 0 || other.length > 0 || pending > 1;
  const digit = (o: PermissionOption) => options.indexOf(o) + 1;
  return (
    <>
      {menu && (
        <DropdownMenu.Root modal={false}>
          <DropdownMenu.Trigger asChild>
            <Pill size="sm" quiet aria-label={plan ? "More ways to build" : "Allow for longer"}>
              {grants.length > 0 ? "Grant" : "More"} <ChevronDown size={12} />
            </Pill>
          </DropdownMenu.Trigger>
          <MenuContent side="top" align="end" className="max-w-[360px] min-w-[220px]">
            {grants.length > 0 && <DropdownMenu.Label className={menuLabel}>Allow for</DropdownMenu.Label>}
            {hour && (
              <DropdownMenu.Item onSelect={() => pick(options.indexOf(hour))} className={menuItem}>
                <Clock size={13} className="shrink-0 text-fg-muted" />
                <span className="min-w-0 flex-1">1 hour</span>
                <MenuShortcut>{digit(hour)}</MenuShortcut>
              </DropdownMenu.Item>
            )}
            {always && (
              <DropdownMenu.Item onSelect={() => pick(options.indexOf(always))} className={menuItem}>
                <CheckCheck size={13} className="shrink-0 text-fg-muted" />
                <span className="min-w-0 flex-1">Always allow</span>
                <MenuShortcut>{digit(always)}</MenuShortcut>
              </DropdownMenu.Item>
            )}
            {grants.length > 0 && (other.length > 0 || pending > 1) && (
              <DropdownMenu.Separator className={menuSeparator} />
            )}
            {other.map((o) => (
              <DropdownMenu.Item key={o.id} onSelect={() => pick(options.indexOf(o))} className={menuItem}>
                <span className="min-w-0 flex-1 break-words">{o.name}</span>
                <MenuShortcut>{digit(o)}</MenuShortcut>
              </DropdownMenu.Item>
            ))}
            {pending > 1 && (
              <DropdownMenu.Item onSelect={() => approveAll(request.sessionId)} className={menuItem}>
                <CheckCheck size={13} /> Allow all {pending} waiting
              </DropdownMenu.Item>
            )}
          </MenuContent>
        </DropdownMenu.Root>
      )}
      {decline && (
        <Tooltip label={`${decline.name} (${digit(decline)})`}>
          <Pill size="sm" onClick={() => pick(options.indexOf(decline))}>
            {plan ? "Keep planning" : "Decline"} <Kbd>esc</Kbd>
          </Pill>
        </Tooltip>
      )}
      {allow && (
        <Tooltip label={`${allow.name} (${digit(allow)})`}>
          <Pill size="sm" prominent onClick={() => pick(options.indexOf(allow))}>
            {plan ? "Build" : "Approve"} <Kbd>⏎</Kbd>
          </Pill>
        </Tooltip>
      )}
    </>
  );
}

/** The tint a risk level gives the card: the icon, and a small badge naming it. */
const RISK_TONE: Record<RiskAssessment["level"], { icon: string; badge: string }> = {
  safe: { icon: "text-success", badge: "bg-success/12 text-success" },
  risky: { icon: "text-warning", badge: "bg-warning/12 text-warning" },
  destructive: { icon: "text-danger", badge: "bg-danger/12 text-danger" },
};

function RiskBadge({ risk }: { risk: RiskAssessment }) {
  return (
    <span
      className={cx("shrink-0 rounded-xs px-1.5 font-medium text-caption", RISK_TONE[risk.level].badge)}
      title={risk.why}
    >
      {risk.label}
    </span>
  );
}

/** The key–value facts of a request: command, folder, paths and an edit's size. */
function Preview({ rows }: { rows: PreviewRow[] }) {
  return (
    <div className="flex flex-col gap-1 rounded-md bg-fill-subtle p-2">
      {rows.map((row) => (
        <div key={`${row.label}:${row.value}`} className="flex gap-2">
          <span className="w-[58px] shrink-0 pt-px text-caption text-fg-muted">{row.label}</span>
          <span
            className={cx(
              "selectable min-w-0 flex-1 text-callout",
              row.mono && "font-mono",
              row.wrap ? "whitespace-pre-wrap break-words" : "truncate",
            )}
          >
            {row.value}
          </span>
        </div>
      ))}
    </div>
  );
}

/** How tall a proposed plan reads before it folds: most plans fit, a long one is a click away. */
const PLAN_SHOWN = 360;

/**
 * Plan mode's proposal, as the plan it is: its Markdown rendered, Copy, and the whole of it a click
 * away when it runs past the fold. It replaces the raw `{"plan": …}` an unknown tool would show.
 */
function PlanProposal({ plan }: { plan: string }) {
  const [open, setOpen] = useState(false);
  const long = plan.split("\n").length > 18 || plan.length > 1400;
  return (
    <div className="flex flex-col gap-1.5 rounded-md bg-fill-subtle p-3" data-testid="proposed-plan">
      <div
        className={cx("selectable min-w-0", !open && long && "overflow-hidden")}
        style={open || !long ? undefined : { maxHeight: PLAN_SHOWN }}
      >
        <Markdown text={plan} />
      </div>
      <div className="flex items-center justify-end gap-1">
        {long && (
          <Pill size="sm" quiet onClick={() => setOpen(!open)}>
            {open ? "Collapse" : "Show the whole plan"}
          </Pill>
        )}
        <CopyButton text={plan} iconOnly label="Copy plan" />
      </div>
    </div>
  );
}

/**
 * An approval as a row in the transcript, at the call it is about: how risky it is, then the
 * key–value facts of exactly what it would run — command, paths, folder, and an edit's size —
 * and any change it wants to make (at most 160px, then it scrolls). The bar over the composer
 * answers the first one waiting, so only a later approval, which the bar does not show yet, has
 * its own buttons here.
 */
export function ApprovalRow({
  request,
  pending = 1,
  number = 1,
  project = "",
}: {
  request: PermissionRequest;
  pending?: number;
  /** Which of the waiting approvals this is, from 1. */
  number?: number;
  /** The thread's folder: a path outside it is called out as destructive. */
  project?: string;
}) {
  const risk = assessApproval(request, project);
  const answers = useApproval(request, risk);
  const inBar = number === 1;
  const rows = approvalPreview(request);
  const note = field(request.input, "reason");
  const tone = RISK_TONE[risk.level];
  const plan = proposedPlan(request);
  if (plan !== undefined) {
    return (
      <div className="flex flex-col gap-1.5" role="group" aria-label="Plan ready">
        <div className="flex items-center gap-2">
          <span className="flex w-[18px] shrink-0 justify-center">
            <ListChecks size={16} className="text-accent" />
          </span>
          <span className="shrink-0 font-medium text-accent text-body">Plan ready</span>
          <span className="min-w-0 truncate text-callout text-fg-muted">{planTitle(plan)}</span>
          {pending > 1 && (
            <span className="shrink-0 text-caption text-fg-muted tabular">
              {number} of {pending}
            </span>
          )}
        </div>
        <div className="ml-[26px]">
          <PlanProposal plan={plan} />
        </div>
        {!inBar && (
          <div className="ml-[26px] flex items-center gap-1.5">
            <ApprovalActions request={request} pending={pending} answers={answers} />
          </div>
        )}
      </div>
    );
  }
  // For a command the block is useful; an edit's diff header already names the file.
  const showBody = rows.length > 0 || request.diffs.length > 0;
  const showInput = rows.length === 0 && showsRawInput(request);
  return (
    <div className="flex flex-col gap-1.5" role="group" aria-label="Approval needed">
      <div className="flex items-center gap-2">
        <span className="flex w-[18px] shrink-0 justify-center">
          <Hand size={16} className={tone.icon} fill="currentColor" fillOpacity={0.2} />
        </span>
        <span className="shrink-0 font-medium text-body text-warning">Waiting for approval</span>
        <RiskBadge risk={risk} />
        <span className="min-w-0 truncate font-mono text-callout text-fg-muted" title={request.title}>
          {approvalSubject(request)}
        </span>
        {pending > 1 && (
          <span className="shrink-0 text-caption text-fg-muted tabular">
            {number} of {pending}
          </span>
        )}
      </div>
      {(showBody || showInput) && (
        // One body, at most 160px: what it would run and the change it would make, then it scrolls.
        <div className="ml-[26px] flex max-h-[160px] flex-col gap-2 overflow-y-auto">
          {rows.length > 0 && <Preview rows={rows} />}
          {showInput && (
            <pre className="selectable m-0 rounded-md bg-fill-subtle p-2 font-mono text-caption">
              {JSON.stringify(request.input, null, 2)}
            </pre>
          )}
          {request.diffs.map((diff, i) => (
            <DiffView key={i} diff={diff} />
          ))}
        </div>
      )}
      {(note || risk.why) && (
        <div className={cx("ml-[26px] flex items-center gap-1.5 text-callout", tone.icon)}>
          <ShieldAlert size={13} className="shrink-0" />
          <span className="min-w-0 truncate">{note ?? risk.why}</span>
        </div>
      )}
      {!inBar && (
        <div className="ml-[26px] flex items-center gap-1.5">
          <ApprovalActions request={request} pending={pending} answers={answers} />
        </div>
      )}
    </div>
  );
}

/**
 * The bar over the composer: one slim line naming the approval that waits, with its answers. It is
 * also the keyboard target (1–9, ⏎, esc), so it can be answered from anywhere in the thread.
 */
export function ApprovalNotice({
  request,
  pending = 1,
  number = 1,
  from,
  keys = true,
  project = "",
}: {
  request: PermissionRequest;
  pending?: number;
  /** Which of `pending` approvals this is, from 1. */
  number?: number;
  /** The thread asking, when it is a thread this one started and not this one. */
  from?: string;
  /** Whether 1–9, ⏎ and esc answer it: only the first card on screen takes them. */
  keys?: boolean;
  /** The thread's folder: a path outside it is called out as destructive. */
  project?: string;
}) {
  const risk = assessApproval(request, project);
  const answers = useApproval(request, risk);
  const { options, pick, decline, allow } = answers;
  const tone = RISK_TONE[risk.level];
  const plan = proposedPlan(request);
  useDigitKeys(keys ? options.length : 0, pick);
  useApprovalKeys(
    keys && decline ? () => pick(options.indexOf(decline)) : undefined,
    keys && allow ? () => pick(options.indexOf(allow)) : undefined,
  );
  return (
    <section
      aria-label="Approval needed"
      className="elevated flex min-h-[34px] items-center gap-2 rounded-2xl px-2 py-1 rc-in"
    >
      {plan !== undefined ? (
        <ListChecks size={14} className="ml-1 shrink-0 text-accent" aria-hidden />
      ) : (
        <Tooltip label={`${risk.label}${risk.why ? `: ${risk.why}` : ""}`}>
          <Hand size={14} className={cx("ml-1 shrink-0", tone.icon)} fill="currentColor" fillOpacity={0.2} />
        </Tooltip>
      )}
      {from && <span className="max-w-[30%] shrink-0 truncate text-callout text-fg-muted">From {from}</span>}
      {plan !== undefined ? (
        // Plan mode asks whether to start building, not whether a tool may run.
        <span className="min-w-0 flex-1 truncate text-callout text-fg-muted">
          Plan ready
          {planTitle(plan) && (
            <>
              {": "}
              <span className="font-medium text-body text-fg">{planTitle(plan)}</span>
            </>
          )}
          . Start building?
        </span>
      ) : (
        <span className="min-w-0 flex-1 truncate text-callout text-fg-muted">
          Allow{" "}
          <span className="font-medium text-body text-fg" title={request.title}>
            {approvalSubject(request)}
          </span>
          ?
        </span>
      )}
      {plan === undefined && <RiskBadge risk={risk} />}
      {pending > 1 && (
        <span className="shrink-0 text-caption text-fg-muted tabular">
          {number} of {pending}
        </span>
      )}
      <span className="flex shrink-0 items-center gap-1">
        <ApprovalActions request={request} pending={pending} answers={answers} />
      </span>
    </section>
  );
}

/**
 * What an answered approval leaves where its card was: one line, tinted by
 * whether it was allowed or declined, held for a moment (state/store.ts
 * `RESOLVED_APPROVAL_MS`) so the answer has a visible result.
 */
export function ResolvedApprovalBar({ approval }: { approval: ResolvedApproval }) {
  return (
    <section
      aria-label="Answer sent"
      className="elevated rc-resolve flex min-h-[34px] items-center gap-2 rounded-2xl px-2 py-1"
    >
      <span className="ml-1 flex w-[14px] shrink-0 justify-center">
        {approval.allowed ? (
          <CircleCheck size={14} className="text-success" />
        ) : (
          <Hand size={14} className="text-fg-muted" />
        )}
      </span>
      <span className="min-w-0 flex-1 truncate text-callout text-fg-muted">{approval.text}</span>
    </section>
  );
}

/** How tall a question's own text reads before it scrolls, so a long one leaves its answers in view. */
const QUESTION_TEXT_MAX = 168;

/** The composer's field, where focus goes back once a question is answered from its form. */
function focusComposerField() {
  document.querySelector<HTMLTextAreaElement>('[data-rc-composer] textarea[aria-label="Message"]')?.focus();
}

/**
 * A question over the composer, in place of nothing it covers: the composer stays below with its
 * draft, so the thread's context and the user's own words are where they were (issue 3696). It
 * says which of a set it is, the question (scrolling past `QUESTION_TEXT_MAX`), its answers with
 * their descriptions, and a field for the user's own answer. ↑/↓ move between answers, 1–9 pick
 * one, esc declines when no field has focus.
 *
 * A question that takes the user's own answer also takes files (issue 3756): Attach files, or a
 * paste or drop on the form, under the composer's own limits (`ui/Attachments.tsx`). A file goes
 * alone, beside the answer picked, or beside the words written; each question keeps its own
 * (`state/questionDrafts.ts`), and a send that fails keeps them.
 */
export function QuestionCard({
  request,
  from,
  keys = true,
}: {
  request: PermissionRequest;
  from?: string;
  /** Whether 1–9 and esc answer it: only the first card on screen takes them. */
  keys?: boolean;
}) {
  const q = request.question!;
  const answers = answerOptions(request);
  const reject = request.options.find((o) => o.kind === "reject_once" || o.kind === "reject_always");
  const [draft, setDraft] = useState<QuestionDraft>(() => questionDraft(request));
  const { ticked, text, attachments } = draft;
  const update = (change: (d: QuestionDraft) => QuestionDraft) =>
    setDraft((d) => {
      const next = change(d);
      saveQuestionDraft(request, next);
      return next;
    });
  const attach = (more: Attachment[]) => {
    if (more.length) update((d) => ({ ...d, attachments: [...d.attachments, ...more] }));
  };
  const remote = threadIsRemote(request.sessionId);
  const machine = useMachineOfThread(request.sessionId);
  const cwd = useApp((s) => s.threads[request.sessionId]?.cwd);
  const waiting = useApp(
    (s) => s.threads[request.sessionId]?.permissions.filter((p) => p.question).length ?? 1,
  );
  const files = takesFiles(request);
  // A question that takes only its own choices has no field, and so no files either.
  const ownAnswer = q.ownAnswer !== false;
  // A file picked here is sent by its path, which a thread on another computer cannot open: there
  // only what the form reads (an image, a small file) goes.
  const reachable = (more: Attachment[]) =>
    more.filter((a) => {
      if (!remote || a.kind !== "file" || !a.path) return true;
      toast(`${a.name} is on this computer; ${machine ?? "the thread's computer"} cannot open it`, "info");
      return false;
    });
  const fromFiles = (list: File[]) => {
    if (!files) return;
    void attachmentsFromFiles(list, true, "").then(attach);
  };
  const fromPaths = (paths: string[]) => {
    if (!files) return;
    void attachmentsFromPaths(paths, true, "").then((more) => attach(reachable(more)));
  };
  const fileInput = useRef<HTMLInputElement>(null);
  const card = useRef<HTMLElement>(null);
  const textId = useId();
  const headId = useId();
  const [dropping, setDropping] = useState(false);

  const send = (options: PermissionOption[], words = "") => {
    const within = card.current?.contains(document.activeElement);
    if (!answerQuestion(request, { options, text: words, attachments })) return;
    if (within) requestAnimationFrame(focusComposerField);
  };
  useDigitKeys(keys && !q.multiple ? answers.length : 0, (i) => {
    const option = answers[i];
    if (option) send([option]);
  });
  useApprovalKeys(keys && reject ? () => answerPermission(request, reject) : undefined);

  // Files dropped on the window arrive as paths from the shell; those over this form are its own.
  const dropped = useRef(fromPaths);
  dropped.current = fromPaths;
  useEffect(() => {
    if (!files) return;
    let stop: (() => void) | undefined;
    let live = true;
    void onFileDrop((event) => {
      const over = !!event.position && overQuestionDrop(event.position);
      setDropping(event.type === "over" && over);
      if (event.type === "drop" && over && event.paths.length) dropped.current(event.paths);
    }).then((unlisten) => {
      if (live) stop = unlisten;
      else unlisten();
    });
    return () => {
      live = false;
      stop?.();
    };
  }, [files]);

  // The form takes the keyboard when it appears, unless the user is typing somewhere already.
  useEffect(() => {
    if (!keys) return;
    const active = document.activeElement;
    if (active && active !== document.body && active.closest("input, textarea, [contenteditable]")) return;
    // The panel itself, not its first answer: ⏎ or space then cannot send an answer nobody chose.
    card.current?.focus({ preventScroll: true });
  }, [keys]);

  /** ↑/↓ between the answers (from the panel, into the first), Home and End to the ends. */
  const onAnswersKey = (e: React.KeyboardEvent<HTMLElement>) => {
    const target = e.target as HTMLElement;
    if (target !== e.currentTarget && !target.hasAttribute("data-question-answer")) return;
    const items = [...e.currentTarget.querySelectorAll<HTMLElement>("[data-question-answer]")];
    const at = items.indexOf(document.activeElement as HTMLElement);
    const next =
      e.key === "ArrowDown"
        ? at + 1
        : e.key === "ArrowUp"
          ? at - 1
          : e.key === "Home"
            ? 0
            : e.key === "End"
              ? items.length - 1
              : null;
    if (next === null || !items.length) return;
    e.preventDefault();
    items[Math.max(0, Math.min(items.length - 1, next))]?.focus();
  };

  const progress =
    q.step && q.steps ? `${q.step} of ${q.steps}` : waiting > 1 ? `${waiting} waiting` : undefined;
  const chooseFiles = () => {
    void pickFiles(cwd).then((paths) => {
      if (paths === null) fileInput.current?.click();
      else fromPaths(paths);
    });
  };
  return (
    <section
      ref={card}
      tabIndex={-1}
      onKeyDown={onAnswersKey}
      aria-labelledby={headId}
      aria-describedby={textId}
      {...(files ? { [QUESTION_DROP_ATTR]: "" } : {})}
      className={cx(
        "rc-composer-card flex flex-col gap-3 px-5 py-4 outline-none rc-in",
        dropping && "outline-2 outline-accent/60 outline-dashed -outline-offset-4",
      )}
      onDragOver={(e) => {
        if (files && e.dataTransfer.types.includes("Files")) {
          e.preventDefault();
          setDropping(true);
        }
      }}
      onDragLeave={() => setDropping(false)}
      onDrop={(e) => {
        if (!files || !e.dataTransfer.files.length) return;
        e.preventDefault();
        setDropping(false);
        fromFiles([...e.dataTransfer.files]);
      }}
    >
      <div className="flex min-w-0 items-center gap-2">
        <MessageCircleQuestion size={16} className="shrink-0 text-accent" aria-hidden />
        <h2 id={headId} className="m-0 min-w-0 flex-1 truncate font-semibold text-accent text-body">
          {from ? `From ${from}: ` : ""}
          {q.header || "RustCode has a question"}
        </h2>
        {progress && <span className="shrink-0 text-caption text-fg-muted tabular">{progress}</span>}
      </div>
      <div
        id={textId}
        className="selectable overflow-y-auto whitespace-pre-wrap break-words text-body"
        style={{ maxHeight: QUESTION_TEXT_MAX }}
      >
        {q.text}
      </div>
      <div
        role="group"
        aria-label="Answers"
        className={cx("flex flex-col", q.multiple ? "gap-1.5" : "gap-1")}
      >
        {answers.map((o, i) =>
          q.multiple ? (
            <label
              key={o.id}
              className="flex items-start gap-2 rounded-lg px-1 py-0.5 has-[:focus-visible]:bg-fill-subtle"
            >
              <input
                type="checkbox"
                data-question-answer=""
                className="mt-0.5 accent-[var(--rc-accent)]"
                checked={ticked.includes(o.id)}
                onChange={(e) =>
                  update((d) => ({
                    ...d,
                    ticked: e.target.checked ? [...d.ticked, o.id] : d.ticked.filter((x) => x !== o.id),
                  }))
                }
              />
              <span className="min-w-0">
                <span className="block break-words text-body">{o.name}</span>
                {q.descriptions[i] && (
                  <span className="block break-words text-caption text-fg-muted">{q.descriptions[i]}</span>
                )}
              </span>
            </label>
          ) : (
            <button
              key={o.id}
              type="button"
              data-question-answer=""
              onClick={() => send([o])}
              aria-keyshortcuts={keys && i < 9 ? String(i + 1) : undefined}
              aria-description={
                attachments.length
                  ? `Sends ${attachments.length === 1 ? "the file" : "the files"} with it`
                  : undefined
              }
              className="group flex w-full min-w-0 items-center gap-2 rounded-lg bg-fill-subtle px-3 py-[9px] text-left hover:bg-accent/14 focus-visible:bg-accent/14"
            >
              {i < 9 && <Kbd>{i + 1}</Kbd>}
              <span className="min-w-0 flex-1">
                <span className="block break-words text-body">{o.name}</span>
                {q.descriptions[i] && (
                  <span className="block break-words text-caption text-fg-muted">{q.descriptions[i]}</span>
                )}
              </span>
              <CircleArrowUp
                size={16}
                aria-hidden
                className="shrink-0 text-accent opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
              />
            </button>
          ),
        )}
      </div>
      {q.multiple && (
        <div className="flex justify-end">
          <Pill
            prominent
            disabled={!ticked.length}
            onClick={() => send(answers.filter((o) => ticked.includes(o.id)))}
          >
            Submit
          </Pill>
        </div>
      )}
      {attachments.length > 0 && (
        <AttachmentStrip
          attachments={attachments}
          className="-my-1 py-1"
          onRemove={(id) => update((d) => ({ ...d, attachments: d.attachments.filter((a) => a.id !== id) }))}
        />
      )}
      {ownAnswer ? (
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            send([], text);
          }}
        >
          <input
            value={text}
            onChange={(e) => update((d) => ({ ...d, text: e.target.value }))}
            onPaste={(e) => {
              const pasted = [...e.clipboardData.files];
              if (!pasted.length || !files) return;
              e.preventDefault();
              fromFiles(pasted);
            }}
            onKeyDown={(e) => {
              // Esc leaves the field rather than dismissing the question from inside it.
              if (e.key === "Escape") e.currentTarget.blur();
            }}
            placeholder={files ? "Or write your own answer, or attach files…" : "Or write your own answer…"}
            aria-label="Your own answer"
            {...noTextHelpers}
            className="h-[30px] min-w-[160px] flex-1 rounded-full bg-fill-subtle px-3 text-body outline-none placeholder:text-fg-faint hover:bg-fill focus:bg-fill"
          />
          <span className="flex shrink-0 items-center gap-2">
            {files ? (
              <Tooltip label="Attach files or images">
                <button
                  type="button"
                  aria-label="Attach files"
                  onClick={chooseFiles}
                  className="inline-flex size-[30px] items-center justify-center rounded-full text-fg-muted hover:bg-fill hover:text-fg"
                >
                  <Paperclip size={14} />
                </button>
              </Tooltip>
            ) : (
              remote && (
                <Tooltip
                  label={`Update RustCode on ${machine ?? "that computer"} to attach files to an answer`}
                >
                  <span
                    role="img"
                    aria-label="Attaching files needs a newer RustCode there"
                    className="inline-flex size-[30px] items-center justify-center text-fg-faint"
                  >
                    <Paperclip size={14} />
                  </span>
                </Tooltip>
              )
            )}
            <Pill prominent type="submit" disabled={!text.trim() && !attachments.length}>
              Send
            </Pill>
            {reject && (
              <Tooltip label={keys ? "Decline the question (esc)" : "Decline the question"}>
                <Pill onClick={() => answerPermission(request, reject)}>{reject.name}</Pill>
              </Tooltip>
            )}
          </span>
          {files && (
            <input
              ref={fileInput}
              type="file"
              multiple
              hidden
              onChange={(e) => {
                const list = [...(e.target.files ?? [])];
                e.target.value = "";
                fromFiles(list);
              }}
            />
          )}
        </form>
      ) : (
        reject && (
          <div className="flex justify-end">
            <Pill onClick={() => answerPermission(request, reject)}>{reject.name}</Pill>
          </div>
        )
      )}
    </section>
  );
}

export function AttentionCard({
  request,
  pending = 1,
  number = 1,
  from,
  keys = true,
  project = "",
}: {
  request: PermissionRequest;
  pending?: number;
  number?: number;
  from?: string;
  keys?: boolean;
  /** The thread's folder, so a path outside it is called out. */
  project?: string;
}) {
  return request.secret ? (
    <SecretCard key={`${request.rpcId}`} request={request} from={from} />
  ) : request.question ? (
    <QuestionCard key={`${request.rpcId}`} request={request} from={from} keys={keys} />
  ) : (
    <ApprovalNotice
      key={`${request.rpcId}`}
      request={request}
      pending={pending}
      number={number}
      from={from}
      keys={keys}
      project={project}
    />
  );
}

/**
 * A request from a thread this one started, at the transcript's foot: a small
 * header naming the thread (a click opens it), then the same card that thread's
 * own chat shows. Its answer goes through that card's own function, so the two
 * places cannot answer differently.
 */
export function ChildRequestCard({
  request,
  from,
  pending = 1,
  number = 1,
  keys = false,
  project = "",
}: {
  request: PermissionRequest;
  from: string;
  pending?: number;
  number?: number;
  /** Only the first card on screen takes the digit and ⏎/esc keys. */
  keys?: boolean;
  /** The thread's folder, so a path outside it is called out. */
  project?: string;
}) {
  return (
    <div
      className="flex flex-col gap-1.5"
      role="group"
      aria-label={`${request.question ? "Question" : "Approval"} from ${from}`}
    >
      <button
        type="button"
        onClick={() => selectThread(request.sessionId)}
        aria-label={`Open ${from}`}
        className="group flex w-full min-w-0 items-center gap-1.5 rounded-sm px-0.5 py-0.5 text-left text-callout text-fg-muted hover:bg-fill-subtle hover:text-fg"
      >
        <CornerDownRight size={12} className="shrink-0 text-fg-faint" />
        <span className="min-w-0 truncate">
          From <span className="font-medium text-fg">{from}</span>
        </span>
        <ArrowUpRight
          size={12}
          className="shrink-0 text-fg-faint opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
        />
      </button>
      <AttentionCard request={request} pending={pending} number={number} keys={keys} project={project} />
    </div>
  );
}
