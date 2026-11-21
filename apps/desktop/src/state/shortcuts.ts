// What a matched shortcut does, in one place: App.tsx turns a keydown into a
// command id (lib/keys.ts, the table) and hands it here, so the table stays
// pure and every action keeps its single implementation in state/store.ts.
//
// "find-in-thread" is not here: the transcript owns its find bar and matches
// the same table itself (ui/Transcript.tsx).

import { toggleDictation } from "../lib/dictation";
import { cycleFocusArea } from "../lib/focusAreas";
import { stepZoom, zoomLabel } from "../lib/zoom";
import { chooseFolder } from "./folders";
import {
  cancel,
  openSettings,
  setSwitcher,
  settleThread,
  setView,
  startNewThread,
  toast,
  toggleRightPanel,
  toggleSidebar,
  useApp,
} from "./store";
import { HOME_FOLDER, startNewThreadFromContext } from "./threadOpen";
import { leaveSettledThreads, statusOf } from "./threadStatus";

function zoom(direction: 1 | -1 | 0) {
  void stepZoom(direction).then(
    (level) => toast(`UI scale ${zoomLabel(level)}`, "info"),
    () => toast("Could not change UI scale", "error"),
  );
}

/** Runs a shortcut's command; false when this module does not own it. */
export function runShortcut(command: string): boolean {
  const s = useApp.getState();
  switch (command) {
    case "settle-current-thread":
      // A thread that is settled already closes for a new thread instead of settling again.
      if (s.selectedId && statusOf(s, s.selectedId) === "done") leaveSettledThreads([s.selectedId]);
      else if (s.selectedId) void settleThread(s.selectedId);
      return true;
    case "new-thread":
      startNewThreadFromContext();
      return true;
    case "new-thread-default":
      startNewThread(s.defaultCwd || HOME_FOLDER);
      return true;
    case "thread-folder":
      void chooseFolder(false);
      return true;
    case "default-folder":
      void chooseFolder(true);
      return true;
    case "stop-turn":
      void cancel();
      return true;
    case "command-palette":
      setSwitcher(!s.switcherOpen);
      return true;
    case "settings":
      openSettings(s.settings ? null : undefined);
      return true;
    case "usage":
      openSettings("usage");
      return true;
    case "jobs":
      // Like the inbox: the key that opened Jobs takes you back to the chat.
      setView(s.view.kind === "loops" ? { kind: "chat" } : { kind: "loops" });
      return true;
    case "inbox":
      // The row toggles: the shortcut that opened the inbox takes you back to the chat.
      setView(s.view.kind === "inbox" ? { kind: "chat" } : { kind: "inbox" });
      return true;
    case "toggle-sidebar":
      toggleSidebar();
      return true;
    case "toggle-panel":
      toggleRightPanel();
      return true;
    case "focus-next-area":
    case "focus-previous-area":
      // The palette, a confirmation and a permission request keep the keyboard; a dialog or a
      // menu that has focus is left alone by `cycleFocusArea` itself.
      if (s.switcherOpen || s.confirm || s.grant || s.onboardingOpen) return false;
      return cycleFocusArea(command === "focus-next-area" ? 1 : -1);
    case "zoom-in":
      zoom(1);
      return true;
    case "zoom-out":
      zoom(-1);
      return true;
    case "zoom-actual":
      zoom(0);
      return true;
    case "toggle-model-picker":
      if (s.view.kind !== "chat" || s.settings || s.switcherOpen || s.confirm || s.grant || s.onboardingOpen)
        return false;
      useApp.setState({ modelPickerAsk: { toggle: true } });
      return true;
    case "dictate":
      // The composer in view records; with none (Settings open, no chat) the key does nothing.
      return toggleDictation();
    default:
      return false;
  }
}
