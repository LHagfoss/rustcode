// One selection across a reply's text and its Smart UI views.
//
// A view is a sandboxed frame, a separate document with a selection of its own, so a drag from
// the reply's words over a view used to stop at the frame: nothing in it lit up and a copy left
// it out. This bridge carries the app's selection across the frame boundary:
//
// - Into a view: while the app's selection covers a frame, the frame is told which part (all of
//   it, or from its top or bottom to the pointer while the drag is over it) and selects that
//   itself, so it draws its own highlight; it answers with the text it selected.
// - Out of a view: a drag that starts in a view and leaves it extends the app's selection from the
//   frame's edge to the pointer, while the frame's own selection runs to its edge.
// - Copy, from either document, writes the app's text with each covered view's text in place.

/** What part of a frame the app's selection covers. */
export type FrameSelection =
  | { mode: "none" }
  | { mode: "all" }
  /** From the frame's start to the point (frame coordinates): the drag came from above. */
  | { mode: "head"; x: number; y: number }
  /** From the point to the frame's end: the drag came from below. */
  | { mode: "tail"; x: number; y: number }
  /** A drag from inside the frame left it: its own selection runs on to its start or end. */
  | { mode: "extend"; to: "start" | "end" };

type Entry = {
  frame: HTMLIFrameElement;
  post: (selection: FrameSelection) => void;
  /** The text the frame has selected now, as it last reported it. */
  text: string;
  /** The last selection sent, so a drag sends only changes. */
  sent: string;
  /** The selection started inside the frame: the frame owns its part, the app only extends. */
  local: boolean;
};

const entries = new Set<Entry>();
let dragging = false;
let pointer: { x: number; y: number } | null = null;
let pending = 0;
let installed = false;
type Point = { node: Node; offset: number };
/**
 * A drag from the app crossed a frame, which took the pointer from the app: the browser's own
 * selection drag ended there, so the app carries the selection from its anchor to the pointer.
 */
let carried: { anchor: Point; focus: Point } | null = null;
/** A drag started in this frame; once it leaves the frame the app gets the pointer. */
let outward: Entry | null = null;

/** Point the carried selection at the pointer, when the pointer is over the reply's text. */
function carryTo(x: number, y: number) {
  if (!carried) return;
  const caret = caretAt(x, y);
  const at = caret && (caret.node instanceof Element ? caret.node : caret.node.parentElement);
  if (!caret || !at?.closest(".selectable") || at.closest("iframe")) return;
  carried.focus = caret;
  document
    .getSelection()
    ?.setBaseAndExtent(carried.anchor.node, carried.anchor.offset, caret.node, caret.offset);
}

/** A drag that started in the frame is at (x, y) in frame coordinates, button still held. */
function dragOutOf(entry: Entry, x: number, y: number) {
  const { frame } = entry;
  if (!entry.local || !Number.isFinite(x) || !Number.isFinite(y)) return;
  const rect = frame.getBoundingClientRect();
  const selection = document.getSelection();
  const edge = slot(frame);
  if (!selection || !edge) return;
  const below = y >= rect.height;
  const above = y < 0;
  if (!below && !above) {
    // Back inside the frame: the selection is the frame's alone again.
    entry.sent = "";
    if (selection.rangeCount > 0 && selection.getRangeAt(0).intersectsNode(frame))
      selection.removeAllRanges();
    return;
  }
  send(entry, { mode: "extend", to: below ? "end" : "start" });
  const caret = caretAt(rect.left + x, rect.top + y);
  const at = caret && (caret.node instanceof Element ? caret.node : caret.node.parentElement);
  if (!caret || !at?.closest(".selectable") || at.closest("iframe")) return;
  selection.setBaseAndExtent(edge.parent, below ? edge.index : edge.index + 1, caret.node, caret.offset);
}

/** A frame's index among its parent's children: the boundary points just before and after it. */
function slot(frame: Element): { parent: Node; index: number } | null {
  const parent = frame.parentNode;
  if (!parent) return null;
  return { parent, index: Array.prototype.indexOf.call(parent.childNodes, frame) };
}

/** The anchor of the selection lies before the frame (the drag runs downward through it). */
function anchorBefore(selection: Selection, frame: Element): boolean {
  if (!selection.anchorNode) return true;
  const anchor = document.createRange();
  anchor.setStart(selection.anchorNode, selection.anchorOffset);
  const before = document.createRange();
  before.setStartBefore(frame);
  return anchor.compareBoundaryPoints(Range.START_TO_START, before) <= 0;
}

function send(entry: Entry, selection: FrameSelection) {
  const key = JSON.stringify(selection);
  if (key === entry.sent) return;
  entry.sent = key;
  if (selection.mode === "none") entry.text = "";
  entry.post(selection);
}

/** Tell each frame what part of it the app's selection covers now. */
function sync() {
  pending = 0;
  const selection = document.getSelection();
  const range =
    selection && selection.rangeCount > 0 && !selection.isCollapsed ? selection.getRangeAt(0) : null;
  for (const entry of entries) {
    if (entry.local) continue;
    const { frame } = entry;
    if (!selection || !range || !frame.isConnected || !range.intersectsNode(frame)) {
      if (entry.sent && entry.sent !== '{"mode":"none"}') send(entry, { mode: "none" });
      continue;
    }
    // The drag ended over the frame: the part it reached stays selected.
    if (!dragging && /"mode":"(head|tail)"/.test(entry.sent)) continue;
    const rect = frame.getBoundingClientRect();
    const over =
      dragging &&
      pointer &&
      pointer.x >= rect.left &&
      pointer.x <= rect.right &&
      pointer.y >= rect.top &&
      pointer.y <= rect.bottom;
    if (over && pointer) {
      const point = { x: Math.round(pointer.x - rect.left), y: Math.round(pointer.y - rect.top) };
      send(entry, { mode: anchorBefore(selection, frame) ? "head" : "tail", ...point });
    } else send(entry, { mode: "all" });
  }
}

