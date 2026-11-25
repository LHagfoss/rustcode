import { PenLine } from "lucide-react";
import { useEffect, useRef } from "react";
import { cx, Pill, Tooltip } from "./primitives";

/** The composer's field, where a message is written. */
export function focusComposerField() {
  document.querySelector<HTMLTextAreaElement>('[data-rc-composer] textarea[aria-label="Message"]')?.focus();
}

function typesIntoAField(target: EventTarget | null) {
  return target instanceof HTMLElement && !!target.closest("input, textarea, select, [contenteditable]");
}

function writesACharacter(event: KeyboardEvent) {
  return event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey && !event.isComposing;
}

/**
 * While the composer is away behind the overview, a key that writes a character brings it back: the
 * field takes focus during the keydown, so the character lands in it as if it had been there.
 */
export function useTypeToCompose(away: boolean, reveal: () => void) {
  const latest = useRef(reveal);
  latest.current = reveal;
  useEffect(() => {
    if (!away) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || !writesACharacter(event) || typesIntoAField(event.target)) return;
      latest.current();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [away]);
}

/** The way back to the composer from the overview, beside its own controls. */
export function WriteMessagePill({ glass, onPress }: { glass: boolean; onPress: () => void }) {
  return (
    <Tooltip label="Or just start typing">
      <Pill size="sm" className={cx("text-caption", glass && "rc-glass")} onClick={onPress}>
        <PenLine size={12} aria-hidden />
        Write a message
      </Pill>
    </Tooltip>
  );
}
