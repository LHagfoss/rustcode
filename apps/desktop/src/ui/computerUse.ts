export function domainEntry(input: string): string | null {
  const entry = input.trim().toLowerCase();
  const host = entry.startsWith("*.") ? entry.slice(2) : entry;
  return host.length > 0 &&
    host.length <= 253 &&
    host
      .split(".")
      .every(
        (label) => label.length > 0 && label.length <= 63 && /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label),
      )
    ? entry
    : null;
}

export function addDomain(entries: string[], input: string): string[] | null {
  const entry = domainEntry(input);
  return entry && !entries.includes(entry) ? [...entries, entry] : null;
}
