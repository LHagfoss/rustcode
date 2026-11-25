import { useConfigIssues } from "../state/configIssues";
import { Callout } from "./primitives";

/** Settings' notice for config files the engine could not read and is running without. */
export function ConfigIssueNotice() {
  const issues = useConfigIssues((state) => state.issues);
  if (!issues.length) return null;
  return (
    <div className="mb-6 flex flex-col gap-2">
      {issues.map((issue) => (
        <Callout key={issue.path} tone="warning">
          {issue.scope === "user"
            ? `config.toml could not be read: ${issue.message}. RustCode is using its defaults and will not save settings until the file is fixed.`
            : `A project config could not be read and is skipped: ${issue.message}.`}
        </Callout>
      ))}
    </div>
  );
}
