export function localDateTime(day: Date, hour: number, minute: number): Date | null {
  const d = new Date(day.getFullYear(), day.getMonth(), day.getDate(), hour, minute);
  return d.getHours() === hour && d.getMinutes() === minute ? d : null;
}
export function monthDays(month: Date): Date[] {
  const first = new Date(month.getFullYear(), month.getMonth(), 1);
  const count = new Date(month.getFullYear(), month.getMonth() + 1, 0).getDate();
  return Array.from({ length: count }, (_, i) => new Date(first.getFullYear(), first.getMonth(), i + 1));
}
export function dayStart(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}
