// What a message is made of before it is sent: the typed text, an optional
// slash-command token, pasted link chips, thread and harness mentions, long
// pastes held above the field, images and files. A paste goes back into the
// text where it was pasted when the message is composed, so it is sent as plain
// text, not as a chip; and `promptBlocks` turns the rest into ACP content
// blocks. No React and no I/O, so tests drive it.

import type { Json, JsonObject } from "../acp/jsonrpc";
import type { SlashCommand } from "../acp/types";
import { fuzzyScore } from "../lib/modelSearch";

/**
 * A message that starts with a command (`/review the diff`), as the name and what follows. A path
 * (`/usr/bin/env`) is not a command: the name stops at a space.
 */
export function splitSlashCommand(text: string): { command: string; rest: string } | null {
  const match = /^(\/[A-Za-z][\w:-]*)(?:\s+|$)/.exec(text);
  return match ? { command: match[1]!, rest: text.slice(match[0].length) } : null;
}

/**
 * A word being typed after its trigger: where it starts, where it ends, and what
 * has been typed of it. `end` is the caret's own word end, which a narrowed `@`
 * token (`@agents pi`) reaches past the trigger's word.
 */
export interface TokenRange {
  start: number;
  end: number;
  query: string;
}

/**
 * The `/word` the caret is in, when the `/` opens a word: at the start of the
 * message or just after whitespace. A path is not a command, so a second slash
 * inside the word (`/usr/bin`) closes the token, and so does a space.
 */
export function slashToken(text: string, caret: number): TokenRange | null {
  const before = text.slice(0, caret);
  const start = before.lastIndexOf("/");
  if (start < 0) return null;
  if (start > 0 && !/\s/.test(before[start - 1] ?? "")) return null;
  const query = before.slice(start + 1);
  if (/\s/.test(query)) return null;
  // The caret mid-word: the rest of the word is part of what gets replaced.
  if (/^\S/.test(text.slice(caret))) return null;
  return { start, end: caret, query };
}

/**
 * The commands the token `query` is looking for: names that start with it first,
 * then the ones with it anywhere, so `/rev` finds `/review` and `/release`.
 */
export function commandSuggestions(query: string, commands: SlashCommand[], limit = 40): SlashCommand[] {
  const typed = `/${query.toLowerCase()}`;
  const prefixed = commands.filter((c) => c.name.toLowerCase().startsWith(typed));
  const others =
    typed.length > 1
      ? commands.filter((c) => !prefixed.includes(c) && c.name.toLowerCase().includes(typed.slice(1)))
      : [];
  return [...prefixed, ...others].slice(0, limit);
}

/** The text with the token replaced by `/name `, and where the caret goes. */
export function insertCommand(
  text: string,
  token: TokenRange,
  command: string,
): { text: string; caret: number } {
  const ref = `${command} `;
  const rest = text.slice(token.end).replace(/^ /, "");
  return { text: text.slice(0, token.start) + ref + rest, caret: token.start + ref.length };
}

/** A message folds under this height (px)... */
export const COLLAPSED_MESSAGE_HEIGHT = 176;
/** ...only when folding hides more than this: three lines. Hiding one line behind a button helps nobody. */
const COLLAPSE_SLACK = 66;

/** Whether a message this tall folds. By how tall it draws, so a fold always opens to something. */
export const messageCollapses = (height: number) => height > COLLAPSED_MESSAGE_HEIGHT + COLLAPSE_SLACK;

/** The engine refuses larger images (`ImageAttachment.maxBytes` in the SwiftUI app). */
export const MAX_IMAGE_BYTES = 20 * 1024 * 1024;
/** A file without a path (the browser) is sent inline only up to this size. */
export const MAX_INLINE_FILE_BYTES = 200 * 1024;

export const IMAGE_TYPES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
};

export function imageMime(name: string): string | undefined {
  return IMAGE_TYPES[name.split(".").pop()?.toLowerCase() ?? ""];
}

