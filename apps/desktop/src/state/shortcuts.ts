// What a matched shortcut does, in one place: App.tsx turns a keydown into a
// command id (lib/keys.ts, the table) and hands it here, so the table stays
// pure and every action keeps its single implementation in state/store.ts.
//
// "find-in-thread" is not here: the transcript owns its find bar and matches
// the same table itself (ui/Transcript.tsx).

import { stepZoom } from "../lib/zoom";
import { chooseFolder } from "./folders";
import {
  cancel,
  openSettings,
  setSwitcher,
  setTerminalOpen,
  setView,
  startNewThread,
  toast,
  toggleRightPanel,
  toggleSidebar,
  useApp,
} from "./store";

function zoom(direction: 1 | -1 | 0) {
  void stepZoom(direction).then((level) => toast(`Zoom ${Math.round(level * 100)}%`, "info"));
}

/** Runs a shortcut's command; false when this module does not own it. */
export function runShortcut(command: string): boolean {
  const s = useApp.getState();
  switch (command) {
    case "new-thread":
      startNewThread();
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
      openSettings(s.settings ? null : "general");
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
    case "toggle-terminal":
      setTerminalOpen(!s.terminalOpen);
      return true;
    case "zoom-in":
      zoom(1);
      return true;
    case "zoom-out":
      zoom(-1);
      return true;
    case "zoom-actual":
      zoom(0);
      return true;
    default:
      return false;
  }
}