function schedule() {
  if (!pending) pending = requestAnimationFrame(sync);
}

function install() {
  if (installed) return;
  installed = true;
  document.addEventListener("selectionchange", schedule);
  document.addEventListener(
    "mousedown",
    (event) => {
      if (event.button !== 0) return;
      dragging = true;
      carried = null;
      outward = null;
      pointer = { x: event.clientX, y: event.clientY };
      // A press in the app starts a selection of its own: no frame owns one any more.
      for (const entry of entries) entry.local = false;
    },
    true,
  );
  document.addEventListener(
    "mousemove",
    (event) => {
      if (outward) {
        // The drag from a frame left it, and the pointer is the app's now.
        const rect = outward.frame.getBoundingClientRect();
        if (event.buttons & 1) dragOutOf(outward, event.clientX - rect.left, event.clientY - rect.top);
        else outward = null;
        return;
      }
      if (!dragging) return;
      if (!(event.buttons & 1)) {
        dragging = false;
        carried = null;
        return;
      }
      pointer = { x: event.clientX, y: event.clientY };
      carryTo(event.clientX, event.clientY);
      schedule();
    },
    true,
  );
  window.addEventListener(
    "mouseup",
    () => {
      dragging = false;
      outward = null;
      const done = carried;
      carried = null;
      // The browser takes the release for a click on the place it lands and collapses the
      // selection there: put the carried selection back once it has.
      if (done) {
        setTimeout(() =>
          document
            .getSelection()
            ?.setBaseAndExtent(done.anchor.node, done.anchor.offset, done.focus.node, done.focus.offset),
        );
      }
      schedule();
    },
    true,
  );
  document.addEventListener("copy", (event) => {
    const text = selectedText();
    if (text === null || !event.clipboardData) return;
    event.clipboardData.setData("text/plain", text);
    event.preventDefault();
  });
}

/**
 * The app's selection as text, with each view it covers in place; null when it covers no view
 * with a selection of its own (the browser's copy is then the right one).
 */
export function selectedText(): string | null {
  const selection = document.getSelection();
  if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return null;
  const range = selection.getRangeAt(0);
  const covered = [...entries]
    .filter((entry) => entry.text && entry.frame.isConnected && range.intersectsNode(entry.frame))
    .sort((a, b) => (a.frame.compareDocumentPosition(b.frame) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1));
  if (covered.length === 0) return null;
  const parts: string[] = [];
  const rest = range.cloneRange();
  for (const entry of covered) {
    const before = rest.cloneRange();
    before.setEndBefore(entry.frame);
    parts.push(before.toString());
    parts.push(entry.text);
    rest.setStartAfter(entry.frame);
  }
  parts.push(rest.toString());
  return parts
    .map((part) => part.replace(/^\n+|\s+$/g, ""))
    .filter(Boolean)
    .join("\n\n");
}

/** A Smart UI frame joins the app's selection; the returned handle takes its reports. */
export function joinSelection(frame: HTMLIFrameElement, post: (selection: FrameSelection) => void) {
  install();
  const entry: Entry = { frame, post, text: "", sent: "", local: false };
  entries.add(entry);
  schedule();
  return {
    /** The frame's selected text changed. */
    text(value: unknown) {
      entry.text = typeof value === "string" ? value : "";
    },
    /** A press inside the frame: its selection is its own, and an app selection over it ends. */
    press() {
      entry.local = true;
      entry.sent = "";
      outward = entry;
      dragging = false;
      const selection = document.getSelection();
      if (selection && selection.rangeCount > 0 && selection.getRangeAt(0).intersectsNode(frame)) {
        selection.removeAllRanges();
      }
    },
    /** A drag that started in the frame is at (x, y) in frame coordinates, button still held. */
    dragOut(x: number, y: number) {
      dragOutOf(entry, x, y);
    },
    /** A drag that started in the app is over the frame (the frame got the pointer, not the app). */
    dragOver(x: number, y: number) {
      if (entry.local || !Number.isFinite(x) || !Number.isFinite(y)) return;
      const rect = frame.getBoundingClientRect();
      dragging = true;
      pointer = { x: rect.left + x, y: rect.top + y };
      const selection = document.getSelection();
      const edge = slot(frame);
      if (!selection?.anchorNode || !edge) return;
      carried ??= {
        anchor: { node: selection.anchorNode, offset: selection.anchorOffset },
        focus: { node: selection.anchorNode, offset: selection.anchorOffset },
      };
      // The selection covers the frame while the frame selects its own part up to the pointer.
      const down = anchorBefore(selection, frame);
      carried.focus = { node: edge.parent, offset: down ? edge.index + 1 : edge.index };
      selection.setBaseAndExtent(
        carried.anchor.node,
        carried.anchor.offset,
        carried.focus.node,
        carried.focus.offset,
      );
      schedule();
    },
    /** The frame copied: when the app's selection runs on past it, the copy takes all of it. */
    copied(): string | null {
      return selectedText();
    },
    leave() {
      entries.delete(entry);
    },
  };
}

function caretAt(x: number, y: number): { node: Node; offset: number } | null {
  const doc = document as Document & {
    caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null;
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
  };
  const position = doc.caretPositionFromPoint?.(x, y);
  if (position) return { node: position.offsetNode, offset: position.offset };
  const range = doc.caretRangeFromPoint?.(x, y);
  return range ? { node: range.startContainer, offset: range.startOffset } : null;
}
