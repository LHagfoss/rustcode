import { useMemo } from "react";
import {
  activeDelegates,
  busiestThreadDelegates,
  effectivePerThreadLimit,
  MAX_DELEGATES_PER_THREAD,
  MAX_DELEGATES_TOTAL,
  MIN_DELEGATES_PER_THREAD,
  MIN_DELEGATES_TOTAL,
  WAKE_BATCH_DEFAULT_S,
  WAKE_BATCH_MAX_S,
} from "../state/orchestration";
import { serverSetting, WORKFLOW_LIMITS } from "../state/settingsRegistry";
import { useLoadScopeSettings, useScopedSetting } from "../state/settingsScope";
import { SETTINGS_DEFAULTS, useApp } from "../state/store";
import { DelegatePresetsGroup } from "./DelegatePresets";
import { LimitFallbackGroup } from "./LimitFallback";
import { Switch } from "./primitives";
import { RestartToApply } from "./RestartToApply";
import { ScopedValue, SettingsScopeBar } from "./SettingsScope";
import { boolLabel, Group, Row, Slider } from "./settingsKit";

// A grouped nav on the left searches every row's title and keywords
// (settingsIndex.ts) and shows one page at a time beside it; picking a result
// opens its page and lights the row. After SettingsView.swift.
// Server-backed pages (Harnesses, Connections, Capabilities, Appearance's
// themes, Usage, Statistics) read and write through `_rustcode/*` requests.
// Diagnostics (DiagnosticsPage.tsx) asks the shell about the server itself.

/** Threads that start and message other threads, and what their delegates and workflows may use. */
export function OrchestrationPage() {
  useLoadScopeSettings();
  return (
    <>
      <SettingsScopeBar />
      <OrchestrationGroup />
      <WorkflowsGroup />
      <DelegatePresetsGroup />
      <LimitFallbackGroup />
    </>
  );
}

/** One workflow limit: a slider over the engine's own range, with the value in the engine's unit. */
function WorkflowLimitRow({
  setting,
  title,
  subtitle,
  hint,
  unit,
}: {
  setting: keyof typeof WORKFLOW_LIMITS;
  title: string;
  subtitle?: string;
  hint: string;
  unit: string;
}) {
  const range = WORKFLOW_LIMITS[setting];
  const fallback = serverSetting(setting).default as number;
  const scoped = useScopedSetting(setting, (value) =>
    typeof value === "number" ? String(value) : String(fallback),
  );
  const value = Math.min(range.max, Math.max(range.min, Number(scoped.value ?? fallback)));
  return (
    <Row
      title={title}
      reset={scoped.mixed || value !== fallback ? () => void scoped.set(fallback) : undefined}
      subtitle={subtitle}
      hint={hint}
    >
      <ScopedValue mixed={scoped.mixed} breakdown={scoped.breakdown} project={scoped.project}>
        <Slider
          label={title}
          value={value}
          min={range.min}
          max={range.max}
          onCommit={(next) => void scoped.set(next)}
          unit={unit}
        />
      </ScopedValue>
    </Row>
  );
}

/** Durable workflow runs (docs/workflows.md): what a run may use, read when it starts. */
function WorkflowsGroup() {
  return (
    <Group title="Workflows">
      <WorkflowLimitRow
        setting="workflow-concurrency"
        title="Workflow workers at once"
        hint="Workers are delegates of the thread that started the run, so the delegate limits above apply too. A run can ask for fewer, never more."
        unit="workers"
      />
      <WorkflowLimitRow
        setting="workflow-call-budget"
        title="Workflow agent calls per run"
        subtitle="Worker threads one run may start in all."
        hint="A call a restarted run replays from disk is not counted again. The call that goes over fails inside the script."
        unit="calls"
      />
      <WorkflowLimitRow
        setting="workflow-timeout-minutes"
        title="Workflow run time limit"
        hint="Counted from each start. When it passes, the run ends as timed out and its running workers are stopped."
        unit="minutes"
      />
      <WorkflowLimitRow
        setting="workflow-kept-runs"
        title="Finished workflow runs kept"
        subtitle="Per thread. Running runs are always kept."
        hint="When a new run starts, older finished runs past this count are deleted with their logs and results. The run list shows the newest 20."
        unit="runs"
      />
    </Group>
  );
}

