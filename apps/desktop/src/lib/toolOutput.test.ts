// A tool result the harness wrapped in a fence of its own: the output is what
// the fence holds, never the fence.

import { describe, expect, test } from "bun:test";
import { stripOutputFence } from "./toolOutput";

const background =
  "```console\nCommand running in background with ID: bc82qcgu4. Output is being written to: /tmp/tasks/bc82qcgu4.output. You will be notified when it completes.\n```";

describe("a fenced tool result", () => {
  test("gives the output the fence holds, without its backticks or language", () => {
    expect(stripOutputFence(background)).toBe(
      "Command running in background with ID: bc82qcgu4. Output is being written to: /tmp/tasks/bc82qcgu4.output. You will be notified when it completes.",
    );
  });

  test("keeps every line of a longer one", () => {
    expect(stripOutputFence("```bash\nbun test v1.3.14\n✓ 24 pass\n```\n")).toBe(
      "bun test v1.3.14\n✓ 24 pass",
    );
  });

  test("unfolds a tilde fence too", () => {
    expect(stripOutputFence("~~~text\nhi\n~~~")).toBe("hi");
  });

  test("is left alone when no fence wraps the whole output", () => {
    expect(stripOutputFence("bun test v1.3.14\n✓ 24 pass\n")).toBe("bun test v1.3.14\n✓ 24 pass\n");
  });

  test("is left alone when the fence is only part of it", () => {
    // `cat README.md`: the fences are the file's own text, the output is all of it.
    const readme = "Setup\n\n```sh\nbun install\n```\n\nDone.\n";
    expect(stripOutputFence(readme)).toBe(readme);
  });

  test("is left alone when the output goes on past the fence", () => {
    const mixed = "```console\nhi\n```\nand then more\n";
    expect(stripOutputFence(mixed)).toBe(mixed);
  });

  test("ends at the first line that closes it, not the last", () => {
    // Two blocks drawn one after the other: neither wraps the whole text.
    const two = "```\na\n```\n```\nb\n```";
    expect(stripOutputFence(two)).toBe(two);
  });

  test("needs the run that closes it to be the same character", () => {
    const mismatched = "```\nhi\n~~~\n";
    expect(stripOutputFence(mismatched)).toBe(mismatched);
  });

  test("keeps a fence inside a longer one that wraps it", () => {
    expect(stripOutputFence("````markdown\nIntro\n\n```sh\nbun install\n```\n````\n")).toBe(
      "Intro\n\n```sh\nbun install\n```",
    );
  });

  test("takes the closing run however long it is, and whatever follows it", () => {
    expect(stripOutputFence("```\nhi\n`````\n\n  ")).toBe("hi");
  });

  test("reads a Windows line ending", () => {
    expect(stripOutputFence("```console\r\nline one\r\nline two\r\n```\r\n")).toBe("line one\r\nline two");
  });

  test("leaves no indentation of the closing line in the output", () => {
    expect(stripOutputFence("  ```console\nhi\n  ```  ")).toBe("hi");
  });

  test("gives back nothing for a fence around nothing", () => {
    expect(stripOutputFence("```\n```")).toBe("");
  });
});
