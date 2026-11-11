import { expect, test } from "bun:test";
import { basename, dirname, isAbsolute, isInside, join, normalize } from "./paths";

test("paths handle POSIX, drives and UNC shares", () => {
  for (const root of ["/repo", "C:\\repo", "\\\\host\\share"]) {
    const file = join(root, "src/a.ts");
    expect(isAbsolute(file)).toBe(true);
    expect(basename(file)).toBe("a.ts");
    expect(dirname(file)).toBe(join(root, "src"));
    expect(isInside(file, root)).toBe(true);
    expect(isInside(join(root, "../other"), root)).toBe(root.startsWith("\\\\"));
  }
  expect(normalize("C:\\a/../../b")).toBe("C:\\b");
  expect(normalize("\\\\host\\share\\..\\b")).toBe("\\\\host\\share\\b");
  expect(normalize("../../a/../b")).toBe("../../b");
  expect(isAbsolute("C:a")).toBe(false);
  expect(isInside("c:/REPO/a", "C:\\repo")).toBe(true);
  expect(isInside("/Repo/a", "/repo")).toBe(false);
  expect(isInside("C:\\repo-other\\a", "C:\\repo")).toBe(false);
  expect(isInside("D:\\repo\\a", "C:\\repo")).toBe(false);
  expect(isInside("/a", "/")).toBe(true);
  expect(dirname("\\\\host\\share\\")).toBe("\\\\host\\share\\");
  expect(dirname("/a")).toBe("/");
  expect(dirname("C:\\a")).toBe("C:\\");
});
