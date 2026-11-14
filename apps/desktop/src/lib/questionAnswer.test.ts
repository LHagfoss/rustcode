// How an answer to an agent's question is shaped: a file alone, with an option or with words, and
// the answers that are no answer.

import { describe, expect, test } from "bun:test";
import type { PermissionOption, PermissionRequest } from "../acp/types";
import type { Attachment } from "../state/compose";
import { answerFile, MAX_QUESTION_FILES, shapeQuestionAnswer } from "./questionAnswer";

const small: PermissionOption = { id: "question_option_0", name: "Small", kind: "allow_once" };
const large: PermissionOption = { id: "question_option_1", name: "Large", kind: "allow_once" };
const cancel: PermissionOption = { id: "question_cancel", name: "Cancel", kind: "reject_once" };

function request(question: Partial<NonNullable<PermissionRequest["question"]>> = {}): PermissionRequest {
  return {
    rpcId: 1,
    sessionId: "s",
    toolCallId: "question:q",
    title: "Which?",
    options: [small, large, cancel],
    question: {
      header: "",
      text: "Which?",
      descriptions: [],
      multiple: false,
      attachments: true,
      ...question,
    },
    diffs: [],
  };
}

const image: Attachment = { id: "i", kind: "image", name: "shot.png", mimeType: "image/png", data: "aGk=" };
const picked: Attachment = { id: "f", kind: "file", name: "log.txt", path: "/repo/log.txt" };
const read: Attachment = { id: "r", kind: "file", name: "notes.md", text: "# Notes" };

describe("shapeQuestionAnswer", () => {
  test("a file alone answers, on the first option, with the files in the meta", () => {
    const shaped = shapeQuestionAnswer(request(), { options: [], text: " ", attachments: [image, picked] });
    expect(shaped?.result).toEqual({
      outcome: { outcome: "selected", optionId: "question_option_0" },
      _meta: {
        rustcode: {
          answer: {
            attachments: [
              { name: "shot.png", mimeType: "image/png", data: "aGk=" },
              { name: "log.txt", path: "/repo/log.txt" },
            ],
          },
        },
      },
    });
    expect(shaped?.answer).toBe("");
    expect(shaped?.files).toEqual(["shot.png", "log.txt"]);
  });

  test("a file goes with the option picked or the words written", () => {
    const withOption = shapeQuestionAnswer(request(), { options: [large], text: "", attachments: [read] });
    expect(withOption?.result).toEqual({
      outcome: { outcome: "selected", optionId: "question_option_1" },
      _meta: {
        rustcode: { answer: { attachments: [{ name: "notes.md", text: "# Notes" }], options: ["Large"] } },
      },
    });
    expect(withOption?.answer).toBe("Large");
    const withText = shapeQuestionAnswer(request(), {
      options: [],
      text: " Like this ",
      attachments: [read],
    });
    expect(withText?.result._meta).toEqual({
      rustcode: { answer: { attachments: [{ name: "notes.md", text: "# Notes" }], text: "Like this" } },
    });
    expect(withText?.answer).toBe("Like this");
  });

  test("without files the answer is the one the engine always took", () => {
    expect(shapeQuestionAnswer(request(), { options: [small], text: "", attachments: [] })?.result).toEqual({
      outcome: { outcome: "selected", optionId: "question_option_0" },
    });
    const several = shapeQuestionAnswer(request({ multiple: true }), {
      options: [small, large],
      text: "",
      attachments: [],
    });
    expect(several?.result._meta).toEqual({ rustcode: { answer: { options: ["Small", "Large"] } } });
    expect(several?.answer).toBe("Small, Large");
    expect(shapeQuestionAnswer(request(), { options: [], text: "Neither", attachments: [] })?.result).toEqual(
      {
        outcome: { outcome: "selected", optionId: "question_option_0" },
        _meta: { rustcode: { answer: { text: "Neither" } } },
      },
    );
  });

  test("nothing, several on a single choice, or files a question cannot take is no answer", () => {
    expect(shapeQuestionAnswer(request(), { options: [], text: "  ", attachments: [] })).toBeNull();
    expect(shapeQuestionAnswer(request(), { options: [small, large], text: "", attachments: [] })).toBeNull();
    expect(
      shapeQuestionAnswer(request({ attachments: false }), { options: [], text: "", attachments: [image] }),
    ).toBeNull();
    const many = Array.from({ length: MAX_QUESTION_FILES + 1 }, (_, i) => ({ ...picked, id: `f${i}` }));
    expect(shapeQuestionAnswer(request(), { options: [], text: "", attachments: many })).toBeNull();
  });

  test("a file with neither a path nor content is not sent", () => {
    expect(answerFile({ id: "x", kind: "file", name: "big.bin" })).toBeNull();
    expect(
      shapeQuestionAnswer(request(), {
        options: [],
        text: "",
        attachments: [{ id: "x", kind: "file", name: "big.bin" }],
      }),
    ).toBeNull();
  });
});
