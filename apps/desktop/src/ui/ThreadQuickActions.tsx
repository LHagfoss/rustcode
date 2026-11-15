import { CircleCheck, CircleDashed, CircleDot, Copy, Pin, PinOff } from "lucide-react";
import { type ContextMenu, DropdownMenu } from "radix-ui";
import { copyThreadLink } from "../state/deepLinks";
import {
  clearFinished,
  markUnread,
  setThreadStatus,
  settleThread,
  statusOf,
  togglePin,
  useApp,
} from "../state/store";
import { MenuQuickActions } from "./primitives";

/** The same frequent actions in a thread row's dropdown, right-click menu and header. */
export function ThreadQuickActions({
  threadId,
  Item = DropdownMenu.Item,
}: {
  threadId: string;
  Item?: typeof DropdownMenu.Item | typeof ContextMenu.Item;
}) {
  const pinned = useApp((s) => s.pinned.includes(threadId));
  const unread = useApp((s) => threadId in s.finished || threadId in s.failed);
  const done = useApp((s) => statusOf(s, threadId) === "done");
  return (
    <MenuQuickActions
      Item={Item}
      actions={[
        {
          label: pinned ? "Unpin" : "Pin",
          icon: pinned ? <PinOff /> : <Pin />,
          checked: pinned,
          onSelect: () => togglePin(threadId),
        },
        {
          label: unread ? "Read" : "Unread",
          description: unread ? "Mark Read" : "Mark Unread",
          icon: unread ? <CircleCheck /> : <CircleDot />,
          checked: unread,
          onSelect: () => (unread ? clearFinished(threadId) : markUnread(threadId)),
        },
        {
          label: done ? "Unsettle" : "Settle",
          icon: done ? <CircleDashed /> : <CircleCheck />,
          checked: done,
          onSelect: () => (done ? void setThreadStatus(threadId, "idle") : settleThread(threadId)),
        },
        { label: "Copy Link", icon: <Copy />, onSelect: () => copyThreadLink(threadId) },
      ]}
    />
  );
}
