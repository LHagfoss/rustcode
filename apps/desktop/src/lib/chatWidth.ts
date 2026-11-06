// How wide the chat lane grows (Settings › Chat). The transcript's rows and the
// composer read one cap (`--rc-chat-content-max-width`, index.css), so the
// setting only moves that cap: the two always stay the same width.

export type ChatWidth = "comfortable" | "wide" | "full";

export const CHAT_WIDTH_DEFAULT: ChatWidth = "comfortable";

export const CHAT_WIDTH_LABEL: Record<ChatWidth, string> = {
  comfortable: "Comfortable",
  wide: "Wide",
  full: "Full",
};

/** The cap in rem, or null for the pane's own width. */
export const CHAT_WIDTH_REM: Record<ChatWidth, number | null> = {
  comfortable: 46,
  wide: 72,
  full: null,
};

export function parseChatWidth(raw: string | null | undefined): ChatWidth {
  return raw === "wide" || raw === "full" ? raw : CHAT_WIDTH_DEFAULT;
}
