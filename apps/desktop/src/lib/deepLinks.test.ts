import { expect, test } from "bun:test";
import { parseDeepLink, promptLink, threadLink } from "./deepLinks";

test("round-trip text and qualified thread keys", () => {
  const prompt = "<script> & + % 日本語\nhello";
  expect(parseDeepLink(promptLink(prompt))).toMatchObject({ kind: "new", prompt });
  for (const id of ["abc-123", "gw-machine/abc-123"])
    expect(parseDeepLink(threadLink(id))).toEqual({ kind: "thread", id });
  expect(parseDeepLink("rustcode://thread/gw%2Fabc")).toEqual({ kind: "thread", id: "gw/abc" });
  expect(parseDeepLink("rustcode://connect/abc_DEF-123")).toMatchObject({ kind: "connect" });
});
test("caps and ignored parameters", () => {
  expect(parseDeepLink(`rustcode://new?prompt=${"x".repeat(9000)}`)).toMatchObject({
    prompt: "x".repeat(8000),
  });
  expect(parseDeepLink("rustcode://new?prompt=ok&permission=full&repo=/tmp/foo")).toEqual({
    kind: "new",
    prompt: "ok",
    repo: undefined,
    harness: undefined,
    model: undefined,
  });
  expect(parseDeepLink("rustcode://new?prompt=a%20b&repo=owner/name&harness=codex&model=m")).toMatchObject({
    prompt: "a b",
    repo: "owner/name",
    harness: "codex",
    model: "m",
  });
});
test("malformed input is inert", () => {
  for (const raw of [
    "",
    "https://new?prompt=x",
    "rustcode://new",
    "rustcode://new?prompt=%zz",
    "rustcode://thread/%FF",
    "rustcode://thread/a/b/c",
    "rustcode://connect/",
    "rustcode://user@new?prompt=x",
    "rustcode://unknown?prompt=x",
  ])
    expect(parseDeepLink(raw)).toBeNull();
});
