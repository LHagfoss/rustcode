import type { JsonObject } from "../acp/jsonrpc";
import { agoText } from "../state/loops";
import { type MemoryFact, type MemoryFile, parseMemoryListing, withoutFact } from "../state/projectMemory";
import { askConfirm, request, toast } from "../state/store";
import { Action, Chip, Group, Loading, PathText, Row, useEngineData, withCode } from "./settingsKit";

/**
 * Settings › Agents › Memory: the facts agents saved with `remember` for the project chosen in
 * the scope bar, and the global ones, each with Forget. Forget all empties the project's file
 * after a confirm. Facts are read and changed on this computer's engine only.
 */
export function ProjectMemory({ project }: { project: { name: string; path: string } | null }) {
  const params: JsonObject = project ? { cwd: project.path } : {};
  const { data, error, setData, reload } = useEngineData("_rustcode/memory/list", parseMemoryListing, params);
  if (!data) {
    return (
      <Group title="Memory">
        <Loading error={error} onRetry={reload} />
      </Group>
    );
  }

  const forget = async (fact: MemoryFact, global: boolean) => {
    try {
      await request("_rustcode/memory/forget", {
        ...params,
        category: fact.category,
        key: fact.key,
        global,
      });
      setData(withoutFact(data, fact, global));
    } catch (e) {
      toast(`Could not forget ${fact.key}: ${e instanceof Error ? e.message : String(e)}`, "error");
    }
  };

  const forgetAll = (name: string) =>
    askConfirm({
      title: `Forget everything saved for ${name}?`,
      message: "Agents in this project stop seeing these facts. Global facts stay.",
      action: "Forget all",
      destructive: true,
      run: () => {
        void request("_rustcode/memory/forget", { ...params, all: true }).then(
          () => setData({ ...data, project: data.project && { ...data.project, facts: [] } }),
          (e) => toast(`Could not forget the facts: ${e instanceof Error ? e.message : String(e)}`, "error"),
        );
      },
    });

  return (
    <>
      {project && data.project && (
        <Group
          title={`Memory for ${project.name}`}
          action={
            data.project.facts.length > 0 ? (
              <Action size="sm" danger onClick={() => forgetAll(project.name)}>
                Forget all
              </Action>
            ) : undefined
          }
          footer={<PathText path={data.project.path} />}
        >
          <FactRows file={data.project} onForget={(fact) => void forget(fact, false)} />
        </Group>
      )}
      <Group
        title="Global memory"
        footer={data.global.path ? <PathText path={data.global.path} /> : undefined}
      >
        <FactRows file={data.global} onForget={(fact) => void forget(fact, true)} />
      </Group>
    </>
  );
}

function FactRows({ file, onForget }: { file: MemoryFile; onForget: (fact: MemoryFact) => void }) {
  if (file.facts.length === 0) {
    return <Row title="No saved facts" subtitle={withCode("Agents save facts with `remember`.")} />;
  }
  return (
    <>
      {file.facts.map((fact) => (
        <Row
          key={`${fact.category}/${fact.key}`}
          title={fact.category ? `${fact.category} / ${fact.key}` : fact.key}
          subtitle={fact.value}
          status={[fact.source, fact.updatedAt ? `updated ${agoText(fact.updatedAt)}` : ""]
            .filter(Boolean)
            .join(" · ")}
          badge={fact.tentative ? <Chip tone="warn">Tentative</Chip> : undefined}
        >
          <Action size="sm" onClick={() => onForget(fact)}>
            Forget
          </Action>
        </Row>
      ))}
    </>
  );
}
