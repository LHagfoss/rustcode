import { expect, test } from "bun:test";
import { settingsReadCache } from "./settingsReadCache";

test("shares in-flight reads, reuses fresh answers, and invalidates", async () => {
  const cache = settingsReadCache();
  let calls = 0;
  const load = async () => {
    calls++;
  };
  const first = cache.read("host", load);
  expect(cache.read("host", load)).toBe(first);
  await first;
  await cache.read("host", load);
  expect(calls).toBe(1);
  cache.invalidate("host");
  await cache.read("host", load);
  expect(calls).toBe(2);
  await cache.read("host", load, true);
  expect(calls).toBe(3);
});

test("failed reads can retry and hosts remain independent", async () => {
  const cache = settingsReadCache();
  await expect(
    cache.read("a", async () => {
      throw new Error("offline");
    }),
  ).rejects.toThrow("offline");
  let calls = 0;
  await cache.read("a", async () => {
    calls++;
  });
  await cache.read("b", async () => {
    calls++;
  });
  expect(calls).toBe(2);
});

test("expired answers revalidate", async () => {
  const cache = settingsReadCache(-1);
  let calls = 0;
  const load = async () => {
    calls++;
  };
  await cache.read("a", load);
  await cache.read("a", load);
  expect(calls).toBe(2);
});
