import { describe, expect, test } from "bun:test";
import {
  buildHarnessRequest,
  EMPTY_HARNESS_FORM,
  type HarnessForm,
  harnessIdFor,
  hasFormErrors,
  validateHarnessForm,
} from "./harnessForm";

const form = (patch: Partial<HarnessForm> = {}): HarnessForm => ({
  name: "Dsh",
  executable: "dsh",
  args: ["--profile", "acp"],
  env: [],
  ...patch,
});

describe("harness form validation", () => {
  test("a name and an executable are required", () => {
    const errors = validateHarnessForm(EMPTY_HARNESS_FORM);
    expect(errors.name).toBeTruthy();
    expect(errors.executable).toBeTruthy();
    expect(hasFormErrors(errors)).toBe(true);
    expect(hasFormErrors(validateHarnessForm(form()))).toBe(false);
  });

  test("a name that another harness has is refused, whatever its case", () => {
    expect(validateHarnessForm(form({ name: " claude CODE " }), ["Claude Code"]).name).toMatch(/already/);
    expect(validateHarnessForm(form({ name: "x".repeat(65) })).name).toMatch(/at most/);
  });

  test("the executable is a name or a path, not a command line", () => {
    expect(validateHarnessForm(form({ executable: "dsh --profile acp" })).executable).toMatch(/own row/);
    expect(validateHarnessForm(form({ executable: '"dsh"' })).executable).toMatch(/shell/);
    expect(validateHarnessForm(form({ executable: "dsh\0" })).executable).toMatch(/NUL/);
    for (const path of [
      "/Applications/My Agent/bin/agent",
      "~/bin/my agent",
      "./my agent",
      "C:\\Tools\\my agent.exe",
    ])
      expect(validateHarnessForm(form({ executable: path })).executable).toBeUndefined();
  });

  test("arguments are literal: spaces, quotes and dollars pass, a blank row is dropped", () => {
    const args = ["--prompt", 'say "hi" $HOME', "", "  padded "];
    expect(hasFormErrors(validateHarnessForm(form({ args })))).toBe(false);
    expect(buildHarnessRequest(form({ args }), []).command).toEqual([
      "dsh",
      "--prompt",
      'say "hi" $HOME',
      "  padded ",
    ]);
    expect(validateHarnessForm(form({ args: ["ok", "a\0b"] })).args).toEqual([undefined, expect.any(String)]);
  });

  test("variables need valid, distinct names; an empty row is ignored", () => {
    const errors = validateHarnessForm(
      form({
        env: [
          { name: "DSH_PROFILE", value: "work" },
          { name: "", value: "" },
          { name: "1BAD", value: "x" },
          { name: "DSH_PROFILE", value: "again" },
          { name: "", value: "orphan" },
        ],
      }),
    );
    expect(errors.env[0]).toBeUndefined();
    expect(errors.env[1]).toBeUndefined();
    expect(errors.env[2]?.name).toMatch(/Letters/);
    expect(errors.env[3]?.name).toMatch(/Already/);
    expect(errors.env[4]?.name).toMatch(/Name the variable/);
  });
});

describe("harness request", () => {
  test("it carries the command, the variables that are named and an id", () => {
    const request = buildHarnessRequest(
      form({
        name: "  My Agent ",
        env: [
          { name: " DSH_PROFILE ", value: " work " },
          { name: "", value: "" },
        ],
      }),
      ["codex"],
    );
    expect(request).toEqual({
      id: "my-agent",
      name: "My Agent",
      command: ["dsh", "--profile", "acp"],
      env: { DSH_PROFILE: " work " },
    });
  });

  test("ids are slugs that no harness has yet", () => {
    expect(harnessIdFor("Dsh", [])).toBe("dsh");
    expect(harnessIdFor("Claude Code", ["claude-code"])).toBe("claude-code-2");
    expect(harnessIdFor("Dsh", ["dsh", "dsh-2"])).toBe("dsh-3");
    expect(harnessIdFor("  !!  ", [])).toBe("custom");
    expect(harnessIdFor("Äpple Åse", [])).toBe("pple-se");
  });
});
