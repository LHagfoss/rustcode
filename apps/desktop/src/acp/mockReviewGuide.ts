/** The review guide the mock's text generator writes for the pull request diff it serves. */
export const MOCK_REVIEW_GUIDE = JSON.stringify({
  stops: [
    {
      title: "Keep the composer focused after sending",
      summary:
        "Sending used to blur the field, so the next message needed a click. It now clears the text, keeps focus and restores the focus ring.",
      lookClosely:
        "Keeping focus changes what Tab does right after sending; check that this is the behavior you want.",
      excerpts: [{ path: "apps/desktop/src/ui/Composer.tsx", side: "new", start: 39, end: 44 }],
      files: ["apps/desktop/src/ui/Composer.tsx"],
      questions: ["Does restoreRing run when the send fails?"],
    },
    {
      title: "Compose the composer's class",
      summary: "The composer renders a styled element instead of nothing, using the new `compose` helper.",
      lookClosely: null,
      excerpts: [{ path: "apps/desktop/src/ui/Composer.tsx", side: "new", start: 1, end: 4 }],
      files: ["apps/desktop/src/ui/Composer.tsx"],
      questions: [],
    },
    {
      title: "Rename the drafts module",
      summary: "`drafts.ts` becomes `draft.ts`, its key is renamed and a draft limit is added.",
      lookClosely: null,
      excerpts: [{ path: "apps/desktop/src/state/draft.ts", side: "new", start: 1, end: 2 }],
      files: ["apps/desktop/src/state/draft.ts"],
      questions: [],
    },
  ],
});
