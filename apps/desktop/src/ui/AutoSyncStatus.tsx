import { type AutoSyncKind, type AutoSyncStatus, autoSyncLine, useAutoSync } from "../state/autoSyncStatus";
import { Chip, Group, Row } from "./settingsKit";

const ROWS: { kind: AutoSyncKind; title: string }[] = [
  { kind: "follow", title: "Settings and files" },
  { kind: "backfill", title: "Thread copies" },
];

const CHIPS: Record<AutoSyncStatus["outcome"], { tone: "ok" | "warn" | "bad"; label: string }> = {
  synced: { tone: "ok", label: "Synced" },
  skipped: { tone: "warn", label: "Skipped" },
  failed: { tone: "bad", label: "Failed" },
};

/** What the automatic syncs last did with this machine (state/autoSyncStatus.ts); nothing until one ran. */
export function AutoSyncStatusGroup({ id }: { id: string }) {
  const statuses = useAutoSync((state) => state.byMachine[id]);
  const rows = ROWS.flatMap(({ kind, title }) => {
    const status = statuses?.[kind];
    const line = autoSyncLine(status);
    return status && line ? [{ kind, title, status, line }] : [];
  });
  if (!rows.length) return null;
  return (
    <Group title="Automatic sync">
      {rows.map(({ kind, title, status, line }) => (
        <Row
          key={kind}
          title={title}
          subtitle={line}
          badge={<Chip tone={CHIPS[status.outcome].tone}>{CHIPS[status.outcome].label}</Chip>}
        />
      ))}
    </Group>
  );
}
