/** Quick actions are one visual row; ↑↓ enter the list and ←→ move across the row. */
export function quickActionTarget(
  key: string,
  current: number,
  quickCount: number,
  total: number,
): number | null {
  if (!quickCount || current < 0) return null;
  if (current < quickCount) {
    if (key === "ArrowRight") return (current + 1) % quickCount;
    if (key === "ArrowLeft") return (current + quickCount - 1) % quickCount;
    if (key === "ArrowDown") return quickCount < total ? quickCount : current;
    if (key === "ArrowUp") return total > quickCount ? total - 1 : current;
  }
  if (current === quickCount && key === "ArrowUp") return quickCount - 1;
  return null;
}
