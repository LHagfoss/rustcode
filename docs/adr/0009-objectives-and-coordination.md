# 0009: Objectives run as workflow runs, and the engine owns them

Status: proposed, 2026-11-26 (#4388). Code: `rustcode/engine/src/workflow.rs`,
`rustcode/engine/src/workflow/`, `rustcode/engine/src/orchestration/local*.rs`,
`rustcode/engine/src/daemon/`, `apps/desktop/src/state/workflows.ts`,
`apps/desktop/src/state/host.ts`, `apps/desktop/src/state/orchestration.ts`.
Docs: [workflows.md](../workflows.md), [orchestrator.md](../orchestrator.md),
[multi-agent.md](../multi-agent.md), [daemon.md](../daemon.md),
[terminology.md](../terminology.md).

## Why

A user wants to give RustCode a goal ("migrate the settings page to the new
store, with tests, and open one PR") and have it split the work, run
implementation threads, review and verify them, retry what failed, ask the
user at the points that need a person, and stop: within a budget, across a
restart, and without the desktop app having to stay open.

RustCode already has four mechanisms that each do part of that. They grew
separately and each has its own owner after a restart:

| Mechanism | Defined in | Durable state | Driven after a restart by |
| --- | --- | --- | --- |
| Subagent (`spawn_agent`) | [multi-agent.md](../multi-agent.md) | `agents.json` in the thread | nobody: marked `interrupted` |
| Delegate (`start_session`, `fan_out`, `message_session`) | [orchestrator.md](../orchestrator.md) | link book in the data store | the app (`resumeInterrupted`), or the engine for its own `engine: true` links |
| Workflow run (`run_workflow`) | [workflows.md](../workflows.md) | `workflows/<run>/` under the starting thread | the engine: the process the thread registers with, or the startup sweep for Full-access threads (#4387) |
| Job (`loop` action) | [daemon.md](../daemon.md) | `daemon/jobs.sqlite`, leased runs | the scheduler daemon or the server |

What is missing for an objective is not a fifth mechanism. It is a decision
on which existing one holds the plan, who drives it when no app is open, and
the few primitives the plan lacks (a human checkpoint, a trigger from a job).
Without that decision the natural next step is a "coordinator" or "team"
feature that duplicates workflows with a different word, which is what this
record exists to prevent.

## Decision

### 1. Vocabulary

- An **objective** is what the user asks for: a goal, the checks that say it
  is met, and a budget. It is not a stored object of its own. It is the
  `args` of a workflow run.
- A **workflow run** carries out an objective. Its script holds the plan:
  phases, task order, which calls run in parallel, retries, checkpoints.
- The thread that started the run is its **coordinator thread**. Nothing
  else is called a coordinator; *orchestrator* stays the word for any thread
  that delegates ([orchestrator.md](../orchestrator.md)), and *parent* for
  the thread above a delegate.
- Each `agent()` call starts a **worker**, which is a **delegate** like any
  other: a thread in the sidebar, nested under the coordinator thread.
- **Swarm**, **team** and **crew** are not product terms. A group of workers
  is a run's phase; a group of threads is a project or a folder.

`docs/terminology.md` lists these.

### 2. The model: an objective is a workflow script

The plan of an objective is a workflow script, so it reuses the run store,
deterministic replay, call keys, limits and the run card that exist today
([workflows.md](../workflows.md)). A task graph is JavaScript: `await` is a
sequential edge, `parallel` a fan-out, and a task that depends on two others
awaits both. No graph format, scheduler or DSL is added; a script that wants
an explicit `depends_on` list sorts it itself in a few lines.

RustCode ships one template, `.rustcode/workflows/objective.js` written by a
later issue, with the shape:

```js
phase("plan");
const plan = await agent(args.goal, { preset: "planner", schema: PLAN });
phase("implement");
const done = await parallel(plan.tasks.map((t) => () => attempt(t, args.retries ?? 1)));
phase("verify");
const verdict = await agent(verifyPrompt(args.checks, done), {
  preset: "reviewer", access: "ask", schema: VERDICT,
});
if (!verdict.met) await checkpoint("The checks are not met. Continue anyway?", ["Stop", "Continue"]);
return { done, verdict };
```

A user starts an objective by asking the agent in a thread ("run the
objective workflow for …"), which calls `run_workflow` with `name:
"objective"` and the goal in `args`; that thread is then the coordinator.

### 3. Roles are delegate presets

Planner, implementer, reviewer and verifier are **delegate presets**
(`delegate-presets`, Settings › Delegation), which `agent()` accepts since
#4384 with `preset`, `access`, `account`, `project` and `worktree`. There are
no built-in role types. A reviewer or verifier runs at `access: "ask"`, which
only lowers what the worker gets.

### 4. Human checkpoints are a workflow global

A `checkpoint(question, options)` global waits for the user durably. It is
keyed and recorded in `calls.jsonl` like an `agent()` call, so a resumed run
answers it from the record and never asks twice. It is asked through the path
`ask_user_question` already uses (the app's question card, and
`list_pending_requests` / `answer_request` for an orchestrator), on the
coordinator thread. With no app attached it waits; the run shows *Needs you*,
and its timeout does not run while it waits. Follow-up issue: #4442.

### 5. Verification is a worker, not a sandbox feature

The script sandbox stays without shell, network or clock
([workflows.md](../workflows.md) › The sandbox): that is what makes replay
deterministic. Verification is an `agent()` call with a `schema` verdict,
which runs the tests in its own thread under its own approvals, and may read
CI with the orchestration tools (`thread_status` gives a delegate's PR
checks). A run's result is never marked met by the script's own say-so.

### 6. The engine owns durable execution; the app views and approves

- A workflow run is driven by the engine process its thread registered with,
  under a lease (`driver.json`, #4387), and an engine that starts resumes
  undriven runs of Full-access threads. The background server
  (`rc server install`) is therefore the place a long objective lives.
- What still needs the app, and why: approval cards for a thread below Full
  access (nobody else can approve), the delegate wake rules and stall notices
  in `state/host.ts`, and remote machines' connections. The no-app path keeps
  answering what needs no person: starting at Full access, waiting,
  interrupting (#4381), and waking the coordinator when its run or engine
  links end (#4382).
- The app remains first whenever it is attached; nothing in this record
  moves a decision away from the user.

### 7. Stopping conditions and loop prevention

A run stops at the first of: the script returns or throws; its call budget
(`workflow-call-budget`); its token or reported-cost budget (#4385, counted
only from what harnesses report); its timeout; `stop_workflow`; or the user's
Stop. Retries are the script's own bounded loops (`attempt(t, n)` above),
plus **Retry** on a failed run (#4383), which re-runs only failed calls.
A script that repeats the same failing call reaches the call budget, because
each retry of a failed key is a new attempt counted once. Delegation depth
stays at two levels, and a wake never creates a link, so a wake cannot loop.

### 8. Cross-harness and cross-machine

Workers already run on any harness (`harness`, or a preset's). Workers on
another machine are left out for now: `start_session` with `machine` needs
the app's connection to that machine, and the no-app path refuses it
([orchestrator.md](../orchestrator.md) › No app attached). Running a worker
on a hub member is the follow-up, on the same `start_session` the hub already
serves for jobs ([daemon.md](../daemon.md) › Across paired machines), under
the access the member granted.

### 9. Triggers: a job may start a workflow run

A job action `workflow` (`{ name, args, workspace }`) starts a run of a
project workflow in a fresh coordinator thread, on a schedule, by hand or
from a webhook; the webhook body and event (#4386) go into `args`. The job's
run records the workflow run id and ends when the run ends. Follow-up issue:
#4443.

### Relation to the execution graph (#2882)

[The execution graph](../architecture/execution-graph.md) names a *Run* and
*ExecutionNode*s under a thread. A workflow run is such a Run, and each
`agent()` call and checkpoint an ExecutionNode. This record adds no new
storage: when the graph's store lands, the run folder maps onto it.

## Alternatives considered

- **A persistent coordinator thread whose model loops over the plan.** The
  plan then lives in a model's context, which compaction and restarts lose
  (the reason workflows exist, #3394), and each step costs a full-context
  turn.
- **A declarative graph (YAML or JSON tasks with `depends_on`).** It needs a
  scheduler, a format and an editor, and cannot express a retry loop or a
  conditional checkpoint without growing into a language. The script already
  is one.
- **A "team" object with standing members.** Members are presets and threads
  already have status, wakes and links; a team adds a name for a phase.

## Follow-ups

- #4442: `checkpoint()` workflow global.
- #4443: `workflow` job action.
- Workers on hub members (cross-machine), after both.
