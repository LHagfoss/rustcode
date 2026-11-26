import { DropdownMenu } from "radix-ui";
import { useState } from "react";
import { useApp } from "../state/store";
import type { ThreadState } from "../state/thread";
import { renameThread } from "../state/threadActions";
import { ChevronDown } from "./icons";
import { MenuContent } from "./primitives";
import { ThreadMenuItems } from "./SidebarThreadRow";

/**
 * The chat header's thread title, the last crumb after the project. It opens the thread's menu
 * (the sidebar row's own, ui/SidebarThreadRow.tsx) with a chevron that shows on hover, and its
 * Rename edits the title in place, once the menu has closed and given focus back.
 */
export function ThreadTitle({ thread, title }: { thread?: ThreadState; title: string }) {
  const session = useApp((s) => (thread ? s.sessions.find((x) => x.id === thread.id) : undefined));
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState("");
  if (!session) return <h1 className="-mx-1 min-w-0 truncate px-2 font-medium text-body text-fg">{title}</h1>;
  const startRename = () => {
    setDraft(session.title ?? title);
    setTimeout(() => setRenaming(true), 0);
  };
  const commit = () => {
    setRenaming(false);
    const next = draft.trim();
    if (next && next !== session.title) void renameThread(session.id, next, { user: true });
  };
  if (renaming) {
    return (
      <input
        autoFocus
        aria-label="Thread title"
        value={draft}
        onFocus={(e) => e.currentTarget.select()}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") setRenaming(false);
        }}
        className="-mx-1 h-7 min-w-0 max-w-[480px] flex-1 rounded-sm bg-fill px-2 font-medium text-body text-fg outline-none"
      />
    );
  }
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label={`Thread actions for ${title}`}
          className="group -mx-1 flex h-7 min-w-0 items-center gap-0.5 rounded-sm px-2 text-left hover:bg-fill-subtle data-[state=open]:bg-fill-subtle"
        >
          <h1 className="min-w-0 truncate font-medium text-body text-fg">{title}</h1>
          <ChevronDown
            size={12}
            aria-hidden
            className="shrink-0 text-fg-muted opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100 group-data-[state=open]:opacity-100"
          />
        </button>
      </DropdownMenu.Trigger>
      <MenuContent align="start" sideOffset={6} className="w-[280px]">
        <ThreadMenuItems
          session={session}
          Menu={DropdownMenu}
          Item={DropdownMenu.Item}
          Separator={DropdownMenu.Separator}
          onRename={startRename}
        />
      </MenuContent>
    </DropdownMenu.Root>
  );
}
