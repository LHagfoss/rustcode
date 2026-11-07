// A message just sent rises out of the composer into its place in the transcript, so sending reads
// as one move: the row starts where the field is and lands where it belongs. Transform and opacity
// only, 220ms on the app's own ease (`--ease-snappy`), which the Web Animations call here cannot
// read from CSS. A reader who asked for less motion gets the fade alone.

/** The ease every moving thing in the app uses, `--ease-snappy` in index.css. */
const SEND_EASE = "cubic-bezier(0.2, 0.8, 0.2, 1)";
/** Its length: a row's own rise (`rc-row-in`, 260ms) cut a little shorter, since it travels further. */
export const SEND_MS = 220;
/** A fade alone, for a reader who asked for less motion. */
const SEND_FADE_MS = 160;
/**
 * The new-thread screen's message arriving (`rc-send-in` in index.css): one 160ms breath, which the
 * thread may not take the screen over before it has played (`ui/ChatView.tsx`).
 */
export const PENDING_ENTRY_MS = SEND_FADE_MS;
/** How far a row may travel: past this a long message would swing too far to read as one move. */
export const MAX_RISE = 220;
/** How far into the card a sent row starts: behind the glass, so the bubble comes out of the field. */
export const SEND_INSET = 24;

/**
 * How far a sent row travels to arrive: it starts a little inside the composer's card, whatever the gap
 * between the field and its row, so the bubble always comes out of the field. Negative when the row
 * rests above the card (a turn's own `Working` line under the message, and a reply already
 * streaming): it then starts that far below its place and rises into it, instead of sitting still
 * and only fading. Bounded both ways, so a row can never swing in from across the screen.
 */
export function sendRise(composerTop: number, rowBottom: number): number {
  const rise = composerTop + SEND_INSET - rowBottom;
  return Math.round(Math.max(-MAX_RISE, Math.min(rise, MAX_RISE)));
}

/**
 * Which sent messages have already flown. One flight per message, however many times its row is
 * drawn: the transcript remounts when a thread is reopened, and a streaming reply re-renders it on
 * every chunk, so the log lives beside the app rather than in a component.
 */
export function flightLog(): (id: string | undefined) => boolean {
  const flown = new Set<string>();
  return (id) => {
    if (!id || flown.has(id)) return false;
    flown.add(id);
    return true;
  };
}

/** The app's own log: a message sent here flies on its row's first sighting, and never again. */
export const takesFlight = flightLog();

/** Whether motion is wanted at all: a reader who asked for less gets none of it. */
export function motionAllowed(): boolean {
  return (
    typeof globalThis.matchMedia !== "function" ||
    !globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/**
 * The field the message left: it comes back empty in a breath rather than blinking out of the card.
 * The text is already gone by the time this runs, so what fades is the empty field itself.
 */
export function fadeClearedField(field: HTMLElement | null): void {
  if (!field || typeof field.animate !== "function" || !motionAllowed()) return;
  field.animate([{ opacity: 0 }, { opacity: 1 }], { duration: SEND_FADE_MS, easing: SEND_EASE });
}

/**
 * The message sent from the composer: `row` is the row it just took in the transcript. It starts at
 * the composer's own edge, at 96%, and lands in place, so nothing on the page is read twice or moves
 * the rows around it — the transform is the row's own, and the lane's height never changes.
 */
export function flyInFromComposer(row: HTMLElement | null): void {
  if (!row || typeof row.animate !== "function") return;
  // The composer's card carries its own marker (`ui/Composer.tsx`): the field it was sent from.
  const composer = document.querySelector<HTMLElement>("[data-rc-composer]");
  const from = composer
    ? sendRise(composer.getBoundingClientRect().top, row.getBoundingClientRect().bottom)
    : 0;
  const motion = motionAllowed();
  row.animate(
    [
      motion ? { opacity: 0, transform: `translateY(${from}px) scale(0.96)` } : { opacity: 0 },
      { opacity: 1, transform: "none" },
    ],
    { duration: motion ? SEND_MS : SEND_FADE_MS, easing: SEND_EASE },
  );
}
