// Which part of the window a file drag is over. The Tauri webview reports a drag once for the
// whole window, so the sidebar (folders become projects, issue 3608), a question's form (files go
// with its answer) and the composer (files are attached) each ask whether the drag is over the
// sidebar's or the question's drop target before taking it.

/** Marks the element that takes dropped folders as projects. */
export const FOLDER_DROP_ATTR = "data-folder-drop";

/** Whether a drag at this CSS-pixel position is over the sidebar's folder drop target. */
export function overFolderDrop(position: { x: number; y: number } | undefined): boolean {
  if (!position || typeof document === "undefined") return false;
  return !!document.elementFromPoint(position.x, position.y)?.closest(`[${FOLDER_DROP_ATTR}]`);
}

/** Marks a question's form, which takes dropped files for its answer instead of the composer. */
export const QUESTION_DROP_ATTR = "data-question-drop";

/** Whether a drag at this CSS-pixel position is over a question's form. */
export function overQuestionDrop(position: { x: number; y: number } | undefined): boolean {
  if (!position || typeof document === "undefined") return false;
  return !!document.elementFromPoint(position.x, position.y)?.closest(`[${QUESTION_DROP_ATTR}]`);
}
