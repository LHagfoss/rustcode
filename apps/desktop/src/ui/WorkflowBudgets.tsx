import { useEffect, useState } from "react";
import { useScopedSetting } from "../state/settingsScope";
import { parseBudget, WORKFLOW_BUDGETS, type WorkflowBudgetKey } from "../state/workflowBudgets";
import { cx, noTextHelpers } from "./primitives";
import { ScopedValue } from "./SettingsScope";
import { fieldClass, Row } from "./settingsKit";

/**
 * One workflow spend limit: a number field, empty for none. It is saved when the field loses focus
 * or on Return; something that is not a number goes back to what was saved.
 */
function BudgetRow({
  setting,
  title,
  hint,
  placeholder,
}: {
  setting: WorkflowBudgetKey;
  title: string;
  hint: string;
  placeholder: string;
}) {
  const scoped = useScopedSetting(setting, (value) =>
    typeof value === "number" && value > 0 ? String(value) : "None",
  );
  const saved = typeof scoped.value === "number" && scoped.value > 0 ? String(scoped.value) : "";
  const [text, setText] = useState(saved);
  useEffect(() => setText(saved), [saved]);
  const commit = () => {
    const next = parseBudget(setting, text);
    if (next === null) {
      setText(saved);
      return;
    }
    if (String(next || "") !== saved) void scoped.set(next);
  };
  return (
    <Row title={title} hint={hint} reset={saved ? () => void scoped.set(0) : undefined}>
      <ScopedValue mixed={scoped.mixed} breakdown={scoped.breakdown} project={scoped.project}>
        <input
          value={text}
          onChange={(event) => setText(event.target.value)}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === "Enter") commit();
          }}
          placeholder={placeholder}
          aria-label={title}
          inputMode={WORKFLOW_BUDGETS[setting].decimal ? "decimal" : "numeric"}
          {...noTextHelpers}
          className={cx(fieldClass, "h-7 w-32 px-2 text-right font-mono")}
        />
      </ScopedValue>
    </Row>
  );
}

/** The token and reported-cost limits of a workflow run (docs/workflows.md › Limits). */
export function WorkflowBudgetRows() {
  return (
    <>
      <BudgetRow
        setting="workflow-token-budget"
        title="Workflow run token budget"
        hint="Input plus output tokens the run's workers' harnesses reported. Once reached, no further agent() call starts; running workers finish. Empty is no limit."
        placeholder="No limit"
      />
      <BudgetRow
        setting="workflow-cost-budget-usd"
        title="Workflow run cost budget, USD"
        hint="Only the cost a harness reported counts: one that reports none adds nothing. Once reached, no further agent() call starts. Empty is no limit."
        placeholder="No limit"
      />
    </>
  );
}
