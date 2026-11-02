import { describe, expect, test } from "bun:test";
import { readReport } from "./report";

describe("report panel", () => {
  test("a bold first line is the title, and Label: value lines are rows under headings", () => {
    const view = readReport(
      "**Status**\n\nModel: `gpt-6-luna`\nEffort: medium\n\n### Session\n- Messages: 14\n- **Tokens**: 18k",
    );
    expect(view.title).toBe("Status");
    expect(view.sections).toEqual([
      {
        rows: [
          { label: "Model", value: "gpt-6-luna" },
          { label: "Effort", value: "medium" },
        ],
      },
      {
        heading: "Session",
        rows: [
          { label: "Messages", value: "14" },
          { label: "Tokens", value: "18k" },
        ],
      },
    ]);
  });

  test("prose stays Markdown", () => {
    const view = readReport("**Fork**\n\nForked this chat into **X**. Open it from the chat list.");
    expect(view.title).toBe("Fork");
    expect(view.sections).toBeUndefined();
    expect(view.body).toBe("Forked this chat into **X**. Open it from the chat list.");
    expect(readReport("Note: one line\nand some prose after it").sections).toBeUndefined();
  });
});
