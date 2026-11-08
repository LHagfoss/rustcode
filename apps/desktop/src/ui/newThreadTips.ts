import { formatBinding } from "../lib/keys";
import type { Platform } from "../lib/platform";

export const NEW_THREAD_TIPS = [
  { text: "Open the command palette", command: "command-palette" },
  { text: "Start a new thread", command: "new-thread" },
  { text: "Choose a thread folder", command: "thread-folder" },
  { text: "Choose the default folder", command: "default-folder" },
  { text: "Open Settings", command: "settings" },
  { text: "Open the needs-you inbox", command: "inbox" },
  { text: "Toggle the sidebar", command: "toggle-sidebar" },
  { text: "Toggle the right panel", command: "toggle-panel" },
  { text: "Find in a thread", command: "find-in-thread" },
  { text: "Toggle the terminal", command: "toggle-terminal" },
  { text: "Steer a running turn", command: "composer-steer" },
  { text: "Insert a new line", command: "composer-newline" },
] as const;

export function tipBinding(binding: string, platform: Platform): string {
  return formatBinding(binding, platform);
}
