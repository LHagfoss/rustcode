import { describe, expect, test } from "bun:test";
import { commandIn, setupChip } from "./systemSetup";

describe("system setup", () => {
  test("a message's command is what follows Run: or with:", () => {
    expect(
      commandIn(
        `~/.local/bin is not on your shell's PATH. Run: echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zprofile`,
      ),
    ).toBe(`echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zprofile`);
    expect(commandIn("Git for Windows was not found. Install it with: winget install --id Git.Git -e")).toBe(
      "winget install --id Git.Git -e",
    );
    expect(
      commandIn("Long paths are off. In PowerShell opened as administrator, run: New-ItemProperty -Name X"),
    ).toBe("New-ItemProperty -Name X");
  });

  test("a message with no command has none to copy", () => {
    expect(commandIn("No quarantine flag on RustCode.")).toBeNull();
    expect(commandIn("")).toBeNull();
  });

  test("every status has a chip, and only the user's steps and failures are loud", () => {
    expect(setupChip("done")).toEqual({ tone: "ok", label: "Done" });
    expect(setupChip("needs-you").tone).toBe("warn");
    expect(setupChip("failed").tone).toBe("bad");
    expect(setupChip("pending").tone).toBe("idle");
    expect(setupChip("skipped").tone).toBe("idle");
  });
});
