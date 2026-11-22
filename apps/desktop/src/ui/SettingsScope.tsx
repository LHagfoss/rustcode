// **Applying to** at the top of a Settings page with server settings, and
// **Showing** on a machine-only page (docs/desktop.md › Settings on one
// machine or all). The choice is one for the whole panel; All machines offers
// every connected machine and leaves the offline ones alone.
//
// A row whose connected machines disagree draws `MixedChip` instead of one
// value, with each machine's value in its hover card.

import type { ReactNode } from "react";
import { type ScopeBreakdownEntry, SOURCE_LABEL } from "../state/settingsRegistry";
import {
  type ProjectRowState,
  projectPathOn,
  scopeLabel,
  useProjectScope,
  useScope,
  useScopeProjects,
} from "../state/settingsScope";
import { Layers } from "./icons";
import { Tooltip } from "./primitives";
import { Action, HintTip, Select } from "./settingsKit";

const THIS = "this";
const ALL = "all";
const ALL_PROJECTS = "";

/** The scope picker; `machineOnly` drops All machines and says Showing. */
export function SettingsScopeBar({ machineOnly = false }: { machineOnly?: boolean }) {
  const { scope, setScope, machines, targets } = useScope();
  const project = useProjectScope((state) => state.project);
  const setProject = useProjectScope((state) => state.setProject);
  const projects = useScopeProjects();
  const projectOptions = [
    { value: ALL_PROJECTS, label: "All projects" },
    ...(project && !projects.some((entry) => entry.path === project.path) ? [project] : [])
      .concat(projects)
      .map((entry) => ({ value: entry.path, label: entry.name })),
  ];
  const skipped =
    project && !machineOnly ? targets.filter((machine) => !projectPathOn(project, machine.host)) : [];
  const value = machineOnly
    ? scope.mode === "machine"
      ? `machine:${scope.host}`
      : THIS
    : scope.mode === "all"
      ? ALL
      : scope.mode === "machine"
        ? `machine:${scope.host}`
        : THIS;
  const offline = scope.mode === "all" ? machines.filter((machine) => !machine.online) : [];
  const shown =
    scope.mode === "machine" ? machines.find((machine) => machine.host === scope.host) : undefined;
  const options = [
    { value: THIS, label: "This machine" },
    ...machines
      .filter((machine) => !machine.local)
      .map((machine) => ({
        value: `machine:${machine.host}`,
        label: machine.online ? machine.name : `${machine.name} · offline`,
      })),
    ...(machineOnly ? [] : [{ value: ALL, label: "All machines" }]),
  ];
  return (
    <div className="mb-6 flex flex-col gap-2 rounded-xl border border-border bg-surface px-4 py-3">
      <div className="flex flex-wrap items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5 font-medium text-body text-fg">
            {machineOnly ? "Showing" : "Applying to"}
            <HintTip label={machineOnly ? "Showing" : "Applying to"}>
              {machineOnly
                ? "These settings belong to the machine you pick."
                : "Server settings change on the machine you pick. App preferences stay on this computer. A project changes only the rows a project can set; the rest apply to every project."}
            </HintTip>
          </div>
          {/* Only a picked project changes what a row means, so only then is there a line to read. */}
          {!machineOnly && project && (
            <div className="max-w-[70ch] text-callout text-fg-muted">
              Project rows change for {project.name} only.
            </div>
          )}
        </div>
        {!machineOnly && (
          <Select
            label="Project"
            value={project?.path ?? ALL_PROJECTS}
            options={projectOptions}
            onChange={(next) => setProject(projects.find((entry) => entry.path === next) ?? null)}
          />
        )}
        <Select
          label={machineOnly ? "Showing machine" : "Applying to"}
          value={value}
          options={options}
          onChange={(next) => {
            if (next === ALL) setScope({ mode: "all" });
            else if (next === THIS) setScope({ mode: "this" });
            else setScope({ mode: "machine", host: next.slice("machine:".length) });
          }}
        />
      </div>
      {offline.length > 0 && (
        <p className="m-0 text-caption text-fg-muted">
          {offline.map((machine) => `${machine.name} is offline`).join(", ")} —{" "}
          {offline.length === 1 ? "it keeps its value." : "they keep their values."}
        </p>
      )}
      {skipped.length > 0 && (
        <p className="m-0 text-caption text-fg-muted">
          {skipped.map((machine) => machine.name).join(", ")}{" "}
          {skipped.length === 1 ? "does not have" : "do not have"} {project?.name}: skipped.
        </p>
      )}
      {shown && !shown.online && (
        <p className="m-0 text-caption text-warning">
          {shown.name} is offline. Its last values are shown, and a change here is not sent.
        </p>
      )}
    </div>
  );
}

/** `Mixed` with the layers mark: the connected machines disagree, and the hover card says how. */
export function MixedChip({ breakdown }: { breakdown: ScopeBreakdownEntry[] }) {
  return (
    <Tooltip
      side="top"
      label={
        <span className="flex flex-col gap-0.5">
          {breakdown.map((entry) => (
            <span key={entry.host || "local"} className="block">
              <span className="font-medium">{entry.name}:</span> {entry.label}
            </span>
          ))}
        </span>
      }
    >
      <span className="inline-flex h-6 shrink-0 items-center gap-1 rounded-full bg-warning/12 px-2 font-medium text-caption text-warning">
        <Layers size={12} />
        Mixed
      </span>
    </Tooltip>
  );
}

/**
 * Where a project row's value comes from (built-in default, the machine, or the project), with
 * the same layers mark and hover card as Mixed, and "Use the machine's value" for an override.
 */
export function SourceChip({ project }: { project: ProjectRowState }) {
  return (
    <span className="flex shrink-0 items-center gap-2">
      <Tooltip
        side="top"
        label={
          <span className="flex flex-col gap-0.5">
            <span className="block font-medium">{project.name}</span>
            {project.breakdown.map((entry) => (
              <span key={entry.host || "local"} className="block">
                <span className="font-medium">{entry.name}:</span> {entry.label}
              </span>
            ))}
          </span>
        }
      >
        <span
          className={`inline-flex h-6 shrink-0 items-center gap-1 rounded-full px-2 font-medium text-caption ${
            project.source === "project" ? "bg-accent/12 text-accent" : "bg-fill-subtle text-fg-muted"
          }`}
        >
          <Layers size={12} />
          {SOURCE_LABEL[project.source]}
        </span>
      </Tooltip>
      {project.clear && (
        <Action quiet size="sm" onClick={() => void project.clear?.()}>
          Use the machine's value
        </Action>
      )}
    </span>
  );
}

/** The value a row shows: the control, or the Mixed chip above it; with a project, its source. */
export function ScopedValue({
  mixed,
  breakdown,
  project = null,
  children,
}: {
  mixed: boolean;
  breakdown: ScopeBreakdownEntry[];
  project?: ProjectRowState | null;
  children: ReactNode;
}) {
  if (project?.readOnly)
    return (
      <div className="flex shrink-0 items-center gap-2">
        <span className="max-w-[28ch] text-right text-caption text-fg-muted">{project.readOnly}</span>
        <div aria-disabled className="pointer-events-none opacity-50">
          {children}
        </div>
      </div>
    );
  return (
    <div className="flex shrink-0 items-center gap-2">
      {project ? <SourceChip project={project} /> : mixed && <MixedChip breakdown={breakdown} />}
      {children}
    </div>
  );
}

export { scopeLabel };
