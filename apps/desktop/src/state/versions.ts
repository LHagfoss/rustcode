/**
 * The version and its prerelease fields: `1.21.0-beta.10` is
 * `[[1,21,0],["beta",10]]`. Build metadata (`1.21.0+build.7`) takes no part in
 * ordering, so it is dropped.
 */
export function versionParts(version: string): [number[], string[]] {
  const text = version.trim().replace(/^v/, "").split("+")[0] ?? "";
  const dash = text.indexOf("-");
  const core = (dash === -1 ? text : text.slice(0, dash)).split(".");
  const pre =
    dash === -1
      ? []
      : text
          .slice(dash + 1)
          .split(".")
          .filter(Boolean);
  return [core.map((n) => Number.parseInt(n, 10) || 0), pre];
}

/** Semver's order for one prerelease field: numbers count as numbers, before text. */
function compareIdentifier(a: string, b: string): number {
  const [na, nb] = [/^\d+$/.test(a), /^\d+$/.test(b)];
  if (na && nb) return Math.sign(Number(a) - Number(b));
  if (na) return -1;
  if (nb) return 1;
  return a === b ? 0 : a < b ? -1 : 1;
}

/** Semver's order: `1.21.0-beta.2` is older than `1.21.0`, and `.10` newer than `.9`. */
export function compareVersions(a: string, b: string): number {
  const [coreA, preA] = versionParts(a);
  const [coreB, preB] = versionParts(b);
  for (let i = 0; i < Math.max(coreA.length, coreB.length); i++) {
    const left = coreA[i] ?? 0;
    const right = coreB[i] ?? 0;
    if (left !== right) return left > right ? 1 : -1;
  }
  if (preA.length === 0 || preB.length === 0) {
    return preA.length === preB.length ? 0 : preA.length ? -1 : 1;
  }
  for (let i = 0; i < Math.min(preA.length, preB.length); i++) {
    const order = compareIdentifier(preA[i]!, preB[i]!);
    if (order !== 0) return order;
  }
  return Math.sign(preA.length - preB.length);
}

/** `0.116.0` is newer than `0.115.2`; a beta is older than the release it leads to. */
export function newerVersion(a: string, b: string): boolean {
  return compareVersions(a, b) > 0;
}
