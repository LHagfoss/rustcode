// Overlapping asks for one panel: only the newest answer may be shown. The
// Changes panel reads a thread's working tree and reads again when the thread
// or its folder changes; a slow answer for the last folder must not land on
// the new one (`ui/ChangesPanel.tsx`).

export interface Latest {
  /** The ticket for the ask that is now the newest, dropping earlier ones. */
  take(): number;
  /** Whether an answer for `ticket` is still the one wanted. */
  current(ticket: number): boolean;
  /** Drops every ask in flight, for a subject that changed. */
  drop(): void;
}

export function latest(): Latest {
  let newest = 0;
  return {
    take: () => ++newest,
    current: (ticket) => ticket === newest,
    drop: () => {
      newest += 1;
    },
  };
}
