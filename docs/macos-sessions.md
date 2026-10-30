# macOS app: chat retention

The macOS app keeps its chat list current by archiving idle chats and, later,
moving long-archived chats to the Trash (#2489).

## Rules

- An unpinned chat with no activity for **7 days** moves to the archive. Age
  counts from the chat's last activity (the engine's `updatedAt`), not its
  creation, so a chat used yesterday stays put however old it is.
- An archived chat moves to the macOS **Trash** once **30 days** have passed
  since the later of its archiving and its last activity. Nothing is deleted
  outright: the session folder can be put back from the Trash until you empty
  it.
- **Pinned** chats are never archived or trashed. Neither is a chat that is
  running a turn, open in the app, selected, or just started.
- The sweep runs at most once a day, after the chat list loads, over every
  project's chats. When it moves anything, a toast says how many.

## Finding and restoring chats

- Archived chats: the archive button at the top of the sidebar lists them.
  Use **Unarchive** (swipe or context menu) to bring one back; this also
  clears its archive date.
- Trashed chats: open the Trash in Finder and use **Put Back** on the session
  folder (under `~/.config/rustcode/threads/<yyyy>/<mm>/<dd>/`). The chat
  reappears in the list on the next refresh.

## Upgrade

Chats archived before this release had no archive date. The first sweep dates
them to that day, so none of them reaches the Trash sooner than 30 days after
upgrading. Dating is idempotent and a no-op on later sweeps. Times are stored
as absolute instants, so time zone changes and restarts do not shift them.
