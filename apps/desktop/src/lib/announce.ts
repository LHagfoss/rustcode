// What a screen reader hears of the app on its own: two visually hidden live regions, one polite
// and one assertive, mounted once at the root (issue 3609). A thread's transcript does not speak as
// it streams (`ui/Transcript.tsx` turns its log off); its moments that matter (a finish, a question,
// an approval, a failure) are said here, once each, in a few words (`state/announceThreads.ts`).

/** Messages that arrive this close together are said as one, so a burst is one announcement. */
export const COALESCE_MS = 1_500;

/** The same words said again within this long are a repeat, and dropped. */
export const REPEAT_MS = 10_000;

export interface AnnounceOptions {
  /** Interrupts what the reader is saying: for what needs the user now (a question, a failure). */
  assertive?: boolean;
}

/** What the announcer needs from the page, so tests can run it on a fake clock. */
export interface AnnouncerIo {
  now: () => number;
  /** Runs `run` after `ms`. */
  schedule: (run: () => void, ms: number) => void;
  /** Puts `text` in the polite or the assertive region. */
  write: (text: string, assertive: boolean) => void;
}

/**
 * Rate limiting and de-duplication, without the DOM. A message waits at most {@link COALESCE_MS}
 * after the last one was said, and what waits together is said in one breath; an assertive one
 * never waits. Words said in the last {@link REPEAT_MS}, or already waiting, are dropped.
 */
export function createAnnouncer(io: AnnouncerIo) {
  const said = new Map<string, number>();
  const pending: string[] = [];
  let lastPolite = Number.NEGATIVE_INFINITY;
  let scheduled = false;

  const repeat = (text: string, now: number) => {
    const at = said.get(text);
    return at !== undefined && now - at < REPEAT_MS;
  };

  const remember = (text: string, now: number) => {
    said.set(text, now);
    // Bounded by what was said recently, not by everything the app ever said.
    for (const [key, at] of said) if (now - at >= REPEAT_MS) said.delete(key);
  };

  const flush = () => {
    scheduled = false;
    if (!pending.length) return;
    const now = io.now();
    const text = pending.splice(0).join(". ");
    lastPolite = now;
    io.write(text, false);
  };

  return {
    announce(text: string, options: AnnounceOptions = {}): void {
      const words = text.trim();
      if (!words) return;
      const now = io.now();
      if (repeat(words, now) || pending.includes(words)) return;
      remember(words, now);
      if (options.assertive) {
        io.write(words, true);
        return;
      }
      pending.push(words);
      if (scheduled) return;
      scheduled = true;
      io.schedule(flush, Math.max(0, lastPolite + COALESCE_MS - now));
    },
  };
}

let regions: { polite: HTMLElement; assertive: HTMLElement } | null = null;

function region(live: "polite" | "assertive"): HTMLElement {
  const el = document.createElement("div");
  el.className = "sr-only";
  el.setAttribute("aria-live", live);
  el.setAttribute("aria-atomic", "true");
  el.setAttribute("role", live === "assertive" ? "alert" : "status");
  el.dataset.announcer = live;
  document.body.appendChild(el);
  return el;
}

/**
 * Mounts the two regions, once. They exist before anything is said into them: a region added with
 * its text already in it is not announced by every reader.
 */
export function mountAnnouncer(): void {
  if (regions || typeof document === "undefined" || !document.body) return;
  regions = { polite: region("polite"), assertive: region("assertive") };
}

const announcer = createAnnouncer({
  now: () => Date.now(),
  schedule: (run, ms) => void setTimeout(run, ms),
  write(text, assertive) {
    mountAnnouncer();
    const el = assertive ? regions?.assertive : regions?.polite;
    if (!el) return;
    // Emptied first, then filled a moment later: a reader announces the change, so the same words
    // twice (a second thread finishing) are still said.
    el.textContent = "";
    setTimeout(() => {
      el.textContent = text;
    }, 50);
  },
});

/** Says `text` to a screen reader, politely unless `assertive`, at most once in a while. */
export function announce(text: string, options?: AnnounceOptions): void {
  announcer.announce(text, options);
}
