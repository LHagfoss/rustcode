/**
 * Settings › Statistics › Your activity: lifetime tokens, the peak day, the
 * longest turn, both streaks, and a year of days as a grid
 * (`state/activityProfile.ts`). It reads every day recorded, whatever period
 * the page below is set to.
 */

import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { UsageReport } from "../acp/rustcode";
import { count } from "../lib/plural";
import {
  monthLabels,
  PROFILE_MODES,
  PROFILE_WEEKS,
  type ProfileCell,
  type ProfileMode,
  profileSummary,
  profileWeeks,
  shortTokens,
} from "../state/activityProfile";
import { formatDuration } from "../state/threadFormat";
import { Flame } from "./icons";
import { cx, Skeleton, Tooltip } from "./primitives";
import { Chip, Choice, Group } from "./settingsKit";

const SHADE: Record<ProfileCell["level"], string> = {
  0: "bg-fill-subtle",
  1: "bg-accent/25",
  2: "bg-accent/45",
  3: "bg-accent/70",
  4: "bg-accent",
};

const longDay = (day: string) =>
  new Date(`${day}T00:00:00Z`).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
    timeZone: "UTC",
  });

function Stat({ value, label, tip, lit }: { value: string; label: string; tip: string; lit?: boolean }) {
  return (
    <Tooltip label={tip}>
      <div className="flex min-w-0 flex-col items-center gap-0.5 px-2 py-3 text-center">
        <span
          className={cx(
            "flex items-center gap-1 font-semibold text-body tabular",
            lit ? "text-accent" : "text-fg",
          )}
        >
          {lit && <Flame size={13} className="shrink-0" />}
          {value}
        </span>
        <span className="truncate text-caption text-fg-muted">{label}</span>
      </div>
    </Tooltip>
  );
}

function cellTip(cell: ProfileCell, mode: ProfileMode): string {
  const day = `${longDay(cell.day)} · ${shortTokens(cell.tokens)} tokens · ${count(cell.turns, "turn")}`;
  if (mode === "weekly") return `${day} · week ${shortTokens(cell.value)} tokens`;
  if (mode === "cumulative") return `${day} · ${shortTokens(cell.value)} tokens so far`;
  return day;
}

export function ActivityProfile({ report, error }: { report: UsageReport | null; error?: string | null }) {
  const [mode, setMode] = useState<ProfileMode>("daily");
  const scroller = useRef<HTMLDivElement>(null);
  const rows = report?.rows;
  const summary = useMemo(() => (rows ? profileSummary(rows) : undefined), [rows]);
  const columns = useMemo(() => (rows ? profileWeeks(rows, mode) : []), [rows, mode]);
  const months = useMemo(() => monthLabels(columns), [columns]);
  const loaded = Boolean(rows);
  useLayoutEffect(() => {
    const el = scroller.current;
    if (loaded && el) el.scrollLeft = el.scrollWidth;
  }, [loaded]);

  const streak = summary?.streak;
  const best =
    streak && streak.longest > streak.current ? ` Your best is ${count(streak.longest, "day")}.` : "";
  const streakTip = !streak
    ? ""
    : streak.current === 0
      ? `No streak right now.${best} Run a turn today to start one.`
      : streak.today
        ? `${count(streak.current, "day")} in a row. Today counts.${best}`
        : `${count(streak.current, "day")} in a row. Run a turn today to keep it.${best}`;

  return (
    <Group
      title="Your activity"
      action={
        <div className="flex items-center gap-2">
          {report?.truncated && (
            <Tooltip label="The day read stopped at its row cap, so the grid may miss the earliest days">
              <span>
                <Chip tone="warn">Day read capped</Chip>
              </span>
            </Tooltip>
          )}
          <Choice<ProfileMode>
            label="Grid"
            value={mode}
            options={PROFILE_MODES.map((one) => ({ value: one.value, label: one.label }))}
            onChange={setMode}
          />
        </div>
      }
      footer={
        error
          ? `Could not read every day: ${error}`
          : summary?.since
            ? `Every day since ${longDay(summary.since)}, on every machine in scope, by this computer's clock.`
            : "Every day recorded, on every machine in scope, by this computer's clock."
      }
    >
      <div>
        {!summary || !streak ? (
          <div className="grid grid-cols-2 gap-px p-3 @min-[40rem]:grid-cols-5">
            {Array.from({ length: 5 }, (_, index) => (
              <Skeleton key={index} className="h-10 w-full" />
            ))}
          </div>
        ) : (
          <div className="grid grid-cols-2 border-border border-b @min-[40rem]:grid-cols-5 @min-[40rem]:divide-x @min-[40rem]:divide-border">
            <Stat
              value={shortTokens(summary.lifetimeTokens)}
              label="Lifetime tokens"
              tip={`${summary.lifetimeTokens.toLocaleString("en-US")} input and output tokens over ${count(summary.lifetimeTurns, "turn")}`}
            />
            <Stat
              value={summary.peak ? shortTokens(summary.peak.tokens) : "—"}
              label="Peak day"
              tip={
                summary.peak
                  ? `${summary.peak.tokens.toLocaleString("en-US")} tokens on ${longDay(summary.peak.day)}`
                  : "No day reported tokens yet"
              }
            />
            <Stat
              value={summary.longestTurnMs === undefined ? "—" : formatDuration(summary.longestTurnMs)}
              label="Longest turn"
              tip={
                summary.longestTurnMs === undefined
                  ? "No turn measured its wall time yet"
                  : "The longest single turn, start to finish"
              }
            />
            <Stat
              value={count(streak.longest, "day")}
              label="Longest streak"
              tip={`The most days in a row with a turn, over ${count(summary.activeDays, "active day")}`}
            />
            <Stat
              value={count(streak.current, "day")}
              label="Current streak"
              tip={streakTip}
              lit={streak.today}
            />
          </div>
        )}
        <div ref={scroller} className={cx("overflow-x-auto", !rows && "p-4")}>
          {!rows ? (
            <Skeleton className="h-[110px] w-full" />
          ) : (
            <div
              role="img"
              aria-label={`Tokens per day over the last ${PROFILE_WEEKS} weeks, ${mode}.`}
              className="flex min-w-[932px] flex-col gap-1.5 p-4"
            >
              <div className="flex gap-[3px]">
                {columns.map((week, index) => (
                  <div
                    key={week.find(Boolean)?.day ?? index}
                    className="flex min-w-0 flex-1 flex-col gap-[3px]"
                  >
                    {week.map((cell, day) =>
                      cell ? (
                        <Tooltip key={cell.day} label={cellTip(cell, mode)}>
                          <span className={cx("rc-heat-cell aspect-square w-full", SHADE[cell.level])} />
                        </Tooltip>
                      ) : (
                        <span key={`future-${index}-${day}`} className="aspect-square w-full" />
                      ),
                    )}
                  </div>
                ))}
              </div>
              <div className="flex gap-[3px] text-caption text-fg-muted">
                {months.map((month, index) => (
                  <span
                    key={columns[index]?.find(Boolean)?.day ?? index}
                    className="min-w-0 flex-1 overflow-visible whitespace-nowrap"
                  >
                    {month}
                  </span>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>
    </Group>
  );
}
