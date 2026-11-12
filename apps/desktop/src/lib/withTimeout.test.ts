import { describe, expect, test } from "bun:test";
import { WaitTimeoutError, withTimeout } from "./withTimeout";

describe("withTimeout", () => {
  test("passes the result through when the work answers in time", async () => {
    expect(await withTimeout(Promise.resolve(7), 1_000, "It")).toBe(7);
  });

  test("passes the work's own error through", async () => {
    await expect(withTimeout(Promise.reject(new Error("boom")), 1_000, "It")).rejects.toThrow("boom");
  });

  test("ends a call that never answers", async () => {
    const never = new Promise<never>(() => {});
    const error = await withTimeout(never, 20, "The machine").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(WaitTimeoutError);
    expect((error as Error).message).toBe("The machine did not answer in 0 seconds.");
  });
});
