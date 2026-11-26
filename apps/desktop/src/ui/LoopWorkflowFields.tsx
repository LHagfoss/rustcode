/** A workflow job's fields in the job editor (issue 4443): the saved workflow and its args. */
import { useEffect, useState } from "react";
import { savedWorkflows, workflowArgsProblem } from "../state/workflowJobs";
import { noTextHelpers, Select } from "./primitives";

function Labeled({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <span className="font-medium text-caption text-fg-muted">{label}</span>
      {children}
    </div>
  );
}

export function LoopWorkflowFields({
  workspace,
  host,
  workflow,
  args,
  onChange,
}: {
  workspace: string;
  host: string;
  workflow: string;
  args: string | undefined;
  onChange: (change: { workflow?: string; args?: string }) => void;
}) {
  const [names, setNames] = useState<string[] | null>(null);
  useEffect(() => {
    let live = true;
    setNames(null);
    if (!workspace) return;
    void savedWorkflows(workspace, host).then((found) => {
      if (live) setNames(found);
    });
    return () => {
      live = false;
    };
  }, [workspace, host]);
  const listed = names ?? [];
  const options = listed.map((name) => ({ value: name, label: name }));
  if (workflow && !listed.includes(workflow)) {
    options.unshift({ value: workflow, label: names ? `${workflow} (not found)` : workflow });
  }
  const argsProblem = workflowArgsProblem(args);
  return (
    <>
      <Labeled label="Workflow">
        {options.length ? (
          <Select
            label="Workflow"
            value={workflow}
            options={
              workflow ? options : [{ value: "", label: "Choose a workflow", disabled: true }, ...options]
            }
            onChange={(name) => onChange({ workflow: name })}
            className="max-w-full self-start"
          />
        ) : (
          <p className="text-caption text-fg-muted">
            {names === null
              ? "Looking for workflows…"
              : "This project has no workflows in .rustcode/workflows/ yet."}
          </p>
        )}
        <p className="text-caption text-fg-muted">
          Each run starts a new thread and runs the workflow there. The run ends when the workflow does.
        </p>
      </Labeled>
      <Labeled label="Args">
        <textarea
          value={args ?? ""}
          onChange={(e) => onChange({ args: e.target.value })}
          placeholder='{"label": "bug"}'
          aria-label="Args"
          rows={2}
          {...noTextHelpers}
          className="max-h-40 min-h-12 resize-y rounded-lg bg-fill-subtle px-3 py-2 font-mono text-caption text-fg placeholder:text-fg-faint hover:bg-fill focus:bg-fill"
        />
        <p className={argsProblem ? "text-caption text-danger" : "text-caption text-fg-muted"}>
          {argsProblem ?? "A webhook run adds the delivery as args.webhook."}
        </p>
      </Labeled>
    </>
  );
}
