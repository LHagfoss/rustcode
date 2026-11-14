/** Short freshness window shared across page mounts; stale values stay visible in their stores. */
export function settingsReadCache(ttl = 30_000) {
  const finished = new Map<string, number>();
  const versions = new Map<string, number>();
  const pending = new Map<string, Promise<void>>();
  return {
    invalidate(key: string) {
      finished.delete(key);
      versions.set(key, (versions.get(key) ?? 0) + 1);
    },
    read(key: string, load: () => Promise<void>, force = false): Promise<void> {
      const running = pending.get(key);
      if (running) return running;
      const at = finished.get(key);
      if (!force && at !== undefined && Date.now() - at < ttl) return Promise.resolve();
      const version = versions.get(key);
      const task = Promise.resolve()
        .then(load)
        .then(() => {
          if (versions.get(key) === version) finished.set(key, Date.now());
        })
        .finally(() => pending.delete(key));
      pending.set(key, task);
      return task;
    },
  };
}
