// Every pull request answer the app has: the lists the panel and the sidebar
// draw (`_rustcode/pulls`), the detail a click opens (`/view`) and the diff the
// review view reads (`/diff`). Each read is a `gh` process in the engine, about
// a second or more, so an answer is painted at once and a newer one read behind
// it. Answers are also saved on the device, so a reload or a restart paints
// them again, and a row the pointer rests on reads its detail before the click.
// One rule everywhere: a key has one read in flight, an answer younger than
// `PULL_FRESH_MS` is not read again, and an answer to an older read never lands.

import { useEffect, useSyncExternalStore } from "react";
import { type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import {
  type PullDetail,
  type PullError,
  type PullRequest,
  parsePull,
  parsePullDetail,
  parsePullError,
  parsePullList,
  pullErrorTitle,
} from "../acp/rustcode";
import { Method } from "../acp/types";
import { request, toast } from "./store";

/** The four lists the panel draws. */
export type PullListState = "open" | "closed" | "merged" | "all";

/** An answer this recent is fresh enough to skip a read. */
export const PULL_FRESH_MS = 30_000;

/** The list the panel opens with: enough rows to scroll a panel, no more. */
export const PULL_LIMIT = 30;

/** Where the answers are saved, and what shape that file has. */
export const SAVED_KEY = "rustcode.pullQueries";
export const SAVED_VERSION = 1;

/** What the saved copy holds: eight projects' lists, forty details, and no diff. */
export const SAVED_PROJECTS = 8;
export const SAVED_LISTS_PER_PROJECT = 4;
export const SAVED_DETAILS = 40;
/** One answer bigger than this is not worth the room: a body of a hundred comments. */
export const SAVED_ENTRY_BYTES = 256 * 1024;
/** The whole file stays under this, so it is read and written in one go. */
export const SAVED_TOTAL_BYTES = 1_500_000;
export const SAVED_MAX_AGE_MS = 7 * 24 * 60 * 60_000;

/** A list answer as a panel draws it. */
export interface PullListAnswer {
  pulls: PullRequest[];
  hasMore: boolean;
  /** When the answer behind it was read, so a filtered broader list says its age. */
  readAt: number;
  /** It came off the device: it paints, and it is always read again behind. */
  saved: boolean;
  /**
   * It is everything the state would answer: a list of that state, or a wider
   * "All" list that was itself exhaustive. A narrowed or filtered answer still
   * paints, but the state's own read runs behind it.
   */
  complete: boolean;
}

export interface PullDiffAnswer {
  diff?: string;
  error?: PullError;
}

export interface PullReadOptions {
  /** How old an answer may be before it is read again. */
  maxAge?: number;
  /** Reads even when the answer is fresh: a reload the user asked for. */
  force?: boolean;
}

interface ListEntry {
  cwd: string;
  state: PullListState;
  limit: number;
  pulls: PullRequest[];
  hasMore: boolean;
  readAt: number;
  saved: boolean;
}

interface DetailEntry {
  cwd: string;
  detail: PullDetail;
  readAt: number;
  saved: boolean;
}

interface DiffEntry {
  diff: string;
  readAt: number;
}

interface SummaryEntry {
  pull: PullRequest;
  readAt: number;
}

/** How the store reaches the engine, or a test's answers with its own timing. */
export type PullReader = (method: string, params: JsonObject, timeoutMs?: number) => Promise<Json>;

let read: PullReader = (method, params, timeoutMs) => request(method, params, timeoutMs);

/** Points the store at another reader: the tests' answers, which they time themselves. */
export function setPullReader(reader: PullReader): void {
  read = reader;
}

const lists = new Map<string, ListEntry>();
const details = new Map<string, DetailEntry>();
const diffs = new Map<string, DiffEntry>();
/** Each pull's row, from any list that carried it: the detail's header before its body. */
const summaries = new Map<string, SummaryEntry>();
/** Why a key's last read failed, and when: a failure is remembered as long as an answer. */
const errors = new Map<string, { error: PullError; at: number }>();
/** The latest read started per key. */
const reads = new Map<string, number>();
/** The read in flight per key, so a second asker waits on the first instead of adding a process. */
const inflight = new Map<string, Promise<void>>();
/** A filtered list, kept per ask so a panel's rows stay the same objects across renders. */
const derived = new Map<string, { from: string; answer: PullListAnswer }>();
const listeners = new Set<() => void>();
let version = 0;

export const pullQueriesVersion = () => version;

export function subscribePullQueries(listener: () => void): () => void {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

function changed() {
  version += 1;
  for (const listener of listeners) listener();
}

const listKey = (cwd: string, state: PullListState, limit: number) => `list|${cwd}|${state}|${limit}`;
const detailKey = (cwd: string, number: number) => `detail|${cwd}|${number}`;
const diffKey = (cwd: string, number: number) => `diff|${cwd}|${number}`;
const summaryKey = (cwd: string, number: number) => `${cwd}|${number}`;

const STATES: readonly PullListState[] = ["open", "closed", "merged", "all"];

function stateOf(value: unknown): PullListState | undefined {
  return typeof value === "string" && (STATES as readonly string[]).includes(value)
    ? (value as PullListState)
    : undefined;
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/**
 * What a screen shows of a failed read: the error only when there is nothing to
 * draw instead, so a refresh that fails leaves the rows (and the panel) as they were.
 */
export function answerError(error: PullError | undefined, answer: unknown): PullError | undefined {
  return answer === undefined ? error : undefined;
}

/** An answer is fresh when it was read here, recently: a saved one always refreshes behind. */
function isFresh(readAt: number, saved: boolean, maxAge: number, now: number): boolean {
  return !saved && now - readAt < maxAge;
}

/** Starts a read: its token is the latest, so an answer to an older one is dropped. */
function beginRead(key: string): number {
  const token = (reads.get(key) ?? 0) + 1;
  reads.set(key, token);
  return token;
}

function failRead(key: string, token: number, error: PullError, announce = false) {
  if (reads.get(key) !== token) return;
  errors.set(key, { error, at: Date.now() });
  // A refresh behind the panel is quiet; a reload the user asked for is not,
  // or the click would look like nothing happened.
  if (announce) toast(`${pullErrorTitle(error.code)}: ${error.message}`, "error");
  changed();
}

/** Runs one read under its key: one at a time, and only the newest answer lands. */
function start(key: string, read: (token: number) => Promise<void>): Promise<void> {
  const token = beginRead(key);
  // The promise compares itself in a holder: a const cannot name itself in its own initializer.
  const holder: { done?: Promise<void> } = {};
  holder.done = read(token)
    // A read that throws (a parser meeting a shape it does not know) is this
    // key's failure, not an unhandled rejection.
    .catch((error: unknown) => failRead(key, token, { code: "failed", message: errorText(error) }))
    .then(() => {
      if (inflight.get(key) === holder.done) inflight.delete(key);
      changed();
    });
  inflight.set(key, holder.done);
  changed();
  return holder.done;
}

function keepSummary(cwd: string, pull: PullRequest, readAt: number) {
  const key = summaryKey(cwd, pull.number);
  const known = summaries.get(key);
  if (known && known.readAt > readAt) return;
  summaries.set(key, { pull, readAt });
}

function seedSummaries(cwd: string, pulls: readonly PullRequest[], readAt: number) {
  for (const pull of pulls) keepSummary(cwd, pull, readAt);
}

// ---------------------------------------------------------------- the lists

/** The newest entry of a project's lists that matches, whatever its age. */
function newestList(
  cwd: string,
  limit: number,
  matches: (entry: ListEntry) => boolean,
): { key: string; entry: ListEntry } | undefined {
  let found: { key: string; entry: ListEntry } | undefined;
  for (const [key, entry] of lists) {
    if (entry.cwd !== cwd || entry.limit < limit || !matches(entry)) continue;
    if (!found || entry.readAt > found.entry.readAt) found = { key, entry };
  }
  return found;
}

/**
 * The list a panel draws for a state, from the answer that can be that list.
 *
 * A list of the same state, as long or longer, answers it exactly: its first
 * `limit` rows are the rows the state's own read would bring back. The sidebar's
 * "All" list answers a state only by filtering, so it paints while the state's
 * own read runs, and it counts as that state's answer only when it was itself
 * exhaustive (`hasMore: false`) — otherwise the filter sees a truncated list and
 * would hide pulls the state's read would find.
 */
export function pullList(cwd: string, state: PullListState, limit = PULL_LIMIT): PullListAnswer | undefined {
  hydratePullQueries();
  const target = listKey(cwd, state, limit);
  const viewed = derived.get(target);
  // A complete answer outranks a filtered one, however old: a stand-in must not
  // replace the state's own list.
  const best =
    newestList(cwd, limit, (entry) => entry.state === state) ??
    newestList(cwd, limit, (entry) => entry.state === "all");
  if (!best) return undefined;
  const entry = best.entry;
  const from = `${best.key}|${entry.readAt}`;
  // One answer per ask, so a panel's rows keep their identity across renders.
  const cached = viewed?.from === from ? viewed.answer : undefined;
  if (cached) return cached;
  const narrowed = entry.state === state ? entry.pulls : entry.pulls.filter((pull) => pull.state === state);
  const answer: PullListAnswer = {
    pulls: narrowed.slice(0, limit),
    // More rows exist than are shown: the source was cut short, or the list is.
    hasMore: entry.hasMore || narrowed.length > limit,
    readAt: entry.readAt,
    saved: entry.saved,
    complete: entry.state === state || !entry.hasMore,
  };
  derived.set(target, { from, answer });
  return answer;
}

export function pullDetail(cwd: string, number: number): PullDetail | undefined {
  hydratePullQueries();
  return details.get(detailKey(cwd, number))?.detail;
}

/** A pull's row: the detail's header when one was read, else the list row that carried it. */
export function pullSummary(cwd: string, number: number): PullRequest | undefined {
  hydratePullQueries();
  return details.get(detailKey(cwd, number))?.detail ?? summaries.get(summaryKey(cwd, number))?.pull;
}

/** The diff kept for a pull, if one was read. Diffs are memory only. */
export function pullDiff(cwd: string, number: number): string | undefined {
  hydratePullQueries();
  return diffs.get(diffKey(cwd, number))?.diff;
}

/** Whether a list is being read right now, for the panel's own reload spinner. */
export function pullListReading(cwd: string, state: PullListState, limit = PULL_LIMIT): boolean {
  return inflight.has(listKey(cwd, state, limit));
}

export function pullDetailReading(cwd: string, number: number): boolean {
  return inflight.has(detailKey(cwd, number));
}

/**
 * Whether a pull's detail was read here in this session, rather than painted
 * from the device: a review posts at the commit a live read reported.
 */
export function pullDetailLive(cwd: string, number: number): boolean {
  const entry = details.get(detailKey(cwd, number));
  return !!entry && !entry.saved;
}

/**
 * Reads a project's list for one state, unless the answer is fresh and complete:
 * a filtered or narrowed stand-in paints, and this still reads the state's own
 * list behind it. A read in flight for the same key is joined rather than repeated.
 */
export function readPullList(
  cwd: string,
  state: PullListState,
  options: PullReadOptions & { limit?: number } = {},
): Promise<void> {
  const limit = options.limit ?? PULL_LIMIT;
  if (!cwd) return Promise.resolve();
  if (hydratePullQueries()) changed();
  const key = listKey(cwd, state, limit);
  const running = inflight.get(key);
  if (running && !options.force) return running;
  const known = pullList(cwd, state, limit);
  if (
    !options.force &&
    known?.complete &&
    isFresh(known.readAt, known.saved, options.maxAge ?? PULL_FRESH_MS, Date.now())
  ) {
    return Promise.resolve();
  }
  return start(key, (token) =>
    read(Method.pulls, { cwd, state, limit }, 60_000).then(
      (result) => {
        const error = parsePullError(result);
        if (error) return void failRead(key, token, error, options.force);
        if (reads.get(key) !== token) return;
        const answer = parsePullList(result);
        const readAt = Date.now();
        lists.set(key, {
          cwd,
          state,
          limit,
          pulls: answer.pulls,
          hasMore: answer.hasMore,
          readAt,
          saved: false,
        });
        derived.clear();
        errors.delete(key);
        seedSummaries(cwd, answer.pulls, readAt);
        scheduleSave();
        changed();
      },
      (error) => failRead(key, token, { code: "failed", message: errorText(error) }, options.force),
    ),
  );
}

/**
 * Reads a pull's detail (`/view`): its body, checks, files and activity. Its
 * header is already known from its list row, so a panel draws that meanwhile.
 */
export function readPullDetail(cwd: string, number: number, options: PullReadOptions = {}): Promise<void> {
  if (!cwd || !number) return Promise.resolve();
  if (hydratePullQueries()) changed();
  const key = detailKey(cwd, number);
  const running = inflight.get(key);
  if (running && !options.force) return running;
  const known = details.get(key);
  if (
    !options.force &&
    known &&
    isFresh(known.readAt, known.saved, options.maxAge ?? PULL_FRESH_MS, Date.now())
  ) {
    return Promise.resolve();
  }
  return start(key, (token) =>
    read(Method.pullView, { cwd, number }, 60_000).then(
      (result) => {
        const error = parsePullError(result);
        if (error) return void failRead(key, token, error, options.force);
        if (reads.get(key) !== token) return;
        const detail = parsePullDetail(result);
        if (!detail) {
          return void failRead(
            key,
            token,
            { code: "failed", message: "The pull request came back empty." },
            options.force,
          );
        }
        const readAt = Date.now();
        details.set(key, { cwd, detail, readAt, saved: false });
        errors.delete(key);
        keepSummary(cwd, detail, readAt);
        scheduleSave();
        changed();
      },
      (error) => failRead(key, token, { code: "failed", message: errorText(error) }, options.force),
    ),
  );
}

/** Reads a pull's diff (`/diff`), memory only: a diff is too big to keep on the device. */
export function readPullDiff(cwd: string, number: number, options: PullReadOptions = {}): Promise<void> {
  if (!cwd || !number) return Promise.resolve();
  if (hydratePullQueries()) changed();
  const key = diffKey(cwd, number);
  const running = inflight.get(key);
  if (running && !options.force) return running;
  const known = diffs.get(key);
  if (!options.force && known && isFresh(known.readAt, false, options.maxAge ?? PULL_FRESH_MS, Date.now())) {
    return Promise.resolve();
  }
  return start(key, (token) =>
    read(Method.pullDiff, { cwd, number }, 60_000).then(
      (result) => {
        const error = parsePullError(result);
        if (error) return void failRead(key, token, error, options.force);
        if (reads.get(key) !== token) return;
        diffs.set(key, { diff: str(obj(result)?.diff) ?? "", readAt: Date.now() });
        errors.delete(key);
        changed();
      },
      (error) => failRead(key, token, { code: "failed", message: errorText(error) }, options.force),
    ),
  );
}

/** The diff of a pull, read unless it is fresh: what the review view draws. */
export async function loadPullDiff(
  cwd: string,
  number: number,
  options: PullReadOptions = {},
): Promise<PullDiffAnswer> {
  await readPullDiff(cwd, number, options);
  const diff = pullDiff(cwd, number);
  const error = answerError(pullDiffError(cwd, number), diff);
  return { ...(diff !== undefined ? { diff } : {}), ...(error ? { error } : {}) };
}

/**
 * What a row reads when the pointer or the keyboard rests on it: the same key
 * the click reads, so the read is never wasted, and nothing is read by passing
 * over a row.
 */
export function prefetchPullDetail(cwd: string | undefined, number: number | undefined): void {
  if (!cwd || !number) return;
  void readPullDetail(cwd, number);
}

export function prefetchPullDiff(cwd: string | undefined, number: number | undefined): void {
  if (!cwd || !number) return;
  void readPullDiff(cwd, number);
}

/** Seeds what a panel draws before its detail: rows a list read elsewhere carried. */
export function rememberPullSummaries(cwd: string, pulls: readonly PullRequest[], now = Date.now()): void {
  if (!cwd || !pulls.length) return;
  hydratePullQueries(now);
  seedSummaries(cwd, pulls, now);
  changed();
}

/** Why a list's last read failed, whether or not an older answer is still drawn. */
export function pullListError(cwd: string, state: PullListState, limit = PULL_LIMIT): PullError | undefined {
  return errors.get(listKey(cwd, state, limit))?.error;
}

/** When a list's last read failed, if it did: the sidebar does not retry it on every render. */
export function pullListFailedAt(cwd: string, state: PullListState, limit = PULL_LIMIT): number | undefined {
  return errors.get(listKey(cwd, state, limit))?.at;
}

export function pullDetailError(cwd: string, number: number): PullError | undefined {
  return errors.get(detailKey(cwd, number))?.error;
}

export function pullDiffError(cwd: string, number: number): PullError | undefined {
  return errors.get(diffKey(cwd, number))?.error;
}

// ---------------------------------------------------------------- the panel's reads

/**
 * A project's list for one state. It paints what the app holds (saved or read
 * a moment ago) and reads behind it; `error` only comes with nothing to draw.
 */
export function usePullList(
  cwd: string | undefined,
  state: PullListState,
  options: { limit?: number; enabled?: boolean } = {},
): { data?: PullRequest[]; hasMore: boolean; error?: PullError; refreshing: boolean } {
  const limit = options.limit ?? PULL_LIMIT;
  const enabled = options.enabled ?? true;
  useSyncExternalStore(subscribePullQueries, pullQueriesVersion);
  useEffect(() => {
    if (!enabled || !cwd) return;
    void readPullList(cwd, state, { limit });
  }, [enabled, cwd, state, limit]);
  const answer = cwd ? pullList(cwd, state, limit) : undefined;
  return {
    data: answer?.pulls,
    hasMore: answer?.hasMore ?? false,
    error: cwd && enabled ? answerError(pullListError(cwd, state, limit), answer) : undefined,
    refreshing: !!cwd && pullListReading(cwd, state, limit),
  };
}

/**
 * One pull in full, with its row as `summary`: a panel draws the title, state,
 * branch and checks from that while the body, files and activity are read.
 */
export function usePullDetail(
  cwd: string | undefined,
  number: number | null | undefined,
): { detail?: PullDetail; summary?: PullRequest; error?: PullError; refreshing: boolean; live: boolean } {
  useSyncExternalStore(subscribePullQueries, pullQueriesVersion);
  useEffect(() => {
    if (cwd && number) void readPullDetail(cwd, number);
  }, [cwd, number]);
  const detail = cwd && number ? pullDetail(cwd, number) : undefined;
  const summary = detail ?? (cwd && number ? pullSummary(cwd, number) : undefined);
  return {
    detail,
    summary,
    error: cwd && number ? answerError(pullDetailError(cwd, number), detail) : undefined,
    refreshing: !!cwd && !!number && pullDetailReading(cwd, number),
    // A saved detail makes a header, not a commit to review at.
    live: !!cwd && !!number && pullDetailLive(cwd, number),
  };
}

// ---------------------------------------------------------------- saved answers

/** How the saved file is shaped: lists and details, newest first, no diff. */
interface SavedList {
  cwd: string;
  state: PullListState;
  limit: number;
  readAt: number;
  hasMore: boolean;
  pulls: Json[];
}

interface SavedDetail {
  cwd: string;
  number: number;
  readAt: number;
  pull: Json;
}

interface SavedQueries {
  version: number;
  lists: SavedList[];
  details: SavedDetail[];
}

let hydrated = false;
let saveTimer: ReturnType<typeof setTimeout> | undefined;

/** The device's storage, when this window has one (a private window may not). */
function storage(): Storage | undefined {
  try {
    return typeof localStorage === "undefined" ? undefined : localStorage;
  } catch {
    return undefined;
  }
}

/** A pull as it is saved: the wire's fields, with the parsed timestamps kept as numbers. */
function savedPull(pull: PullRequest): JsonObject {
  return {
    number: pull.number,
    title: pull.title,
    author: pull.author,
    state: pull.state,
    draft: pull.draft,
    ...(pull.review ? { review: pull.review } : {}),
    checks: {
      passed: pull.checks.passed,
      failed: pull.checks.failed,
      pending: pull.checks.pending,
      skipped: pull.checks.skipped,
    },
    headRef: pull.headRef,
    baseRef: pull.baseRef,
    headSha: pull.headSha,
    ...(pull.updatedAt !== undefined ? { updatedAt: pull.updatedAt } : {}),
    ...(pull.url ? { url: pull.url } : {}),
    additions: pull.additions,
    deletions: pull.deletions,
    changedFiles: pull.changedFiles,
  };
}

function savedDetail(detail: PullDetail): JsonObject {
  return {
    ...savedPull(detail),
    body: detail.body,
    checkRuns: detail.checkRuns.map((check) => ({
      name: check.name,
      status: check.status,
      ...(check.url ? { url: check.url } : {}),
    })),
    files: detail.files.map((file) => ({
      path: file.path,
      additions: file.additions,
      deletions: file.deletions,
    })),
    activity: detail.activity.map((line) => ({
      author: line.author,
      kind: line.kind,
      ...(line.at !== undefined ? { at: line.at } : {}),
      body: line.body,
      ...(line.url ? { url: line.url } : {}),
    })),
    // Undefined fields drop out of the saved JSON.
    reviewComments: detail.reviewComments.map((c) => ({ ...c }) as unknown as JsonObject),
    ...(detail.reviewCommentsError ? { reviewCommentsError: detail.reviewCommentsError } : {}),
  };
}

/**
 * The answers that would be saved now, trimmed to what the device may hold: the
 * newest eight projects' lists (four asks each), the newest forty details,
 * nothing over 256 KB, nothing older than a week, all under 1.5 MB.
 *
 * Each row is serialized once, and that text is both what is measured and what
 * the file is built from: the file is the same JSON `JSON.stringify` of the
 * payload would give, without writing every row twice to find out its size.
 */
function trimmed(now: number): { payload: SavedQueries; text: string } {
  let total = 0;
  const savedLists: SavedList[] = [];
  const listTexts: string[] = [];
  const perProject = new Map<string, number>();
  for (const entry of [...lists.values()].sort((a, b) => b.readAt - a.readAt)) {
    if (now - entry.readAt > SAVED_MAX_AGE_MS) continue;
    const kept = perProject.get(entry.cwd) ?? 0;
    if (kept >= SAVED_LISTS_PER_PROJECT) continue;
    if (!perProject.has(entry.cwd) && perProject.size >= SAVED_PROJECTS) continue;
    const row: SavedList = {
      cwd: entry.cwd,
      state: entry.state,
      limit: entry.limit,
      readAt: entry.readAt,
      hasMore: entry.hasMore,
      pulls: entry.pulls.map(savedPull),
    };
    const text = JSON.stringify(row);
    if (text.length > SAVED_ENTRY_BYTES || total + text.length > SAVED_TOTAL_BYTES) continue;
    total += text.length;
    perProject.set(entry.cwd, kept + 1);
    savedLists.push(row);
    listTexts.push(text);
  }
  const savedDetails: SavedDetail[] = [];
  const detailTexts: string[] = [];
  for (const entry of [...details.values()].sort((a, b) => b.readAt - a.readAt)) {
    if (savedDetails.length >= SAVED_DETAILS) break;
    if (now - entry.readAt > SAVED_MAX_AGE_MS) continue;
    const row: SavedDetail = {
      cwd: entry.cwd,
      number: entry.detail.number,
      readAt: entry.readAt,
      pull: savedDetail(entry.detail),
    };
    const text = JSON.stringify(row);
    if (text.length > SAVED_ENTRY_BYTES || total + text.length > SAVED_TOTAL_BYTES) continue;
    total += text.length;
    savedDetails.push(row);
    detailTexts.push(text);
  }
  return {
    payload: { version: SAVED_VERSION, lists: savedLists, details: savedDetails },
    // The same text `JSON.stringify(payload)` gives, in the same key order.
    text: `{"version":${SAVED_VERSION},"lists":[${listTexts.join(",")}],"details":[${detailTexts.join(",")}]}`,
  };
}

/** The answers that would be saved now, trimmed to the device's bounds. */
export function savedPayload(now = Date.now()): SavedQueries {
  return trimmed(now).payload;
}

/** Writes the trimmed answers now. Never throws: a window without storage works as before. */
export function savePullQueries(now = Date.now()): void {
  hydratePullQueries(now);
  const store = storage();
  if (!store) return;
  const { text } = trimmed(now);
  try {
    store.setItem(SAVED_KEY, text);
  } catch {
    // Full, private, or held by another window: the answers stay in memory.
  }
}

/**
 * How long a burst of answers waits before it is written: a panel opens with a
 * list and a detail, and one file write for both is a write nobody notices.
 */
const SAVE_DEBOUNCE_MS = 2000;

function scheduleSave() {
  if (!storage() || saveTimer !== undefined) return;
  saveTimer = setTimeout(() => {
    saveTimer = undefined;
    savePullQueries();
  }, SAVE_DEBOUNCE_MS);
}

/**
 * Reads the saved answers once, on the first paint that needs one. A saved
 * answer counts as stale, so it always refreshes behind: it is only ever what
 * the app paints before the engine answers.
 */
export function hydratePullQueries(now = Date.now()): boolean {
  if (hydrated) return false;
  hydrated = true;
  const store = storage();
  if (!store) return false;
  let saved: SavedQueries | undefined;
  try {
    const raw = store.getItem(SAVED_KEY);
    saved = raw ? (JSON.parse(raw) as SavedQueries) : undefined;
  } catch {
    return false;
  }
  if (!saved || saved.version !== SAVED_VERSION) return false;
  for (const list of Array.isArray(saved.lists) ? saved.lists : []) {
    const state = stateOf(list?.state);
    const cwd = str(list?.cwd);
    const limit = typeof list?.limit === "number" ? list.limit : undefined;
    const readAt = typeof list?.readAt === "number" ? list.readAt : undefined;
    if (!state || !cwd || limit === undefined || readAt === undefined) continue;
    if (now - readAt > SAVED_MAX_AGE_MS) continue;
    const key = listKey(cwd, state, limit);
    if (lists.has(key)) continue;
    const pulls = (Array.isArray(list.pulls) ? list.pulls : []).flatMap((value) => {
      const pull = parsePull(value);
      return pull ? [pull] : [];
    });
    if (!pulls.length) continue;
    lists.set(key, { cwd, state, limit, pulls, hasMore: !!list.hasMore, readAt, saved: true });
    derived.clear();
    seedSummaries(cwd, pulls, readAt);
  }
  for (const row of Array.isArray(saved.details) ? saved.details : []) {
    const cwd = str(row?.cwd);
    const readAt = typeof row?.readAt === "number" ? row.readAt : undefined;
    if (!cwd || readAt === undefined) continue;
    if (now - readAt > SAVED_MAX_AGE_MS) continue;
    const detail = parsePullDetail({ pull: row.pull });
    if (!detail) continue;
    const key = detailKey(cwd, detail.number);
    if (details.has(key)) continue;
    details.set(key, { cwd, detail, readAt, saved: true });
    keepSummary(cwd, detail, readAt);
  }
  return true;
}

/** Drops every answer held in memory: a test's clean slate. The saved copy stays, as it would across a reload. */
export function clearPullQueries(): void {
  clearTimeout(saveTimer);
  saveTimer = undefined;
  hydrated = false;
  lists.clear();
  details.clear();
  diffs.clear();
  summaries.clear();
  errors.clear();
  reads.clear();
  inflight.clear();
  derived.clear();
  changed();
}