export type Attachment =
  | { id: string; kind: "image"; name: string; mimeType: string; data: string }
  | { id: string; kind: "file"; name: string; path?: string; text?: string }
  /**
   * A long paste the field kept out of the draft (lib/paste.ts). `at` is the caret offset it was
   * pasted at, kept right while the draft changes (`trackPastes`), so the sent text can put it back
   * where the user wrote it.
   */
  | { id: string; kind: "text"; text: string; at: number };

// ---------------------------------------------------------------- mentions

/**
 * Something a message points at besides its words: another thread (`#Title`), or the harness, model
 * and effort an agent should delegate to (`@pi/deepseek/deepseek-flash`). The draft keeps the
 * short form the reader picks, and the message spells it out (`composedText`), so the agent can act
 * on it: a thread id for `read_session`, or what `start_session` takes. The id is what makes it
 * unambiguous: two threads may share a title, and the list of mentions carries which one was picked.
 */
export type Mention =
  | { kind: "thread"; id: string; title: string }
  | {
      kind: "agent";
      /** The harness that would run the delegate. */
      harness: string;
      /** The model it would start on, when the row named one. */
      model?: string;
      /** The effort it would start on. */
      effort?: string;
      /** What the draft says after the `@`: `pi/deepseek/deepseek-flash`. */
      token: string;
    };

/** What a mention reads as in the draft, and what the mirror draws on its pill: `#Title`, `@codex`. */
export function mentionToken(mention: Mention): string {
  return mention.kind === "thread" ? `#${mention.title}` : `@${mention.token}`;
}

/** A link's label cannot hold an unescaped bracket: `#[a]b](…)` would end at the first `]`. */
const escapeLabel = (title: string) => title.replace(/[\\[\]]/g, (c) => `\\${c}`);

/**
 * What a mention says in the sent message: a link around the title, so the agent reads the thread's
 * id and can open it; or the harness, model and effort of the delegate, as `start_session` takes
 * them, spelled out after the token the drawer wrote.
 */
export function mentionText(mention: Mention): string {
  if (mention.kind === "thread") return `#[${escapeLabel(mention.title)}](rustcode://thread/${mention.id})`;
  const delegation = [
    `harness=${mention.harness}`,
    ...(mention.model ? [`model=${mention.model}`] : []),
    ...(mention.effort ? [`effort=${mention.effort}`] : []),
  ].join(", ");
  return `@${mention.token} (delegate: ${delegation})`;
}

/** One mention written out in the draft, as the text and what it stands for. */
export interface MentionRun {
  mention: Mention;
  /** The draft's own characters: `#Title`, `@codex`, exactly as the field holds them. */
  raw: string;
}

/**
 * Whether the character at `at` sits in code: inside a fenced block (``` or ~~~) opened above it,
 * or between the backticks of an inline span on its own line. A `#` there is text being written,
 * not a mention: a note about an issue number is not a thread.
 */
