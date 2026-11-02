// `bun run dev:web`: the engine bridge plus the Vite dev server, stopped together.

const root = new URL("..", import.meta.url).pathname;
const bridge = Bun.spawn(["bun", "scripts/engine-bridge.ts"], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
});
const vite = Bun.spawn(["bun", "x", "vite"], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
  stdin: "inherit",
});

const stop = () => {
  bridge.kill();
  vite.kill();
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
await Promise.race([bridge.exited, vite.exited]);
stop();
