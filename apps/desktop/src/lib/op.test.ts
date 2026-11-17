import { describe, expect, test } from "bun:test";
import { createOpRunner, type Op, type OpEvent, opError, opRunning, opTransition } from "./op";

/** A runner over a plain variable, the way `useOp` drives one over component state. */
function harness<T>() {
  let op: Op<T> = { status: "idle" };
  const runner = createOpRunner<T>((event: OpEvent<T>) => {
    op = opTransition(op, event);
  });
  return { runner, op: () => op };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

describe("opTransition", () => {
  test("a start keeps the last data and clears a failure", () => {
    expect(opTransition<number>({ status: "failed", error: "boom", data: 1 }, { type: "start" })).toEqual({
      status: "running",
      data: 1,
    });
  });

  test("a success stores its answer", () => {
    expect(opTransition<number>({ status: "running", data: 1 }, { type: "succeed", data: 2 })).toEqual({
      status: "idle",
      data: 2,
    });
  });

  test("a failure keeps the last data", () => {
    expect(opTransition<number>({ status: "running", data: 1 }, { type: "fail", error: "boom" })).toEqual({
      status: "failed",
      error: "boom",
      data: 1,
    });
  });

  test("a reset forgets data and failure", () => {
    expect(opTransition<number>({ status: "failed", error: "boom", data: 1 }, { type: "reset" })).toEqual({
      status: "idle",
    });
  });

  test("opError and opRunning read the state", () => {
    expect(opError({ status: "failed", error: "boom" })).toBe("boom");
    expect(opError({ status: "idle" })).toBeNull();
    expect(opRunning({ status: "running" })).toBe(true);
    expect(opRunning({ status: "idle" })).toBe(false);
  });
});

describe("createOpRunner", () => {
  test("success: running, then idle with the answer", async () => {
    const { runner, op } = harness<string>();
    const answer = deferred<string>();
    const result = runner.run(() => answer.promise);
    expect(op()).toEqual({ status: "running", data: undefined });
    answer.resolve("listing");
    expect(await result).toEqual({ status: "done", value: "listing" });
    expect(op()).toEqual({ status: "idle", data: "listing" });
  });

  test("failure: keeps the error text", async () => {
    const { runner, op } = harness<string>();
    expect(await runner.run(() => Promise.reject(new Error("no such folder")))).toEqual({
      status: "failed",
      error: "no such folder",
    });
    expect(opError(op())).toBe("no such folder");
  });

  test("failure: dropDataOnFailure forgets the last answer", async () => {
    const { runner, op } = harness<number>();
    await runner.run(() => Promise.resolve(1));
    await runner.run(() => Promise.reject(new Error("boom")), { dropDataOnFailure: true });
    expect(op()).toEqual({ status: "failed", error: "boom" });
    const retry = runner.run(() => Promise.resolve(2));
    expect(op()).toEqual({ status: "running", data: undefined });
    await retry;
    expect(op()).toEqual({ status: "idle", data: 2 });
  });

  test("failure: a caller's describe names the text", async () => {
    const { runner, op } = harness<void>();
    await runner.run(() => Promise.reject("raw"), { describe: (e) => `said ${String(e)}` });
    expect(opError(op())).toBe("said raw");
  });

  test("a retry clears the error", async () => {
    const { runner, op } = harness<number>();
    await runner.run(() => Promise.reject(new Error("boom")));
    expect(opError(op())).toBe("boom");
    const retry = runner.run(() => Promise.resolve(3));
    expect(opError(op())).toBeNull();
    expect(op().status).toBe("running");
    await retry;
    expect(op()).toEqual({ status: "idle", data: 3 });
  });

  test("a stale answer after a newer run is dropped", async () => {
    const { runner, op } = harness<string>();
    const slow = deferred<string>();
    const fast = deferred<string>();
    const first = runner.run(() => slow.promise);
    const second = runner.run(() => fast.promise);
    fast.resolve("second");
    expect(await second).toEqual({ status: "done", value: "second" });
    slow.resolve("first");
    expect(await first).toEqual({ status: "stale" });
    expect(op()).toEqual({ status: "idle", data: "second" });
  });

  test("a stale failure after a newer run is dropped", async () => {
    const { runner, op } = harness<string>();
    const slow = deferred<string>();
    const first = runner.run(() => slow.promise);
    const second = runner.run(() => Promise.resolve("second"));
    await second;
    slow.reject(new Error("late"));
    expect(await first).toEqual({ status: "stale" });
    expect(op()).toEqual({ status: "idle", data: "second" });
  });

  test("an answer after detach (unmount) is dropped", async () => {
    const { runner, op } = harness<string>();
    const slow = deferred<string>();
    const result = runner.run(() => slow.promise);
    runner.detach();
    slow.resolve("late");
    expect(await result).toEqual({ status: "stale" });
    expect(op().status).toBe("running");
  });

  test("an answer after reset is dropped", async () => {
    const { runner, op } = harness<string>();
    const slow = deferred<string>();
    const result = runner.run(() => slow.promise);
    runner.reset();
    slow.resolve("late");
    expect(await result).toEqual({ status: "stale" });
    expect(op()).toEqual({ status: "idle" });
  });

  test("runs after a re-attach apply again", async () => {
    const { runner, op } = harness<number>();
    runner.detach();
    runner.attach();
    await runner.run(() => Promise.resolve(5));
    expect(op()).toEqual({ status: "idle", data: 5 });
  });
});
