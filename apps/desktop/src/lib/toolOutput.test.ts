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
});
