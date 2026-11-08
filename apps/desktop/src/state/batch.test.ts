import { describe, expect, test } from "bun:test";
import { createStore } from "zustand/vanilla";
import { createBatcher, frameQueue } from "./batch";

function counterStore() {
  const batcher = createBatcher<{ n: number }>();
  const store = createStore<{ n: number }>()((_set, _get, api) => ({ ...batcher.attach(api), n: 0 }));
  return { batcher, store };
}

describe("store batching", () => {
  test("outside a batch every change notifies at once", () => {
    const { store } = counterStore();
    const seen: number[] = [];
    store.subscribe((s) => seen.push(s.n));
    store.setState({ n: 1 });
    store.setState({ n: 2 });
    expect(seen).toEqual([1, 2]);
  });

  test("inside a batch state updates at once but subscribers hear once, with the state before", () => {
    const { batcher, store } = counterStore();
    const calls: [number, number][] = [];
    store.subscribe((s, prev) => calls.push([s.n, prev.n]));
    batcher.batch(() => {
      for (let i = 1; i <= 100; i++) store.setState({ n: i });
      expect(store.getState().n).toBe(100);
      expect(calls).toEqual([]);
    });
    expect(calls).toEqual([[100, 0]]);
  });

  test("a batch that changed nothing notifies nobody; nested batches notify at the outer end", () => {
    const { batcher, store } = counterStore();
    let calls = 0;
    store.subscribe(() => calls++);
    batcher.batch(() => undefined);
    expect(calls).toBe(0);
    batcher.batch(() => {
      batcher.batch(() => store.setState({ n: 1 }));
      expect(calls).toBe(0);
      store.setState({ n: 2 });
    });
    expect(calls).toBe(1);
  });

  test("an unsubscribed listener is not called at the end of the batch", () => {
    const { batcher, store } = counterStore();
    let calls = 0;
    const off = store.subscribe(() => calls++);
    batcher.batch(() => {
      store.setState({ n: 1 });
      off();
    });
    expect(calls).toBe(0);
  });

  test("subscribers hear about the batch even when it throws", () => {
    const { batcher, store } = counterStore();
    let calls = 0;
    store.subscribe(() => calls++);
    expect(() =>
      batcher.batch(() => {
        store.setState({ n: 1 });
        throw new Error("boom");
      }),
    ).toThrow("boom");
    expect(calls).toBe(1);
  });
});

describe("frame queue", () => {
  test("items pushed together are applied together, in order", async () => {
    const batches: number[][] = [];
    const q = frameQueue<number>((items) => batches.push(items));
    for (let i = 0; i < 5; i++) q.push(i);
    expect(batches).toEqual([]);
    await Promise.resolve();
    expect(batches).toEqual([[0, 1, 2, 3, 4]]);
    q.push(5);
    q.flush();
    expect(batches).toEqual([[0, 1, 2, 3, 4], [5]]);
    await Promise.resolve();
    expect(batches.length).toBe(2);
  });

  test("a bounded queue keeps only its newest items", () => {
    const batches: number[][] = [];
    const q = frameQueue<number>((items) => batches.push(items), 16, 3);
    for (let i = 0; i < 5; i++) q.push(i);
    q.flush();
    expect(batches).toEqual([[2, 3, 4]]);
  });
});
