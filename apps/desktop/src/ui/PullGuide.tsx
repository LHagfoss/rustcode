/**
 * The pull request's Guide tab: a tour of the change in reading order, one
 * stop at a time, written by the text generator on request. Each stop says
 * what changed, shows the lines worth reading, marks a call only the
 * maintainer can make as "Look closely", and offers questions that open a
 * thread about that stop. Nothing is posted to the pull request from here.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { PullRequest } from "../acp/rustcode";
import { pullErrorTitle } from "../acp/rustcode";
import { count } from "../lib/plural";
import { loadPullDiff } from "../state/pullQueries";
import { type PullDiffFile, pullDiffFiles } from "../state/pullReview";
import {
  type GuideStop,
  guidePrompt,
  LEFTOVER_TITLE,
  parseGuide,
  type ReviewGuide,
  savedGuide,
  saveGuide,
  stopQuestion,
} from "../state/reviewGuide";
import { openSettings, request, setDraft, setPanel, startNewThread } from "../state/store";
import { relativeAge } from "../state/thread";
import { Patch } from "./DiffView";
import { Bot, ChevronLeft, ChevronRight, RefreshCw, Sparkles, TriangleAlert } from "./icons";
import { Markdown } from "./Markdown";
import { Callout, Pill, Skeleton } from "./primitives";

/** A generator reading a large pull request takes minutes; the engine gives up at four. */
const WRITE_TIMEOUT_MS = 250_000;

/** Guides being written, by pull request, so leaving the tab does not lose one. */
const writing = new Map<string, Promise<ReviewGuide | null>>();
const guideKey = (cwd: string, number: number) => `${cwd}\u0000${number}`;

async function writeGuide(cwd: string, pull: PullRequest, body: string): Promise<ReviewGuide | null> {
  const answer = await loadPullDiff(cwd, pull.number);
  if (answer.error) throw new Error(`${pullErrorTitle(answer.error.code)}: ${answer.error.message}`);
  const files: PullDiffFile[] = pullDiffFiles(answer.diff ?? "");
  if (!files.length) throw new Error("The diff is empty.");
  const generated = (await request(
    "_rustcode/textGeneration/generate",
    { kind: "review_guide", prompt: guidePrompt(pull, body, files), project: cwd },
    WRITE_TIMEOUT_MS,
  )) as { text?: string | null };
  if (!generated.text) return null;
  const guide = parseGuide(generated.text, files, pull.headSha);
  saveGuide(cwd, pull.number, guide);
  return guide;
}

function startWrite(cwd: string, pull: PullRequest, body: string) {
  const key = guideKey(cwd, pull.number);
  const running = writing.get(key);
  if (running) return running;
  const promise = writeGuide(cwd, pull, body).finally(() => writing.delete(key));
  writing.set(key, promise);
  return promise;
}

type GuideState =
  | { status: "idle" }
  | { status: "writing" }
  | { status: "unconfigured" }
  | { status: "failed"; error: string };

export function PullGuide({ cwd, pull, body }: { cwd: string; pull: PullRequest; body: string }) {
  const [guide, setGuide] = useState<ReviewGuide | null>(() => savedGuide(cwd, pull.number));
  const [state, setState] = useState<GuideState>({ status: "idle" });
  const [at, setAt] = useState(0);
  const key = guideKey(cwd, pull.number);
  const shown = useRef(key);
  shown.current = key;
  const follow = useCallback(
    (promise: Promise<ReviewGuide | null>) => {
      const alive = () => shown.current === key;
      setState({ status: "writing" });
      promise
        .then((written) => {
          if (!alive()) return;
          if (!written) return setState({ status: "unconfigured" });
          setGuide(written);
          setAt(0);
          setState({ status: "idle" });
        })
        .catch((e) => {
          if (alive()) setState({ status: "failed", error: e instanceof Error ? e.message : String(e) });
        });
    },
    [key],
  );
  useEffect(() => {
    setGuide(savedGuide(cwd, pull.number));
    setAt(0);
    setState({ status: "idle" });
    const running = writing.get(key);
    if (running) follow(running);
  }, [cwd, pull.number, key, follow]);
  const write = () => follow(startWrite(cwd, pull, body));
  const stale = !!guide && guide.headSha !== pull.headSha;
  const stop = guide?.stops[Math.min(at, guide.stops.length - 1)];

  if (state.status === "writing") {
    return (
      <div role="status" aria-label="Writing the guide" className="flex flex-col gap-2">
        <p className="text-callout text-fg-muted">
          Reading the change. A large pull request takes a few minutes; you can leave this tab.
        </p>
        {[80, 60, 90, 45].map((w) => (
          <Skeleton key={w} className="h-3" style={{ width: `${w}%` }} />
        ))}
      </div>
    );
  }
  if (!guide || !stop) {
    return (
      <div className="flex flex-col items-start gap-2">
        <p className="text-callout text-fg-muted">
          A tour of this pull request in reading order: each stop says what changed, shows the lines worth
          reading and marks the calls only you can make. Every changed file appears on some stop.
        </p>
        <GuideProblem state={state} />
        <Pill prominent onClick={write}>
          <Sparkles size={13} /> Write guide
        </Pill>
      </div>
    );
  }
  const index = guide.stops.indexOf(stop);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2 text-caption text-fg-muted">
        <span className="min-w-0 flex-1">
          {count(guide.stops.length, "stop")} · written {relativeAge(guide.writtenAt)}
          {stale && " for an earlier push"}
        </span>
        <Pill size="sm" quiet onClick={write}>
          <RefreshCw size={12} /> Rewrite
        </Pill>
      </div>
      <GuideProblem state={state} />
      <ol aria-label="Guide stops" className="flex flex-wrap gap-1">
        {guide.stops.map((s, i) => (
          <li key={`${i}-${s.title}`}>
            <Pill
              size="sm"
              quiet={i !== index}
              aria-current={i === index ? "step" : undefined}
              onClick={() => setAt(i)}
              className="max-w-[220px]"
            >
              <span className="tabular">{i + 1}</span>
              <span className="truncate">{s.title}</span>
              {s.lookClosely && <TriangleAlert size={12} className="shrink-0 text-warning" />}
            </Pill>
          </li>
        ))}
      </ol>
      <GuideStopView stop={stop} pull={pull} cwd={cwd} hideExcerpts={stale} />
      <div className="flex items-center justify-between gap-2">
        <Pill size="sm" quiet disabled={index === 0} onClick={() => setAt(index - 1)}>
          <ChevronLeft size={12} /> Previous
        </Pill>
        <span className="text-caption text-fg-muted tabular">
          {index + 1} of {guide.stops.length}
        </span>
        <Pill size="sm" quiet disabled={index === guide.stops.length - 1} onClick={() => setAt(index + 1)}>
          Next <ChevronRight size={12} />
        </Pill>
      </div>
    </div>
  );
}

