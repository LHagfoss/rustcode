import { describe, expect, test } from "bun:test";
import {
  dragOpacity,
  dragState,
  holdSelection,
  isClick,
  type SelectionDoc,
  startsDrag,
  TOAST_DISMISS_PX,
  TOAST_FLICK_PX,
} from "./toastDrag";

describe("toast drag", () => {
  test("only a drag right past 48px dismisses", () => {
    expect(dragState(TOAST_DISMISS_PX, 0).dismiss).toBe(false);
    expect(dragState(TOAST_DISMISS_PX + 1, 0).dismiss).toBe(true);
    expect(dragState(-200, 0).dismiss).toBe(false);
    expect(dragState(0, 200).dismiss).toBe(false);
    expect(dragState(0, -200).dismiss).toBe(false);
  });

  test("a quick flick right dismisses short of 48px, a slow or tiny one does not", () => {
    expect(dragState(30, 0, 40).dismiss).toBe(true);
    expect(dragState(30, 0, 400).dismiss).toBe(false);
    expect(dragState(TOAST_FLICK_PX - 1, 0, 5).dismiss).toBe(false);
    expect(dragState(-30, 0, 40).dismiss).toBe(false);
  });

  test("right follows fully, every other way resists at a sixth", () => {
    expect(dragState(40, 0)).toMatchObject({ dx: 40, dy: 0, away: 40 });
    expect(dragState(-60, 0)).toMatchObject({ dx: -10, away: 0 });
    expect(dragState(0, 60).dy).toBe(10);
    expect(dragState(0, -60).dy).toBe(-10);
  });

  test("in the left corner the card swipes left instead", () => {
    expect(dragState(-(TOAST_DISMISS_PX + 1), 0, Number.POSITIVE_INFINITY, -1).dismiss).toBe(true);
    expect(dragState(TOAST_DISMISS_PX + 1, 0, Number.POSITIVE_INFINITY, -1).dismiss).toBe(false);
    expect(dragState(-30, 0, 40, -1).dismiss).toBe(true);
    expect(dragState(-40, 0, Number.POSITIVE_INFINITY, -1)).toMatchObject({ dx: -40, away: 40 });
    expect(dragState(60, 0, Number.POSITIVE_INFINITY, -1)).toMatchObject({ dx: 10, away: 0 });
  });

  test("the card fades as it travels but never below a fifth", () => {
    expect(dragOpacity(0)).toBe(1);
    expect(dragOpacity(100)).toBe(0.5);
    expect(dragOpacity(1000)).toBe(0.2);
  });

  test("a press that barely moves is a click", () => {
    expect(isClick(0, 0)).toBe(true);
    expect(isClick(2, 2)).toBe(true);
    expect(isClick(4, 0)).toBe(false);
  });

  test("a press on a control is a click, never a drag", () => {
    expect(startsDrag(null)).toBe(true);
    expect(startsDrag({ closest: () => null })).toBe(true);
    expect(startsDrag({ closest: () => ({}) })).toBe(false);
  });

  test("holding selection turns it off on html and body and gives both back once", () => {
    const listeners = new Set<(event: { preventDefault(): void }) => void>();
    const doc: SelectionDoc = {
      documentElement: { style: { userSelect: "auto" } },
      body: { style: { userSelect: "text" } },
      addEventListener: (_type, listener) => listeners.add(listener),
      removeEventListener: (_type, listener) => listeners.delete(listener),
    };
    const release = holdSelection(doc);
    expect(doc.documentElement.style.userSelect).toBe("none");
    expect(doc.body.style.userSelect).toBe("none");
    expect(listeners.size).toBe(1);
    // A native selection that starts anyway is cancelled.
    let prevented = false;
    for (const listener of listeners) listener({ preventDefault: () => (prevented = true) });
    expect(prevented).toBe(true);
    // Every way a drag ends calls this; only the first one restores.
    release();
    expect(doc.documentElement.style.userSelect).toBe("auto");
    expect(doc.body.style.userSelect).toBe("text");
    expect(listeners.size).toBe(0);
    release();
    expect(doc.documentElement.style.userSelect).toBe("auto");
  });

  test("releasing after the document was already put back leaves it alone", () => {
    const doc: SelectionDoc = {
      documentElement: { style: { userSelect: "auto" } },
      body: { style: { userSelect: "auto" } },
      addEventListener: () => {},
      removeEventListener: () => {},
    };
    const release = holdSelection(doc);
    release();
    release();
    expect(doc.documentElement.style.userSelect).toBe("auto");
    expect(doc.body.style.userSelect).toBe("auto");
  });
});
