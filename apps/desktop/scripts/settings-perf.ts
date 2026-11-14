// Bounded, account-free probe-cost benchmark. This measures cache overhead and
// avoided requests, not browser paint or real harness/Keychain latency.
import { settingsReadCache } from "../src/state/settingsReadCache";

const delay = () => Bun.sleep(25);
let calls = 0;
const load = async () => {
  calls++;
  await delay();
};
const time = async (run: () => Promise<void>) => {
  const start = performance.now();
  await run();
  return +(performance.now() - start).toFixed(2);
};
const before = await time(async () => {
  for (let i = 0; i < 10; i++) await load();
});
const beforeCalls = calls;
calls = 0;
const cache = settingsReadCache();
const cold = await time(() => cache.read("local", load));
const warm = await time(async () => {
  for (let i = 0; i < 10; i++) await cache.read("local", load);
});
console.log(
  JSON.stringify({
    simulatedProbeMs: 25,
    beforeTenVisitsMs: before,
    beforeCalls,
    afterColdMs: cold,
    afterTenWarmVisitsMs: warm,
    afterCalls: calls,
  }),
);