export function inCode(text: string, at: number): boolean {
  const lines = text.slice(0, at).split("\n");
  const line = lines.pop() ?? "";
  if (lines.filter((l) => /^\s*(```|~~~)/.test(l)).length % 2 === 1) return true;
  return (line.match(/`/g)?.length ?? 0) % 2 === 1;
}

/**
 * The `#word` the caret is in: a thread mention. The `#` opens a word — at the start of the message
 * or just after whitespace, so `abc#x` is not one — and an empty one opens the list. A markdown
 * heading is not one either: its `#` is followed by a space. Nor is a `#` in code (see `inCode`),
 * or one inside a token already written (`#[Title]`).
 */
export function hashToken(text: string, caret: number): TokenRange | null {
  const before = text.slice(0, caret);
  const start = before.lastIndexOf("#");
  if (start < 0) return null;
  if (start > 0 && !/\s/.test(before[start - 1] ?? "")) return null;
  const query = before.slice(start + 1);
  if (/\s/.test(query)) return null;
  // A token already written, or a title being edited inside one: the drawer has no business there.
  if (/[[\]]/.test(query)) return null;
  // The caret mid-word: the rest of the word is part of what gets replaced.
  if (/^\S/.test(text.slice(caret))) return null;
  if (inCode(text, start)) return null;
  return { start, end: caret, query };
}

/** The mention whose own token starts at `start`, the longest first, or undefined. */
function mentionAt(
  text: string,
  start: number,
  mentions: readonly Mention[],
  written: readonly number[],
): number | undefined {
  let best: number | undefined;
  for (let i = 0; i < mentions.length; i++) {
    const token = mentionToken(mentions[i]!);
    if (!text.startsWith(token, start)) continue;
    // A token that runs into a word is a different word: `#Fix` is not `#Fixer`.
    if (/[\w-]/.test(text[start + token.length] ?? "")) continue;
    if (best === undefined) {
      best = i;
      continue;
    }
    const length = mentionToken(mentions[best]!).length;
    // The longest token wins, so `#Fix` inside `#Fix the tests` is not a second mention; of two the
    // same length (the same title in two projects), the one written least often so far goes next.
    if (token.length > length || (token.length === length && written[i]! < written[best]!)) best = i;
  }
  return best;
}

/**
 * The text as runs of plain words and the mentions in it, in the order they were typed. A mention
 * counts where its token opens a word — the start of the text or after whitespace — and where it
 * does not run into a longer word. The mirror draws each of these on a pill, and `composedText`
 * writes each one out in full.
 */
export function mentionRuns(text: string, mentions: readonly Mention[]): (string | MentionRun)[] {
  if (!mentions.length) return text ? [text] : [];
  const written = mentions.map(() => 0);
  const out: (string | MentionRun)[] = [];
  let last = 0;
  let at = 0;
  while (at < text.length) {
    const c = text[at];
    if ((c === "#" || c === "@") && (at === 0 || /\s/.test(text[at - 1] ?? ""))) {
      const found = mentionAt(text, at, mentions, written);
      if (found !== undefined) {
        const mention = mentions[found]!;
        const raw = mentionToken(mention);
        written[found] = written[found]! + 1;
        if (at > last) out.push(text.slice(last, at));
        out.push({ mention, raw });
        at += raw.length;
        last = at;
        continue;
      }
    }
    at += 1;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/**
 * The mentions the draft still writes, in the order it writes them: a token the user deleted leaves
 * nothing to expand, so its registration goes with it. The same token twice keeps two registrations,
 * because two threads may share a title (a `#` token says only the title).
 */
export function pruneMentions(text: string, mentions: readonly Mention[] | undefined): Mention[] {
  if (!mentions?.length) return [];
  return mentionRuns(text, mentions).flatMap((run) => (typeof run === "string" ? [] : [run.mention]));
}

/** The message's words for each mention in it; a mention no longer in the draft is left out. */
export function serializeMentions(text: string, mentions: readonly Mention[] | undefined): string {
  if (!mentions?.length) return text;
  return mentionRuns(text, mentions)
    .map((run) => (typeof run === "string" ? run : mentionText(run.mention)))
    .join("");
}

/** The text with the `#` token replaced by the mention's own words, and where the caret goes. */
export function insertMention(
  text: string,
  token: TokenRange,
  mention: Mention,
): { text: string; caret: number } {
  const ref = `${mentionToken(mention)} `;
  const rest = text.slice(token.end).replace(/^ /, "");
  return { text: text.slice(0, token.start) + ref + rest, caret: token.start + ref.length };
}

/** How a thread is doing, as the `#` list orders it and says it: what asks or works comes first. */
export type MentionState = "needs" | "working" | "open" | "settled";

/** A thread the `#` list offers. */
export interface ThreadChoice {
  id: string;
  title: string;
  /** The harness it runs on, for the row's mark. */
  harness?: string;
  /** The project it lives in, when it is not this one's. */
  project?: string;
  /** It runs in the folder the message is written in. */
  home?: boolean;
  state: MentionState;
  updatedAt?: number;
  /** It works for the thread the message is written in. */
  delegate?: boolean;
}

const STATE_RANK: Record<MentionState, number> = { needs: 0, working: 1, open: 2, settled: 3 };

/**
 * The threads a `#` token is looking for: this project's before another project's, the closest
 * title first (`lib/modelSearch.ts`'s `fuzzyScore`, so a title typed out of order still finds it),
 * and what asks or works before what is settled when two names match as well. A thread with no
 * name is left out: there is nothing to match it on, and nothing to write in the message.
 */
export function rankThreads(threads: readonly ThreadChoice[], query: string, limit = 20): ThreadChoice[] {
  const scored = threads.flatMap((thread, at) => {
    const score = fuzzyScore(query, thread.title);
    return score === null ? [] : [{ thread, at, score }];
  });
  scored.sort(
    (a, b) =>
      Number(!!b.thread.home) - Number(!!a.thread.home) ||
      b.score - a.score ||
      STATE_RANK[a.thread.state] - STATE_RANK[b.thread.state] ||
      (b.thread.updatedAt ?? 0) - (a.thread.updatedAt ?? 0) ||
      a.at - b.at,
  );
  return scored.slice(0, limit).map((hit) => hit.thread);
}

/** A harness the `@` list offers: what the agent can hand work to. */
export interface AgentChoice {
  /** The harness's id: what the message names. */
  harness: string;
  /** What the harness is called: the row's own left-hand name when it has no model. */
  harnessName: string;
  /** The model the delegate would start on, when the row names one. */
  model?: string;
  /** That model's name, as the harness lists it: the row's name. */
  name: string;
  /** The effort the delegate would start on. */
  effort?: string;
  /** The named login it would run as, when the harness has more than one. */
  account?: string;
  /** What that login's plan has left, when the app read it: `62% left`. */
  plan?: string;
  /** What the draft writes after the `@`. */
  token: string;
}

/**
 * What an agent row answers: the harness, the model, the effort, and what the drawer calls it. The
 * model's name and id lead, then the harness's, then the token the message will carry.
 */
function agentScore(query: string, agent: AgentChoice): number | null {
  const scores = [
    fuzzyScore(query, agent.name),
    agent.model ? fuzzyScore(query, agent.model) : null,
    fuzzyScore(query, agent.harnessName),
    fuzzyScore(query, agent.harness),
    // `@codex/` is how a harness's models are asked for: the token starts with it.
    fuzzyScore(query, agent.token),
  ].flatMap((score) => (score === null ? [] : [score]));
  return scores.length ? Math.max(...scores) : null;
}

/** The agents an `@` token is looking for, the closest one first. */
export function rankAgents(query: string, agents: readonly AgentChoice[], limit = 8): AgentChoice[] {
  const scored = agents.flatMap((agent, at) => {
    const score = agentScore(query, agent);
    return score === null ? [] : [{ agent, at, score }];
  });
  scored.sort((a, b) => b.score - a.score || a.at - b.at);
  return scored.slice(0, limit).map((hit) => hit.agent);
}

export interface ComposeExtras {
  command?: SlashCommand;
  links: string[];
  attachments: Attachment[];
  /** The threads and harnesses the message names (`#Title`, `@codex`). */
  mentions?: Mention[];
}

export const emptyExtras: ComposeExtras = { links: [], attachments: [] };

export function hasExtras(extras: ComposeExtras | undefined): boolean {
  return !!extras && (!!extras.command || extras.links.length > 0 || extras.attachments.length > 0);
}

/**
 * Whether anything about the message has to travel beside its words: a chip, or the id a mention
 * stands for (the draft says only `#Title`, the message says the thread). Not `hasExtras`: a
 * mention never makes an empty draft sendable, but it must not be dropped when the message waits in
 * a queue or a new thread's setup, or the id it was picked for is lost.
 */
export function carriesExtras(extras: ComposeExtras | undefined): boolean {
  return hasExtras(extras) || (extras?.mentions?.length ?? 0) > 0;
}

/**
 * Where a paste sits after the draft changes: `before` and `after` are the draft around one edit,
 * and `at` the offset the paste took when it was made. The edit starts where the two texts stop
 * agreeing, so one that starts at or before the paste moves it by the edit's length change; one
 * after the paste leaves it alone. Clamped to the new draft, since the text it was pasted under may
 * have gone.
 */
export function shiftPasteOffset(at: number, before: string, after: string): number {
  const max = Math.min(before.length, after.length);
  let common = 0;
  while (common < max && before[common] === after[common]) common++;
  const next = common <= at ? at + (after.length - before.length) : at;
  return Math.max(0, Math.min(after.length, next));
}

/** The attachments with each paste's offset kept where it was in the draft that became `after`. */
export function trackPastes(
  attachments: readonly Attachment[],
  before: string,
  after: string,
): readonly Attachment[] {
  if (before === after) return attachments;
  let changed = false;
  const next = attachments.map((a) => {
    if (a.kind !== "text") return a;
    const at = shiftPasteOffset(a.at, before, after);
    if (at === a.at) return a;
    changed = true;
    return { ...a, at };
  });
  return changed ? next : attachments;
}

/** A paste spliced into the draft at `at`, with a newline where it would run into the text around it. */
function insertPaste(draft: string, at: number, pasted: string): { text: string; added: number } {
  const before = draft.slice(0, at);
  const after = draft.slice(at);
  const lead = before && !/\s$/.test(before) && !/^\s/.test(pasted) ? "\n" : "";
  const trail = after && !/\s$/.test(pasted) && !/^\s/.test(after) ? "\n" : "";
  return {
    text: `${before}${lead}${pasted}${trail}${after}`,
    added: lead.length + pasted.length + trail.length,
  };
}

/**
 * The typed text with every long paste put back where it was pasted, as plain text: the field keeps
 * a paste above it while the draft is written, and the message says it in the place it was pasted.
 * Pastes go in by offset (`at`), those at the same offset in the order they were made, and each one
 * gets a newline where it would otherwise run into the text beside it. The typed text keeps the
 * trim it had before pastes were a part of it.
 */
export function splicePastes(text: string, attachments: readonly Attachment[]): string {
  const start = text.length - text.trimStart().length;
  let body = text.slice(start).trimEnd();
  let added = 0;
  const pastes = attachments.flatMap((a, i) => (a.kind === "text" ? [{ paste: a, i }] : []));
  pastes.sort((x, y) => x.paste.at - y.paste.at || x.i - y.i);
  for (const { paste } of pastes) {
    const at = Math.max(0, Math.min(body.length, paste.at - start + added));
    const next = insertPaste(body, at, paste.text);
    body = next.text;
    added += next.added;
  }
  return body;
}

/**
 * The text the prompt carries, in the order the composer shows it: command, links, then what was
 * typed with its pastes in place and each mention written out in full. The transcript shows the
 * same text, so the mention it draws as a chip is the one the agent was given.
 */
export function composedText(text: string, extras: ComposeExtras): string {
  const head = [extras.command?.name, ...extras.links].filter(Boolean).join(" ");
  const body = serializeMentions(splicePastes(text, extras.attachments), extras.mentions);
  if (!head) return body;
  return body ? `${head} ${body}` : head;
}

function relative(path: string, cwd: string): string {
  return cwd && path.startsWith(`${cwd.replace(/\/$/, "")}/`)
    ? path.slice(cwd.replace(/\/$/, "").length + 1)
    : path;
}

/** A message another thread delivered: the app writes it as `From thread “…” (agent): …`. */
const DELIVERED = /^From thread “/;

/**
 * What ↑ walks in the composer: the user's own messages in this thread, oldest first. A wake is the
 * app's own turn, a message another thread delivered is not the user's to recall, and one with no
 * text (attachments only) has nothing to put back in the field.
 */
export function sentMessages(
  items: readonly { kind: string; text?: string; wake?: boolean; continued?: boolean }[],
): string[] {
  const out: string[] = [];
  for (const item of items) {
    if (item.kind !== "user" || item.wake || item.continued) continue;
    const text = item.text;
    if (!text?.trim() || DELIVERED.test(text)) continue;
    out.push(text);
  }
  return out;
}

/** Preserve skill metadata when a drawer row sends immediately, just as a command chip does. */
export function commandSubmission(command: SlashCommand): { text: string; extras: ComposeExtras } {
  return command.action === "skill"
    ? { text: "", extras: { ...emptyExtras, command } }
    : { text: command.name, extras: emptyExtras };
}

export interface BlockOptions {
  /** The thread's harness takes image blocks. */
  images: boolean;
  /** The built-in harness reads only text and images; others also take `resource_link` blocks. */
  builtin: boolean;
  cwd: string;
}

/**
 * ACP prompt blocks for a message. Images become `image` blocks; files with a
 * path become `resource_link` blocks on an external harness and an "Attached
 * files" list in the text on the built-in one (it ignores links, as the
 * SwiftUI app found); a file without a path is inlined as text. A long paste is
 * already in the text, at the offset it was pasted (`composedText`), so it
 * needs no block of its own.
 *
 * A skill is RustCode's own (the harness did not advertise it): the built-in
 * harness reads `/name` as a prompt and routes it to `use_skill`, but another
 * harness may reject a leading slash command it does not know, so it is sent as
 * words naming the skill instead. What the transcript shows is unchanged.
 */
export function promptBlocks(text: string, extras: ComposeExtras, options: BlockOptions): Json[] {
  let body = composedText(text, extras);
  if (!options.builtin && extras.command?.action === "skill") {
    const name = extras.command.name.replace(/^\/+/, "");
    const rest = composedText(text, { ...extras, command: undefined });
    body = rest ? `Use the ${name} skill. ${rest}` : `Use the ${name} skill.`;
  }
  const files = extras.attachments.filter((a) => a.kind === "file");
  const withPath = files.filter((f) => f.path);
  const inline = files.filter((f) => !f.path && f.text !== undefined);
  if (withPath.length && options.builtin) {
    const list = withPath.map((f) => `- \`${relative(f.path!, options.cwd)}\``).join("\n");
    body = `${body ? `${body}\n\n` : ""}Attached files:\n${list}`;
  }
  for (const f of inline) {
    body = `${body ? `${body}\n\n` : ""}Attached file \`${f.name}\`:\n\`\`\`\n${f.text}\n\`\`\``;
  }
  const blocks: Json[] = [];
  if (body) blocks.push({ type: "text", text: body });
  if (!options.builtin) {
    for (const f of withPath) {
      const block: JsonObject = { type: "resource_link", uri: `file://${encodeURI(f.path!)}`, name: f.name };
      blocks.push(block);
    }
  }
  if (options.images) {
    for (const a of extras.attachments) {
      if (a.kind === "image") blocks.push({ type: "image", mimeType: a.mimeType, data: a.data });
    }
  }
  return blocks;
}

/** What the transcript shows for a sent message's attachments. */
export interface SentAttachment {
  kind: "image" | "file" | "text";
  name: string;
  /** A data URL, for an image sent from here. */
  preview?: string;
  /**
   * The pasted text, on a message sent before pastes were composed into the text: history keeps its
   * chip (`ui/PasteChip.tsx`), and Retry or Edit carries the text again.
   */
  text?: string;
}

/**
 * What the transcript keeps of a sent message's attachments. A long paste is in the message's text
 * now (`composedText`), so no chip is kept for it; images and files keep their bubble.
 */
export function sentAttachments(extras: ComposeExtras): SentAttachment[] {
  return extras.attachments.flatMap((a): SentAttachment[] => {
    if (a.kind === "image")
      return [{ kind: "image", name: a.name, preview: `data:${a.mimeType};base64,${a.data}` }];
    if (a.kind === "text") return [];
    return [{ kind: "file", name: a.name }];
  });
}

const IMAGE_MARKER = /!\[image\]\(file:\/\/[^)\s]+\)\n?/g;

/** A replayed user message: its text without the engine's `![image](file://…)` markers, and how many there were. */
export function splitImageMarkers(text: string): { text: string; images: number } {
  const images = text.match(IMAGE_MARKER)?.length ?? 0;
  return { text: images ? text.replace(IMAGE_MARKER, "").trim() : text, images };
}

const URL_IN_TEXT = /\bhttps?:\/\/[^\s<>"'`]+[^\s<>"'`.,;:!?)\]}]/gi;
/** A file reference the `@` menu wrote: `@src/main.ts` or, with a space in the path, `@"my file.ts"`. */
const REF_IN_TEXT = /(^|[\s(])@("[^"\n]+"|[\w./@+-]+)/g;

/**
 * What the composer draws as a token inside the draft: a URL, or a `@file` reference.
 *
 * T3 Code's composer keeps references and citations as tokens in the text itself. RustCode has no
 * editor, so the field's mirror does it: the textarea's text is transparent and the mirror under it
 * draws the same string with each token on a quiet pill. The spans take **no** horizontal padding,
 * since the mirror and the field share one line box and any padding would move the caret off its
 * character.
 */
export interface DraftToken {
  text?: string;
  url?: string;
  ref?: string;
  /** A thread mention, drawn on a pill as the draft's own characters (`#Title`). */
  mention?: string;
}

/** The mention runs of one text part, a thread's on a pill and a harness's as a reference. */
function mentionTokens(text: string, mentions: readonly Mention[]): DraftToken[] {
  return mentionRuns(text, mentions).flatMap((run): DraftToken[] => {
    if (typeof run === "string") return refRuns(run);
    return run.mention.kind === "thread" ? [{ mention: run.raw }] : [{ ref: run.raw.slice(1) }];
  });
}

function refRuns(text: string): DraftToken[] {
  const out: DraftToken[] = [];
  let last = 0;
  for (const m of text.matchAll(REF_IN_TEXT)) {
    const ref = m[2];
    if (!ref) continue;
    const at = (m.index ?? 0) + (m[1]?.length ?? 0);
    if (at > last) out.push({ text: text.slice(last, at) });
    out.push({ ref });
    // `at` is the `@`; the pill draws it back in front of the reference.
    last = at + 1 + ref.length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/**
 * The draft split into text and tokens: URLs first, then the mentions and references inside the
 * text between them. A mention's pill carries the draft's own characters, so the mirror and the
 * field stay on the same line.
 */
export function splitTokens(text: string, mentions: readonly Mention[] = []): DraftToken[] {
  return splitUrls(text).flatMap((part) =>
    part.url ? [part] : part.text === undefined ? [] : mentionTokens(part.text, mentions),
  );
}

/**
 * What the mirror draws, as one string: exactly the draft's characters, token by token. The field's
 * text is transparent over it, so anything the mirror prints differently — a shorter label, a
 * bolder glyph — moves the caret and the wrap points off the text. A test holds the two together.
 */
export function mirrorText(text: string, mentions: readonly Mention[] = []): string {
  return splitTokens(text, mentions)
    .map((part) => part.text ?? part.url ?? (part.ref !== undefined ? `@${part.ref}` : (part.mention ?? "")))
    .join("");
}

/** Whether the draft has anything the mirror draws as a token. */
export function hasTokens(text: string, mentions: readonly Mention[] = []): boolean {
  const urls = new RegExp(URL_IN_TEXT.source, "i");
  const refs = new RegExp(REF_IN_TEXT.source);
  return urls.test(text) || refs.test(text) || mentions.some((m) => text.includes(mentionToken(m)));
}

/** Plain text split into text runs and URLs, for link chips in user messages. */
export function splitUrls(text: string): { text?: string; url?: string }[] {
  const out: { text?: string; url?: string }[] = [];
  let last = 0;
  for (const m of text.matchAll(URL_IN_TEXT)) {
    const at = m.index ?? 0;
    if (at > last) out.push({ text: text.slice(last, at) });
    out.push({ url: m[0] });
    last = at + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}
