// What the user has put into each waiting question's form: the options ticked, the words written
// and the files attached. Each question keeps its own, apart from the composer's draft, so
// switching threads, or a later question taking the bar first, loses none of it. A draft is
// dropped once its question is answered or dismissed (state/store.ts `reply`); a send that fails
// leaves it, and the form still has it.

import type { PermissionRequest } from "../acp/types";
import type { Attachment } from "./compose";

export interface QuestionDraft {
  ticked: string[];
  text: string;
  attachments: Attachment[];
}

export const emptyQuestionDraft: QuestionDraft = { ticked: [], text: "", attachments: [] };

const drafts = new Map<string, QuestionDraft>();

const keyOf = (request: Pick<PermissionRequest, "sessionId" | "rpcId">) =>
  `${request.sessionId}\u0000${request.rpcId}`;

export function questionDraft(request: Pick<PermissionRequest, "sessionId" | "rpcId">): QuestionDraft {
  return drafts.get(keyOf(request)) ?? emptyQuestionDraft;
}

export function saveQuestionDraft(
  request: Pick<PermissionRequest, "sessionId" | "rpcId">,
  draft: QuestionDraft,
) {
  const empty = !draft.ticked.length && !draft.text && !draft.attachments.length;
  if (empty) drafts.delete(keyOf(request));
  else drafts.set(keyOf(request), draft);
}

export function dropQuestionDraft(request: Pick<PermissionRequest, "sessionId" | "rpcId">) {
  drafts.delete(keyOf(request));
}