function GuideProblem({ state }: { state: GuideState }) {
  if (state.status === "unconfigured") {
    return (
      <Callout
        tone="warning"
        action={
          <Pill size="sm" onClick={() => openSettings("agents")}>
            Open Settings
          </Pill>
        }
      >
        Guides are written by the text generator. Choose one in Settings › Agents › Text generation.
      </Callout>
    );
  }
  if (state.status === "failed")
    return <Callout tone="danger">The guide could not be written: {state.error}</Callout>;
  return null;
}

function GuideStopView({
  stop,
  pull,
  cwd,
  hideExcerpts,
}: {
  stop: GuideStop;
  pull: PullRequest;
  cwd: string;
  hideExcerpts: boolean;
}) {
  const ask = (question: string) => {
    startNewThread(cwd);
    setDraft("new", stopQuestion(pull, stop, question));
    setPanel(null);
  };
  const questions = useMemo(
    () => [...stop.questions, ...(stop.title === LEFTOVER_TITLE ? [] : ["Explain this stop."])],
    [stop],
  );
  return (
    <section aria-label={stop.title} className="flex flex-col gap-2">
      <h3 className="selectable font-semibold text-body">{stop.title}</h3>
      {stop.lookClosely && (
        <div className="flex gap-2 rounded-md bg-warning/10 p-2 text-callout">
          <TriangleAlert size={14} className="mt-0.5 shrink-0 text-warning" />
          <span className="selectable">
            <b>Look closely.</b> {stop.lookClosely}
          </span>
        </div>
      )}
      {stop.summary && <Markdown text={stop.summary} className="selectable text-callout" />}
      {stop.title === LEFTOVER_TITLE && (
        <p className="text-callout text-fg-muted">Files no other stop covers; read them in the Diff tab.</p>
      )}
      {hideExcerpts
        ? stop.excerpts.length > 0 && (
            <p className="text-caption text-fg-muted">
              The excerpts are hidden: the pull request changed after this guide was written.
            </p>
          )
        : stop.excerpts.map((excerpt) => (
            <div key={`${excerpt.path}:${excerpt.side}:${excerpt.start}`} className="flex flex-col gap-1">
              <span className="font-mono text-caption text-fg-muted">
                {excerpt.path}:{excerpt.start}
                {excerpt.end > excerpt.start && `–${excerpt.end}`}
                {excerpt.side === "old" && " (removed)"}
              </span>
              <Patch lines={excerpt.lines} path={excerpt.path} />
            </div>
          ))}
      {stop.files.length > 0 && (
        <ul aria-label="Files on this stop" className="flex flex-wrap gap-1">
          {stop.files.map((path) => (
            <li
              key={path}
              className="rounded bg-fill-subtle px-1.5 py-0.5 font-mono text-caption text-fg-muted"
            >
              {path}
            </li>
          ))}
        </ul>
      )}
      {questions.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {questions.map((question) => (
            <Pill key={question} size="sm" onClick={() => ask(question)}>
              <Bot size={12} /> {question}
            </Pill>
          ))}
        </div>
      )}
    </section>
  );
}
