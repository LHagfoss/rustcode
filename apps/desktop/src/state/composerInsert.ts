import { appendToDraft } from "../lib/reviewFeedback";
import { setDraft, useApp } from "./store";

/**
 * Puts `addition` into the open thread's composer, after what it already holds, and puts the
 * caret at its end. The user reads, edits and sends it.
 */
export function appendToComposer(addition: string): void {
  if (!addition) return;
  const key = useApp.getState().selectedId ?? "new";
  setDraft(key, appendToDraft(useApp.getState().drafts[key] ?? "", addition));
  requestAnimationFrame(() => {
    const field =
      document.querySelector<HTMLTextAreaElement>('[data-rc-composer] textarea[aria-label="Message"]') ??
      document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]');
    if (!field) return;
    field.focus();
    field.setSelectionRange(field.value.length, field.value.length);
  });
}
