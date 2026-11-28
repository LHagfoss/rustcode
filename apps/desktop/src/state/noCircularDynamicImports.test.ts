import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const SRC = join(import.meta.dir, "..");
const BANNED_DYNAMIC =
  /\bimport\(\s*["'](?:\.\/(?:store|machines|threadMoveMachine)|(?:\.\.|\.)\/lib\/shell)["']\s*\)/;

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sourceFiles(path);
    return /\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : [];
  });
}

describe("static import hygiene", () => {
  test("core state and shell modules are not dynamically imported", () => {
    const found: string[] = [];
    for (const path of sourceFiles(SRC)) {
      readFileSync(path, "utf8")
        .split("\n")
        .forEach((line, index) => {
          if (BANNED_DYNAMIC.test(line)) {
            found.push(`${relative(SRC, path)}:${index + 1} ${line.trim()}`);
          }
        });
    }
    expect(found).toEqual([]);
  });
});
