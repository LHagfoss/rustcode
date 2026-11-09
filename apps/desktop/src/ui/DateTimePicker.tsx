import { useState } from "react";
import { dayStart, localDateTime, monthDays } from "./dateTime";
import { inputClass } from "./primitives";

export function DateTimePicker({ value, onChange }: { value: Date; onChange: (value: Date | null) => void }) {
  const [month, setMonth] = useState(value);
  const [day, setDay] = useState(value);
  const twelve = new Intl.DateTimeFormat(undefined, { hour: "numeric" }).resolvedOptions().hour12 === true;
  const format = (d: Date) =>
    `${String(twelve ? d.getHours() % 12 || 12 : d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}${twelve ? (d.getHours() < 12 ? " AM" : " PM") : ""}`;
  const [time, setTime] = useState(() => format(value));
  const today = new Date();
  function update(d: Date, t: string) {
    setDay(d);
    setTime(t);
    const match = /^(\d{1,2}):(\d{2})(?:\s*(AM|PM))?$/i.exec(t);
    const hour = match ? Number(match[1]) : -1;
    const resolvedHour = twelve && match ? (hour % 12) + (match[3]?.toUpperCase() === "PM" ? 12 : 0) : hour;
    onChange(
      match &&
        (twelve ? hour >= 1 && hour <= 12 && !!match[3] : hour < 24 && !match[3]) &&
        Number(match[2]) < 60
        ? localDateTime(d, resolvedHour, Number(match[2]))
        : null,
    );
  }
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <button
          type="button"
          aria-label="Previous month"
          onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() - 1, 1))}
        >
          ‹
        </button>
        <span>{month.toLocaleDateString(undefined, { month: "long", year: "numeric" })}</span>
        <button
          type="button"
          aria-label="Next month"
          onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() + 1, 1))}
        >
          ›
        </button>
      </div>
      <div className="grid grid-cols-7 gap-1">
        {Array.from({ length: 7 }, (_, i) => (
          <span key={i} className="text-center text-fg-muted">
            {new Date(2024, 0, 7 + i).toLocaleDateString(undefined, { weekday: "narrow" })}
          </span>
        ))}
        {Array.from({ length: new Date(month.getFullYear(), month.getMonth(), 1).getDay() }, (_, i) => (
          <span key={`empty-${i}`} />
        ))}
        {monthDays(month).map((d) => (
          <button
            type="button"
            key={d.getTime()}
            disabled={dayStart(d) < dayStart(today)}
            aria-label={d.toLocaleDateString(undefined, { dateStyle: "full" })}
            aria-pressed={dayStart(d) === dayStart(day)}
            className={`p-1 disabled:opacity-30 ${dayStart(d) === dayStart(day) ? "bg-fill-strong" : dayStart(d) === dayStart(today) ? "text-accent" : "hover:bg-fill"}`}
            onClick={() => update(d, time)}
          >
            {d.getDate()}
          </button>
        ))}
      </div>
      <label>
        Time {twelve ? "(HH:MM AM/PM)" : "(HH:MM)"}
        <input
          className={inputClass}
          value={time}
          onChange={(e) => update(day, e.target.value)}
          onKeyDown={(e) => {
            if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
            e.preventDefault();
            const m = /^(\d{1,2}):(\d{2})(?:\s*(AM|PM))?$/i.exec(time);
            if (!m || (twelve && !m[3])) return;
            const hour = twelve
              ? (Number(m[1]) % 12) + (m[3]?.toUpperCase() === "PM" ? 12 : 0)
              : Number(m[1]);
            const current = localDateTime(day, hour, Number(m[2]));
            if (!current) return;
            const next = new Date(current.getTime() + (e.key === "ArrowUp" ? 60000 : -60000));
            setMonth(next);
            update(next, format(next));
          }}
        />
      </label>
    </div>
  );
}
