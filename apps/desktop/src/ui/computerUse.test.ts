import { expect, test } from "bun:test";
import { addDomain, domainEntry } from "./computerUse";

test("hosts match engine allowlist syntax", () => {
  for (const host of ["example.com", "*.example.org", "localhost"]) expect(domainEntry(host)).toBe(host);
  expect(domainEntry(" EXAMPLE.COM ")).toBe("example.com");
  for (const host of ["", "*", "https://example.com", "a:443", "a..com", "-a.com", "a/com", "a@b.com"])
    expect(domainEntry(host)).toBeNull();
});
test("editor adds unique hosts and preserves previous state", () => {
  const entries = ["example.com"];
  expect(addDomain(entries, "EXAMPLE.COM")).toBeNull();
  expect(addDomain(entries, "*.example.org")).toEqual(["example.com", "*.example.org"]);
  expect(addDomain(entries, "https://a.com")).toBeNull();
  expect(entries).toEqual(["example.com"]);
  expect(entries.filter((host) => host !== "example.com")).toEqual([]);
});
