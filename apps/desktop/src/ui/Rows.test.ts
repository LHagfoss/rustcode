// A tool row's own line: it stays one line while it runs, and what a call
// printed is only ever in the card it opens, labelled as an output.

import { describe, expect, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { parseSessionUpdate } from "../acp/types";
import { applyUpdate, beginTurn, emptyThread, finishTurn, type TranscriptItem } from "../state/thread";
import { ToolDetail, ToolRow } from "./Rows";

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

/** The row the app draws for one `npm test` call, after the updates given and its turn's end. */
const call = (bodies: Record<string, unknown>[], end = true): ToolItem => {
  let state = beginTurn(emptyThread("s", "/p"), "go");
  state = applyUpdate(
    state,
    parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "tool_call",
        toolCallId: "b1",
        title: "npm test",
        kind: "execute",
        status: "in_progress",
        rawInput: { command: "npm test", description: "Run the unit tests" },
      },
    } as never),
  );
  for (const body of bodies) {
    state = applyUpdate(
      state,
      parseSessionUpdate({
        sessionId: "s",
        update: { sessionUpdate: "tool_call_update", toolCallId: "b1", ...body },
      } as never),
    );
  }
  if (end) state = finishTurn(state, "completed");
  const item = state.items.find((i) => i.id === "b1");
  if (item?.kind !== "tool") throw new Error("the call is not a row");
  return item;
};

const drawn = (item: ToolItem) => renderToStaticMarkup(createElement(ToolRow, { item, cwd: "/p" }));
const detail = (item: ToolItem) => renderToStaticMarkup(createElement(ToolDetail, { item }));
const spinners = (markup: string) => markup.split("rc-spin").length - 1;

const backgrounded = {
  status: "completed",
  content: [
    {
      type: "content",
      content: {
        type: "text",
        text: "```console\nCommand running in background with ID: bc82qcgu4. Output is written to /tmp/tasks/bc82qcgu4.output.\n```",
      },
    },
  ],
  _meta: { jetbrains: { air: { asyncTasks: { backgrounded: true } } } },
};

describe("a tool row while it runs", () => {
  test("stays one line: what it printed is not drawn under it", () => {
    const item = call(
      [{ content: [{ type: "content", content: { type: "text", text: "bun test v1.3.14\n✓ 24 pass\n" } }] }],
      false,
    );
    const markup = drawn(item);
    expect(spinners(markup)).toBe(1);
    // The row, then nothing: no output block under the line the spinner is on.
    expect(markup).not.toContain("bun test v1.3.14");
    expect(markup).not.toContain("<pre");
  });

  test("says the call, never its description as if it were output", () => {
    const markup = drawn(call([], false));
    expect(markup).toContain("npm test");
    expect(markup).not.toContain("Run the unit tests");
  });
});

describe("a command the harness backgrounded", () => {
  test("stops spinning when the call returns and says where its work went", () => {
    const item = call([backgrounded]);
    const markup = drawn(item);
    expect(spinners(markup)).toBe(0);
    expect(markup).toContain("Started in background");
  });

  test("prints the task's answer without the fence the harness wrapped it in", () => {
    const markup = detail(call([backgrounded]));
    expect(markup).toContain("Command running in background with ID: bc82qcgu4.");
    expect(markup).not.toContain("```");
  });
});

describe("a call's detail", () => {
  test("names the command, and never passes it off as the output", () => {
    const markup = detail(call([], false));
    expect(markup).toContain(">Command<");
    expect(markup).toContain("npm test");
    expect(markup).not.toContain(">Output<");
    // The description is the row's summary already; the body does not repeat it as JSON.
    expect(markup).not.toContain("{");
  });

  test("a command that failed says its exit code in the danger colour", () => {
    const markup = detail({ ...call([], false), output: "boom", exitCode: 2 });
    expect(markup).toContain(">Output<");
    expect(markup.replaceAll("<!-- -->", "")).toContain(`text-danger">Exit code 2`);
  });

  test("an MCP call lists its arguments as rows, not one JSON blob", () => {
    const item: ToolItem = {
      kind: "tool",
      id: "m1",
      title: "mcp__linear__create_issue",
      status: "completed",
      input: { title: "Bug", labels: ["a"], priority: 2 },
      diffs: [],
    };
    const markup = detail(item);
    expect(markup).toContain(">Arguments<");
    expect(markup).toContain(">title</dt>");
    expect(markup).toContain(">Bug</dd>");
    expect(markup).toContain(">2</dd>");
    expect(markup).not.toContain('"title"');
  });

  test("says there is no output when the call printed nothing", () => {
    const item = call([], false);
    expect(detail({ ...item, input: undefined })).toContain("No output");
  });
});

describe("a tool row whose call is over (issue 3711)", () => {
  const open = call([{ status: "in_progress" }], false);

  test("spins while its turn runs, and settles when no call is in flight", () => {
    expect(spinners(drawn(open))).toBe(1);
    // The thread is not running: nothing is in flight, so the row does not spin for good.
    const idle = renderToStaticMarkup(
      createElement(ToolRow, { item: open, cwd: "/p", threadId: "no-such-thread" }),
    );
    expect(spinners(idle)).toBe(0);
  });

  test("a failed call says why, with the harness's error wrapper taken off", () => {
    const failed = call(
      [
        {
          status: "failed",
          content: [
            {
              type: "content",
              content: {
                type: "text",
                text: "<tool_use_error>Error calling tool (mcp__rustcode__start_session): </tool_use_error>",
              },
            },
          ],
        },
      ],
      false,
    );
    const markup = drawn(failed);
    expect(markup).not.toContain("tool_use_error");
    expect(markup).toContain("The tool gave no reason");
    expect(detail(failed)).not.toContain("tool_use_error");
  });
});