/** Threads that start and message other threads (`state/host.ts`, docs/orchestrator.md). */
function OrchestrationGroup() {
  const orchestration = useScopedSetting("orchestration", boolLabel);
  const threadLimitSetting = useScopedSetting("delegates-per-thread", (value) =>
    typeof value === "number" ? String(value) : "4",
  );
  const totalLimitSetting = useScopedSetting("delegates-total", (value) =>
    typeof value === "number" ? String(value) : "8",
  );
  const wakeBatchSetting = useScopedSetting("wake-batch-seconds", (value) =>
    typeof value === "number" ? (value === 0 ? "Off" : `${value} s`) : "60 s",
  );
  const wakeBatch = Number(wakeBatchSetting.value ?? WAKE_BATCH_DEFAULT_S);
  const trustAccounts = useScopedSetting("trust-delegate-accounts", boolLabel);
  const readAcrossProjects = useScopedSetting("read-across-projects", boolLabel);
  const orchestrationValue = Boolean(orchestration.value ?? true);
  const readAcrossValue = Boolean(readAcrossProjects.value ?? true);
  const threadLimit = Number(threadLimitSetting.value ?? SETTINGS_DEFAULTS.threadLimit);
  const totalLimit = Number(totalLimitSetting.value ?? SETTINGS_DEFAULTS.totalLimit);
  const pending = useApp((s) => s.orchestrationPending);
  const links = useApp((s) => s.links);
  const running = useMemo(() => activeDelegates(links), [links]);
  const busiest = useMemo(() => busiestThreadDelegates(links), [links]);
  const possible = effectivePerThreadLimit({ perThread: threadLimit, total: totalLimit });
  return (
    <>
      <Group title="Orchestration">
        <Row
          title="Let delegates use my named accounts without asking"
          subtitle="Named logins on this computer."
          hint="Accounts on other computers still need approval."
        >
          <ScopedValue
            mixed={trustAccounts.mixed}
            breakdown={trustAccounts.breakdown}
            project={trustAccounts.project}
          >
            <Switch
              label="Let delegates use my named accounts without asking"
              checked={Boolean(trustAccounts.value ?? false)}
              onChange={(value) => void trustAccounts.set(value)}
            />
          </ScopedValue>
        </Row>
        <Row
          title="Threads can start and message other threads"
          reset={
            orchestration.mixed || !orchestrationValue
              ? () => void orchestration.set(SETTINGS_DEFAULTS.orchestration)
              : undefined
          }
          subtitle="Read, message and start threads, and compare a task across harnesses."
          hint="Full access does this without asking; Auto-edit may list, read and wait; otherwise you approve each. A thread it starts gets the lower of this thread's access and Default access."
        >
          <ScopedValue
            mixed={orchestration.mixed}
            breakdown={orchestration.breakdown}
            project={orchestration.project}
          >
            <Switch
              label="Threads can start and message other threads"
              checked={orchestrationValue}
              onChange={(value) => void orchestration.set(value)}
            />
          </ScopedValue>
        </Row>
        <Row
          title="Let agents read threads in other projects"
          reset={
            readAcrossProjects.mixed || !readAcrossValue
              ? () => void readAcrossProjects.set(SETTINGS_DEFAULTS.readAcrossProjects)
              : undefined
          }
          hint="An agent can list, search and read any thread, in any project. Reading still follows the approval rules above. Messaging and starting threads stay in the project."
        >
          <ScopedValue
            mixed={readAcrossProjects.mixed}
            breakdown={readAcrossProjects.breakdown}
            project={readAcrossProjects.project}
          >
            <Switch
              label="Let agents read threads in other projects"
              checked={readAcrossValue}
              onChange={(value) => void readAcrossProjects.set(value)}
            />
          </ScopedValue>
        </Row>
        {pending && <RestartToApply />}
      </Group>
      <Group
        title="Delegation"
        disabled={!orchestrationValue}
        footer={
          orchestrationValue
            ? "Only live delegates take a slot. A follow-up to a thread this one already runs takes none."
            : "These limits have no effect while “Threads can start and message other threads” is off."
        }
      >
        <Row
          title="Delegates across all threads"
          reset={
            totalLimitSetting.mixed || totalLimit !== SETTINGS_DEFAULTS.totalLimit
              ? () => void totalLimitSetting.set(SETTINGS_DEFAULTS.totalLimit)
              : undefined
          }
          subtitle="Running at once, whichever thread started them."
          status={
            running === 0
              ? "No delegates running now"
              : running === 1
                ? "1 running now"
                : `${running} running now`
          }
        >
          <ScopedValue
            mixed={totalLimitSetting.mixed}
            breakdown={totalLimitSetting.breakdown}
            project={totalLimitSetting.project}
          >
            <Slider
              label="Delegates across all threads"
              value={totalLimit}
              min={MIN_DELEGATES_TOTAL}
              max={MAX_DELEGATES_TOTAL}
              onCommit={(value) => void totalLimitSetting.set(value)}
              unit="delegates"
            />
          </ScopedValue>
        </Row>
        <Row
          title="Delegates one thread can run at once"
          reset={
            threadLimitSetting.mixed || threadLimit !== SETTINGS_DEFAULTS.threadLimit
              ? () => void threadLimitSetting.set(SETTINGS_DEFAULTS.threadLimit)
              : undefined
          }
          hint="The total above can make this lower."
          status={
            threadLimit > totalLimit
              ? `${threadLimit} set, ${possible} possible while the total is ${totalLimit}`
              : busiest === 0
                ? "No delegates on any thread now"
                : `The busiest thread has ${busiest} now`
          }
        >
          <ScopedValue
            mixed={threadLimitSetting.mixed}
            breakdown={threadLimitSetting.breakdown}
            project={threadLimitSetting.project}
          >
            <Slider
              label="Delegates one thread can run at once"
              value={threadLimit}
              min={MIN_DELEGATES_PER_THREAD}
              max={MAX_DELEGATES_PER_THREAD}
              onCommit={(value) => void threadLimitSetting.set(value)}
              unit="delegates"
            />
          </ScopedValue>
        </Row>
        <Row
          title="Merge delegate wakes within"
          reset={
            wakeBatchSetting.mixed || wakeBatch !== WAKE_BATCH_DEFAULT_S
              ? () => void wakeBatchSetting.set(WAKE_BATCH_DEFAULT_S)
              : undefined
          }
          subtitle="Delegates that finish or stall in this window wake the thread once."
          hint="Each wake is a model turn that resends the thread's whole context. A question for you is never held back. Off sends each as it comes."
        >
          <ScopedValue
            mixed={wakeBatchSetting.mixed}
            breakdown={wakeBatchSetting.breakdown}
            project={wakeBatchSetting.project}
          >
            <Slider
              label="Merge delegate wakes within"
              value={wakeBatch}
              min={0}
              max={WAKE_BATCH_MAX_S}
              step={15}
              onCommit={(value) => void wakeBatchSetting.set(value)}
              unit="seconds"
            />
          </ScopedValue>
        </Row>
      </Group>
    </>
  );
}
