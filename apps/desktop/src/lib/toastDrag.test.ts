import { describe, expect, test } from "bun:test";
import {
  dragExit,
  dragOpacity,
  dragState,
  holdSelection,
  type SelectionDoc,
  startsDrag,
  TOAST_DISMISS_PX,
} from "./toastDrag";

describe("toast drag", () => {
  test("a small move is a snap back, not a dismissal", () => {
    expect(dragState(TOAST_DISMISS_PX, 0).dismiss).toBe(false);
    expect(dragState(-TOAST_DISMISS_PX, 0).dismiss).toBe(false);
    expect(dragState(0, -(TOAST_DISMISS_PX / 2) + 1).dismiss).toBe(false);
  });

  test("past 48px sideways or 24px up it dismisses, the way it is going", () => {
    expect(dragState(TOAST_DISMISS_PX + 1, 0).dismiss).toBe(true);
    expect(dragState(TOAST_DISMISS_PX + 1, 0).direction).toBe("right");
    expect(dragState(-TOAST_DISMISS_PX - 1, 0).direction).toBe("left");
    expect(dragState(0, -TOAST_DISMISS_PX / 2).direction).toBe("up");
    // Up wins when the drag is more vertical than sideways.
    expect(dragState(10, -40).direction).toBe("up");
    expect(dragState(60, -20).direction).toBe("right");
  });

  test("sideways follows fully, downward resists at a sixth", () => {
    expect(dragState(40, 0)).toMatchObject({ dx: 40, dy: 0, away: 40 });
    expect(dragState(0, 60).dy).toBe(10);
    expect(dragState(0, -60).dy).toBe(-60);
    // A downward drag resists, so it never travels far enough to fade.
    expect(dragState(0, 60).away).toBe(0);
  });

  test("the card fades as it travels but never below a fifth", () => {
    expect(dragOpacity(0)).toBe(1);
    expect(dragOpacity(100)).toBe(0.5);
    expect(dragOpacity(1000)).toBe(0.2);
  });

  test("a dismissed card flies off the way it was dragged", () => {
    expect(dragExit("left", -60)).toBe("translateX(-120%)");
    expect(dragExit("right", 60)).toBe("translateX(120%)");
    expect(dragExit("up", 4)).toBe("translateY(-140%)");
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
