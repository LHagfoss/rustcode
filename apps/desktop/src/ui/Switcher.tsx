// ⌘K: switch to a thread by title or project, after SessionSwitcherView.swift.

import { CornerDownLeft, MessageSquare, Search } from "lucide-react";
import { Dialog } from "radix-ui";
import { useMemo, useState } from "react";
import { modelName, projectName } from "../acp/types";
import { selectThread, setSwitcher, useApp } from "../state/store";
import { relativeAge } from "../state/thread";
import { cx, Kbd } from "./primitives";

function highlight(text: string, query: string) {
  const q = query.trim().toLowerCase();
  const at = q ? text.toLowerCase().indexOf(q) : -1;
  if (at < 0) return text;
  return (
    <>
      {text.slice(0, at)}
      <b className="font-semibold">{text.slice(at, at + q.length)}</b>
      {text.slice(at + q.length)}
    </>
  );
}

export function Switcher() {
  const open = useApp((s) => s.switcherOpen);
  const sessions = useApp((s) => s.sessions);
  const selectedId = useApp((s) => s.selectedId);
  const projectCwd = useApp((s) => s.projectCwd);
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState<"all" | "project">("all");
  const [index, setIndex] = useState(0);

  const results = useMemo(() => {
    const q = query.trim().toLowerCase();
    return sessions
      .filter((s) => scope === "all" || s.cwd === projectCwd)
      .filter((s) => !q || `${s.title ?? ""} ${projectName(s.cwd)}`.toLowerCase().includes(q))
      .slice(0, 60);
  }, [sessions, query, scope, projectCwd]);

  const choose = (i: number) => {
    const s = results[i];
    if (s) selectThread(s.id);
  };

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(o) => {
        setSwitcher(o);
        if (!o) {
          setQuery("");
          setIndex(0);
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/12" />
        <Dialog.Content
          aria-describedby={undefined}
          className="surface fixed top-[max(72px,14vh)] left-1/2 z-50 flex w-[min(600px,calc(100vw-56px))] -translate-x-1/2 flex-col overflow-hidden rounded-[24px] shadow-[0_14px_30px_rgb(0_0_0/0.18)] rc-in"
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              setIndex((i) =>
                Math.max(0, Math.min(results.length - 1, i + (e.key === "ArrowDown" ? 1 : -1))),
              );
            } else if (e.key === "Enter") {
              e.preventDefault();
              choose(index);
            } else if (e.key === "Tab") {
              e.preventDefault();
              setScope(scope === "all" ? "project" : "all");
            }
          }}
        >
          <Dialog.Title className="sr-only">Switch to thread</Dialog.Title>
          <div className="flex h-[52px] items-center gap-3 px-4">
            <Search size={17} className="text-fg-2" />
            <input
              autoFocus
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setIndex(0);
              }}
              placeholder="Switch to thread…"
              aria-label="Switch to thread"
              className="flex-1 bg-transparent text-title3 outline-none placeholder:text-fg-3"
            />
          </div>
          <div className="max-h-[420px] overflow-y-auto p-2" role="listbox" aria-label="Threads">
            {!results.length && (
              <div className="px-3 py-4 text-center text-callout text-fg-2">
                {sessions.length ? `No threads match “${query}”.` : "No threads yet."}
              </div>
            )}
            {results.map((s, i) => (
              <button
                key={s.id}
                type="button"
                role="option"
                aria-selected={i === index}
                onMouseMove={() => setIndex(i)}
                onClick={() => choose(i)}
                className={cx(
                  "flex min-h-[52px] w-full items-center gap-3 rounded-[14px] px-3 py-2.5 text-left",
                  i === index && "bg-brand/16",
                )}
              >
                <MessageSquare size={13} className={i === index ? "text-brand" : "text-fg-2"} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-body">
                    {highlight(s.title || "New thread", query)}
                  </span>
                  <span className="block truncate text-caption text-fg-2">
                    {[projectName(s.cwd), modelName(s.model), relativeAge(s.updatedAt)]
                      .filter(Boolean)
                      .join(" · ")}
                  </span>
                </span>
                {s.id === selectedId && (
                  <span className="rounded-full bg-fg/7 px-[7px] py-0.5 font-medium text-caption text-fg-2">
                    Open
                  </span>
                )}
                {i === index && <CornerDownLeft size={12} className="text-fg-3" />}
              </button>
            ))}
          </div>
          <div className="flex h-[34px] items-center gap-3 bg-fg/[0.03] px-[18px] text-caption text-fg-2">
            <span className="flex items-center gap-1">
              <Kbd>↑↓</Kbd> move
            </span>
            <span className="flex items-center gap-1">
              <Kbd>⏎</Kbd> open
            </span>
            <span className="flex items-center gap-1">
              <Kbd>⇥</Kbd> all / project
            </span>
            <span className="flex items-center gap-1">
              <Kbd>esc</Kbd> close
            </span>
            <span className="flex-1" />
            <span>{scope === "all" ? "All projects" : projectName(projectCwd)}</span>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
