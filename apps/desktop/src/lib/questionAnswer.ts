// What an answer to an agent's question sends: the options picked, the words written and the files
// attached, shaped into the reply `session/request_permission` takes (`acp/types.ts`), with the
// line and the file names the answered row keeps (`ui/Rows.tsx` QuestionRow).
//
// Files travel in `_meta.rustcode.answer.attachments` (engine `acp/questions.rs`): an image as its
// data, which the engine saves where the thread runs; a file picked on this computer by its path;
// a small one read in the browser as its content. The agent reads them as paths and content at the
// end of the answer's text, so every harness gets them, whether or not it reads images.

import type { Json, JsonObject } from "../acp/jsonrpc";
import {
  isAllow,
  type PermissionOption,
  type PermissionRequest,
  permissionSelected,
  questionOptionsAnswer,
  questionTextAnswer,
} from "../acp/types";
import type { Attachment } from "../state/compose";

/** Most files one answer carries; the engine refuses more. */
export const MAX_QUESTION_FILES = 100;

/** What the user has put into one question's form so far. */
export interface QuestionChoice {
  options: readonly PermissionOption[];
  text: string;
  attachments: readonly Attachment[];
}

export interface ShapedAnswer {
  result: JsonObject;
  /** What the answered row says: the options and the words, without the files. */
  answer: string;
  /** The names of the files it carries, for the answered row. */
  files: string[];
}

/** The options that answer the question, without its Cancel. */
export function answerOptions(request: PermissionRequest): PermissionOption[] {
  return request.options.filter((o) => isAllow(o.kind) || !o.kind);
}

/** Whether the form offers files: the engine takes them, and the answer may be the user's own. */
export const takesFiles = (request: PermissionRequest) => !!request.question?.attachments;

/** One attachment as the engine reads it, or nothing for one it cannot carry. */
export function answerFile(attachment: Attachment): JsonObject | null {
  switch (attachment.kind) {
    case "image":
      return { name: attachment.name, mimeType: attachment.mimeType, data: attachment.data };
    case "file":
      if (attachment.path) return { name: attachment.name, path: attachment.path };
      return attachment.text !== undefined ? { name: attachment.name, text: attachment.text } : null;
    case "text":
      return { name: "Pasted text", text: attachment.text };
  }
}

const fileName = (a: Attachment) => (a.kind === "text" ? "Pasted text" : a.name);

/**
 * The reply for what the form holds, or null when it holds nothing that answers: no option, no
 * words and no file, several options on a single-choice question, or files the question cannot
 * take. A file answers on its own, beside the options picked, or beside the words written.
 */
export function shapeQuestionAnswer(request: PermissionRequest, choice: QuestionChoice): ShapedAnswer | null {
  const text = choice.text.trim();
  const options = [...choice.options];
  if (options.length > 1 && !request.question?.multiple) return null;
  const answer = [options.map((o) => o.name).join(", "), text].filter(Boolean).join(" · ");
  const sent = choice.attachments.flatMap((a) => {
    const file = answerFile(a);
    return file ? [{ file, name: fileName(a) }] : [];
  });
  if (sent.length) {
    if (!takesFiles(request) || sent.length > MAX_QUESTION_FILES) return null;
    const carrier = options[0] ?? answerOptions(request)[0];
    if (!carrier) return null;
    const body: JsonObject = { attachments: sent.map((s) => s.file) as Json[] };
    if (options.length) body.options = options.map((o) => o.name);
    if (text) body.text = text;
    return {
      result: {
        outcome: { outcome: "selected", optionId: carrier.id },
        _meta: { rustcode: { answer: body } },
      },
      answer,
      files: sent.map((s) => s.name),
    };
  }
  if (text) {
    const carrier = answerOptions(request)[0];
    if (!carrier) return null;
    return { result: questionTextAnswer(text, carrier), answer: text, files: [] };
  }
  if (!options.length) return null;
  return {
    result: options.length === 1 ? permissionSelected(options[0]!.id) : questionOptionsAnswer(options),
    answer,
    files: [],
  };
}
