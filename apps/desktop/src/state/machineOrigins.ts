/** Placeholder folder for a thread started in the machine's home directory. */
export const HOME_FOLDER = "~";

/**
 * Per-session client memory of folders visited on each machine (`sessionId -> host -> cwd`).
 * Mirrors the engine's persisted `machine_origins` so the Machine picker can preview
 * `"Returns to <folder>"` immediately before the user clicks.
 */
const originFolders = new Map<string, Map<string, string>>();

export function rememberMachineOrigin(sessionId: string, host: string, cwd: string | undefined): void {
  if (!sessionId || !cwd || cwd === HOME_FOLDER) return;
  let perHost = originFolders.get(sessionId);
  if (!perHost) {
    perHost = new Map();
    originFolders.set(sessionId, perHost);
  }
  perHost.set(host, cwd);
}

export function rememberedMachineOrigin(sessionId: string, host: string): string | undefined {
  return originFolders.get(sessionId)?.get(host);
}

export function clearMachineOrigins(): void {
  originFolders.clear();
}
