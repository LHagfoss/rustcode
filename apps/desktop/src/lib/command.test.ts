import { expect, test } from "bun:test";
import { displayCommand, stripLeadingCd } from "./command";

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
  expect(displayCommand("cd /home/lucas/code && ls /home/lucas")).toBe("cd ~/code && ls ~");
  expect(displayCommand("echo /tmp/Users/x")).toBe("echo /tmp/Users/x");
});

test("strips redundant leading cd matching cwd", () => {
  expect(stripLeadingCd("cd ~/code/rustcode && git status", "/home/lucas/code/rustcode")).toBe("git status");
  expect(stripLeadingCd("cd /home/lucas/code/rustcode && git status", "/home/lucas/code/rustcode")).toBe(
    "git status",
  );
  expect(stripLeadingCd("cd ~/code/other && git status", "/home/lucas/code/rustcode")).toBe(
    "cd ~/code/other && git status",
  );
});
