import { describe, expect, test } from "bun:test";
import { parseInlineRef, parseLinkRef, resolveRef } from "./fileRefs";

describe("file references", () => {
  test("inline code that names a file", () => {
    expect(parseInlineRef("src/a.rs:12")).toEqual({ path: "src/a.rs", line: 12, column: undefined });
    expect(parseInlineRef("src/a.rs:12:4")).toEqual({ path: "src/a.rs", line: 12, column: 4 });
    expect(parseInlineRef("Cargo.toml")).toEqual({ path: "Cargo.toml", line: undefined, column: undefined });
    expect(parseInlineRef("src/ui")).toBeNull();
    for (const no of ["cargo test", "1.2.3", "foo()", "https://x.dev/a.rs", "--flag", "a b.rs"])
      expect(parseInlineRef(no)).toBeNull();
  });

  test("relative links, with GitHub-style line anchors", () => {
    expect(parseLinkRef("src/a.rs#L10")).toEqual({ path: "src/a.rs", line: 10, column: undefined });
    expect(parseLinkRef("docs/x%20y.md")?.path).toBe("docs/x y.md");
    for (const no of ["https://a.dev", "mailto:a@b", "#top", "//cdn", ""])
      expect(parseLinkRef(no)).toBeNull();
  });

  test("resolves against the thread's folder", () => {
    expect(resolveRef({ path: "src/a.rs" }, "/p/repo")).toBe("/p/repo/src/a.rs");
    expect(resolveRef({ path: "../b/c.ts" }, "/p/repo/")).toBe("/p/b/c.ts");
    expect(resolveRef({ path: "/abs/x.md" }, "/p")).toBe("/abs/x.md");
  });
});

test("Windows references preserve drives, UNC roots and line numbers", () => {
  for (const path of ["src\\a.rs", "C:\\repo\\a.rs", "\\\\host\\share\\a.rs"]) {
    expect(parseInlineRef(`${path}:12:4`)).toEqual({ path, line: 12, column: 4 });
    expect(parseLinkRef(`${path}#L12`)?.line).toBe(12);
  }
  expect(resolveRef({ path: "src/a.rs" }, "C:\\repo")).toBe("C:\\repo\\src\\a.rs");
  expect(resolveRef({ path: "..\\a.rs" }, "C:\\repo")).toBe("C:\\a.rs");
  expect(resolveRef({ path: "src\\a.rs" }, "\\\\host\\share")).toBe("\\\\host\\share\\src\\a.rs");
  expect(resolveRef({ path: "D:\\a.rs" }, "C:\\repo")).toBe("D:\\a.rs");
});
