import { expect, test } from "bun:test";
import { displayCommand } from "./command";

test("drops the login shell wrapper", () => {
  expect(displayCommand("/bin/zsh -lc 'cat /Users/lagos/.codex/skills/SKILL.md'")).toBe(
    "cat ~/.codex/skills/SKILL.md",
  );
  expect(displayCommand(`bash -c "echo \\"hi\\""`)).toBe('echo "hi"');
  expect(displayCommand("/bin/zsh -lc 'echo '\\''quoted'\\'''")).toBe("echo 'quoted'");
});

test("keeps commands it cannot unwrap whole", () => {
  expect(displayCommand("zsh -lc 'a' && rm b")).toBe("zsh -lc 'a' && rm b");
  expect(displayCommand("git status")).toBe("git status");
});

test("shortens home folders anywhere in the line", () => {
  expect(displayCommand("cd /Users/lagos/code && ls /Users/lagos")).toBe("cd ~/code && ls ~");
  expect(displayCommand("echo /tmp/Users/x")).toBe("echo /tmp/Users/x");
});
