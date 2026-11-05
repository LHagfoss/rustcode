import { describe, expect, test } from "bun:test";
import {
  clear,
  dismiss,
  due,
  enqueue,
  nextExpiry,
  pause,
  remove,
  resume,
  TOAST_ERROR_MS,
  TOAST_INFO_MS,
  TOAST_LIMIT,
  type Toast,
  type ToastInput,
  toastTtl,
} from "./toasts";

let counter = 0;
function add(toasts: Toast[], text: string, at: number, extra: Partial<ToastInput> = {}) {
  return enqueue(toasts, { id: `t${++counter}`, text, tone: "info", ...extra }, { now: at, paused: false });
}

describe("toast queue", () => {
  test("the newest sits on top", () => {
    let list = add([], "first", 0);
    list = add(list, "second", 1);
    expect(list.map((t) => t.text)).toEqual(["second", "first"]);
  });

  test("the same text and tone refreshes one card in place and restarts its time", () => {
    let list = add([], "Copied", 0, { tone: "success" });
    const id = list[0]!.id;
    const firstExpiry = list[0]!.expiresAt!;
    list = enqueue(list, { id: "another", text: "Copied", tone: "success" }, { now: 1000, paused: false });
    expect(list.length).toBe(1);
    expect(list[0]!.id).toBe(id);
    expect(list[0]!.expiresAt).toBe(1000 + TOAST_INFO_MS);
    expect(list[0]!.expiresAt!).toBeGreaterThan(firstExpiry);
  });

  test("a re-add while the old card still fades keeps one card, not a duplicate key", () => {
    let list = add([], "Starting engine…", 0, { id: "engine", persist: true });
    list = dismiss(list, "engine");
    list = enqueue(
      list,
      { id: "engine", text: "The engine stopped.", tone: "error" },
      { now: 100, paused: false },
    );
    expect(list.map((t) => t.id)).toEqual(["engine"]);
    expect(list[0]!.leaving).toBeUndefined();
  });

  test("the same text in another tone is another notice", () => {
    let list = add([], "The engine stopped.", 0);
    list = add(list, "The engine stopped.", 1, { tone: "error" });
    expect(list.length).toBe(2);
  });

  test("an id replaces that card in place, without moving it", () => {
    let list = add([], "one", 0);
    list = add(list, "two", 1);
    list = enqueue(
      list,
      { id: list[1]!.id, text: "one changed", tone: "error", action: { label: "Open", run: () => {} } },
      { now: 2, paused: false },
    );
    expect(list.map((t) => t.text)).toEqual(["two", "one changed"]);
    expect(list[1]!.action?.label).toBe("Open");
  });

  test("a repeat still burns its full time while paused", () => {
    let list = add([], "one", 0);
    list = enqueue(list, { id: list[0]!.id, text: "one", tone: "info" }, { now: 10, paused: true });
    expect(list[0]!.expiresAt).toBeNull();
    expect(list[0]!.remainingMs).toBe(TOAST_INFO_MS);
  });

  test(`at most ${TOAST_LIMIT} show, and a burst of 20 keeps the newest three`, () => {
    let list: Toast[] = [];
    for (let i = 0; i < 20; i++) list = add(list, `Thread ${i} finished`, i);
    expect(list.length).toBe(TOAST_LIMIT);
    expect(list.map((t) => t.text)).toEqual([
      "Thread 19 finished",
      "Thread 18 finished",
      "Thread 17 finished",
    ]);
    expect(new Set(list.map((t) => t.id)).size).toBe(TOAST_LIMIT);
  });

  test("a fading card does not take a live slot", () => {
    let list: Toast[] = [];
    for (let i = 0; i < TOAST_LIMIT; i++) list = add(list, `notice ${i}`, i);
    list = dismiss(list, list[0]!.id);
    list = add(list, "fresh", 10);
    expect(list.filter((t) => !t.leaving).length).toBe(TOAST_LIMIT);
    expect(list.filter((t) => t.leaving).length).toBe(1);
  });

  test("errors stay longer than info", () => {
    expect(toastTtl("error")).toBe(TOAST_ERROR_MS);
    expect(toastTtl("info")).toBe(TOAST_INFO_MS);
    expect(toastTtl("success")).toBe(TOAST_INFO_MS);
    expect(TOAST_ERROR_MS).toBeGreaterThan(TOAST_INFO_MS);
  });

  test("pausing keeps what is left, resuming starts from it, and nothing expires while paused", () => {
    let list = add([], "notice", 0);
    expect(due(list, 2999)).toEqual([]);
    expect(due(list, 3000)).toEqual([list[0]!.id]);
    list = pause(list, 1000);
    expect(list[0]!.expiresAt).toBeNull();
    expect(list[0]!.remainingMs).toBe(2000);
    expect(due(list, 999_999)).toEqual([]);
    // Pausing twice does not eat the remaining time.
    list = pause(list, 5000);
    expect(list[0]!.remainingMs).toBe(2000);
    list = resume(list, 5000);
    expect(list[0]!.expiresAt).toBe(7000);
    expect(due(list, 6999)).toEqual([]);
    expect(due(list, 7000)).toEqual([list[0]!.id]);
  });

  test("a persistent toast never expires and is never paused", () => {
    let list = add([], "Starting engine…", 0, { persist: true, progress: 0.2 });
    expect(list[0]!.expiresAt).toBeNull();
    expect(due(list, 10_000_000)).toEqual([]);
    list = pause(list, 100);
    list = resume(list, 200);
    expect(list[0]!.expiresAt).toBeNull();
  });

  test("dismiss fades once, remove drops it, and only then is it gone", () => {
    let list = add([], "gone soon", 0);
    const id = list[0]!.id;
    list = dismiss(list, id);
    expect(list[0]!.leaving).toBe(true);
    expect(list[0]!.expiresAt).toBeNull();
    const again = dismiss(list, id);
    expect(again).toBe(list);
    list = remove(list, id);
    expect(list).toEqual([]);
    expect(remove(list, id)).toBe(list);
  });

  test("clear keeps the toasts that stay until dismissed", () => {
    let list = add([], "notice", 0);
    list = add(list, "Starting engine…", 1, { id: "engine", persist: true });
    list = clear(list);
    expect(list.map((t) => t.text)).toEqual(["Starting engine…"]);
  });

  test("one expiry feeds one timer, and a paused queue feeds none", () => {
    let list = add([], "first", 0);
    list = enqueue(list, { id: "slow", text: "slow", tone: "error" }, { now: 0, paused: false });
    expect(nextExpiry(list)).toBe(TOAST_INFO_MS);
    list = pause(list, 0);
    expect(nextExpiry(list)).toBeNull();
    list = resume(list, 0);
    expect(nextExpiry(list)).toBe(TOAST_INFO_MS);
  });
});
