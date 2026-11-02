// design-lint: allow-file (Shiki's syntax palette (Xcode's colours))
// The two small Shiki themes carrying the Xcode colours the SwiftUI app uses
// (CodeStyle.swift); CSS picks light or dark per token. Shared by the
// highlighter worker (lib/highlight.worker.ts).

import type { ThemeRegistration } from "shiki";

const scopes = (light: string, dark: string, scope: string[]) => ({ light, dark, scope });

const RULES = [
  scopes("#9B2393", "#FC5FA3", [
    "keyword",
    "storage",
    "storage.type",
    "keyword.control",
    "constant.language",
    "variable.language",
  ]),
  scopes("#C41A16", "#FC6A5D", [
    "string",
    "string.quoted",
    "string.template",
    "punctuation.definition.string",
  ]),
  scopes("#5D6C79", "#7F8C98", ["comment", "punctuation.definition.comment"]),
  scopes("#1C00CF", "#D0BF69", ["constant.numeric", "constant.character"]),
  scopes("#0B4F79", "#5DD8FF", [
    "entity.name.type",
    "entity.name.class",
    "support.type",
    "support.class",
    "entity.other.inherited-class",
  ]),
];

function theme(name: string, mode: "light" | "dark"): ThemeRegistration {
  return {
    name,
    type: mode,
    colors: {
      "editor.foreground": mode === "light" ? "#1f1f1f" : "#e6e6e6",
      "editor.background": "#00000000",
    },
    tokenColors: RULES.map((r) => ({ scope: r.scope, settings: { foreground: r[mode] } })),
  };
}

export const THEMES: ThemeRegistration[] = [theme("rc-light", "light"), theme("rc-dark", "dark")];
