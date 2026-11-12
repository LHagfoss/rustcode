import { afterEach, describe, expect, test } from "bun:test";
import type { Transport } from "../acp/transport";
import { setShellTransport, shellCallWithin } from "./shell";
import { WaitTimeoutError } from "./withTimeout";

/** A shell whose native side never answers, as one waiting on a machine that is away. */
class HangingShell implements Transport {
  readonly name = "websocket";
  async start(): Promise<void> {}
  send(): void {}
  async restart(): Promise<void> {}
  async stop(): Promise<void> {}
  call(): Promise<unknown> {
    return new Promise(() => {});
  }
}

afterEach(() => setShellTransport(null));

describe("shellCallWithin", () => {
  test("a native call that never answers ends in an error the page can show", async () => {
    setShellTransport(new HangingShell());
    const error = await shellCallWithin(20, "machines_probe").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(WaitTimeoutError);
  });

  test("an answer in time is returned as it is", async () => {
    setShellTransport({ ...new HangingShell(), call: async () => ["ok"] } as Transport);
    expect(await shellCallWithin(1_000, "machines_list")).toEqual(["ok"]);
  });
});
