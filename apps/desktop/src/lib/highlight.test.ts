import { describe, expect, test } from "bun:test";
import { highlight, langForPath, normalizeLang, peekHighlight } from "./highlight";
import { HIGHLIGHT_LANGS } from "./highlightLangs";

describe("highlight cache", () => {
  test("a block is highlighted once; later renders read it back synchronously", async () => {
    const code = "const answer = 42;";
    expect(peekHighlight(code, "ts")).toBeUndefined();
    const html = await highlight(code, "ts");
    expect(html).toContain("answer");
    expect(peekHighlight(code, "ts")).toBe(html);
    expect(await highlight(code, "typescript")).toBe(html);
  });

  test("plain text is never highlighted or cached", async () => {
    expect(peekHighlight("x", "text")).toBeNull();
    expect(await highlight("x", undefined)).toBeNull();
  });

  test("curated languages and aliases highlight; unsupported languages return null", async () => {
    expect(await highlight("fn main() {}", "rs")).toContain("main");
    expect(await highlight("echo hi", "shellscript")).toContain("echo");
    expect(await highlight("FROM alpine:3.20", "docker")).toContain("FROM");
    expect(await highlight("SELECT 1;", "sql")).toContain("SELECT");
    expect(await highlight("IDENTIFICATION DIVISION.", "cobol")).toBeNull();
  });

  test("common aliases and file extensions resolve to bundled loaders", () => {
    for (const sample of [
      "a.ts",
      "a.tsx",
      "a.mjs",
      "a.rs",
      "a.py",
      "a.swift",
      "a.go",
      "a.md",
      "a.mdx",
      "a.jsonc",
      "a.toml",
      "a.yml",
      "a.scss",
      "a.svg",
      "a.fish",
      "a.sql",
      "a.hpp",
      "a.cs",
      "a.kt",
      "a.rb",
      "a.php",
      "a.lua",
      "a.vue",
      "a.svelte",
      "a.graphql",
      "a.proto",
      "a.nix",
      "a.zig",
      "a.tf",
      "a.patch",
      "Dockerfile",
      "Makefile",
    ]) {
      const lang = langForPath(sample);
      expect(lang).toBeDefined();
      expect(HIGHLIGHT_LANGS[lang!]).toBeDefined();
    }
    for (const alias of ["sh", "ps1", "c#", "tf", "docker", "make", "protobuf", "tex", "gql"]) {
      expect(HIGHLIGHT_LANGS[normalizeLang(alias)]).toBeDefined();
    }
  });
});

describe("langForPath", () => {
  test("reads the extension or the file's name", () => {
    expect(langForPath("src/a.mjs")).toBe("javascript");
    expect(langForPath("db/init.SQL")).toBe("sql");
    expect(langForPath("deploy/Dockerfile")).toBe("dockerfile");
    expect(langForPath("Dockerfile.dev")).toBe("dockerfile");
    expect(langForPath("v1.2/notes")).toBeUndefined();
    expect(langForPath("a.unknown")).toBeUndefined();
  });
});
