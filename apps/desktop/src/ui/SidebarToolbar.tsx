import { DropdownMenu } from "radix-ui";
import type { ReactNode } from "react";
import { projectName } from "../acp/types";
import {
  DEFAULT_SIDEBAR_SORT,
  PREF,
  SIDEBAR_SORTS,
  usePref,
  useSidebarDensity,
  useSidebarSort,
} from "../state/sidebarPrefs";
import { setView, toast, useApp } from "../state/store";
import { applySidebarSort } from "../state/threadActions";
import { startNewThreadFromContext } from "../state/threadOpen";
import { ArrowDownUp, Check, ListFilter, Repeat, SquarePen, X } from "./icons";
import { cx, MenuContent, menuItem, menuLabel, menuSeparator, Tooltip } from "./primitives";

const TOOL_BUTTON =
  "relative inline-flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg data-[state=open]:bg-fill data-[state=open]:text-fg";

/** A row in the nav block over the search: New thread and the destinations beside the threads. */
const NAV_ROW =
  "flex h-8 w-full shrink-0 items-center gap-2 rounded-md px-3 font-medium text-callout text-fg-muted hover:bg-fill hover:text-fg";

function NavRow({
  label,
  current,
  onClick,
  children,
}: {
  label: string;
  current?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-current={current ? "page" : undefined}
      className={cx(NAV_ROW, current && "bg-fill-strong text-fg hover:bg-fill-strong")}
      onClick={onClick}
    >
      {children}
      <span className="flex-1 text-left">{label}</span>
    </button>
  );
}

export function SidebarToolbar() {
  const projectCwd = useApp((s) => s.projectCwd);
  const view = useApp((s) => s.view.kind);
  const [onlyProject, setOnlyProject] = usePref(PREF.onlyProject, false);
  const filtering = onlyProject && !!projectCwd;
  return (
    <div className="flex shrink-0 flex-col gap-1.5 px-3 pb-2">
      <nav aria-label="Sidebar" className="flex flex-col gap-0.5 pb-1">
        <button
          type="button"
          aria-label="New thread"
          className={NAV_ROW}
          onClick={() => startNewThreadFromContext()}
        >
          <SquarePen size={14} />
          <span className="flex-1 text-left">New thread</span>
        </button>
        <NavRow label="Loops" current={view === "loops"} onClick={() => setView({ kind: "loops" })}>
          <Repeat size={14} />
        </NavRow>
      </nav>
      {/* A hairline sets the actions apart from the list under them. */}
      <div aria-hidden className="mx-1 border-border border-b" />
      {filtering && (
        <div className="flex min-w-0 items-center">
          <span className="inline-flex h-6 min-w-0 items-center gap-1 rounded-full bg-accent/12 pr-1 pl-3 text-caption text-accent">
            <span className="truncate">Only {projectName(projectCwd)}</span>
            <Tooltip label="Show every project">
              <button
                type="button"
                aria-label="Show every project"
                onClick={() => setOnlyProject(false)}
                className="inline-flex size-4 shrink-0 items-center justify-center rounded-full hover:bg-accent/20"
              >
                <X size={12} />
              </button>
            </Tooltip>
          </span>
        </div>
      )}
    </div>
  );
}

/** The heading of Pinned or Threads: it names the group, sits close to its rows, and never folds. */
export function ThreadSectionLabel({
  label,
  threads,
  filtering,
}: {
  label: string;
  threads: boolean;
  filtering: boolean;
}) {
  return (
    <div
      className={cx(
        "flex w-full px-2 font-medium text-caption text-fg-faint",
        threads ? "h-7 items-center gap-1" : "h-5 items-start",
      )}
    >
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {threads && (
        <>
          <SortMenu />
          <FilterMenu filtering={filtering} />
        </>
      )}
    </div>
  );
}

/** Ordering is independent of which threads the list shows. */
export function SortMenu() {
  const [sort] = useSidebarSort();
  return (
    <DropdownMenu.Root>
      <Tooltip label="Sort threads">
        <DropdownMenu.Trigger asChild>
          <button type="button" aria-label="Sort threads" className={TOOL_BUTTON}>
            <ArrowDownUp size={14} />
            {sort !== DEFAULT_SIDEBAR_SORT && (
              <span aria-hidden className="absolute top-1 right-1 size-1.5 rounded-full bg-accent" />
            )}
          </button>
        </DropdownMenu.Trigger>
      </Tooltip>
      <MenuContent align="end" className="min-w-[220px]">
        <DropdownMenu.Label className={menuLabel}>Sort by</DropdownMenu.Label>
        <DropdownMenu.RadioGroup value={sort}>
          {SIDEBAR_SORTS.map((option) => (
            <DropdownMenu.RadioItem
              key={option.id}
              value={option.id}
              className={menuItem}
              onSelect={() => applySidebarSort(option.id)}
            >
              {option.label}
              {sort === option.id && <Check size={14} className="ms-auto shrink-0 text-accent" />}
            </DropdownMenu.RadioItem>
          ))}
        </DropdownMenu.RadioGroup>
      </MenuContent>
    </DropdownMenu.Root>
  );
}

/** Filtering and the existing list presentation preferences. */
export function FilterMenu({ filtering }: { filtering: boolean }) {
  const projectCwd = useApp((s) => s.projectCwd);
  const [onlyProject, setOnlyProject] = usePref(PREF.onlyProject, false);
  const [byProject, setByProject] = usePref(PREF.groupByProject, false);
  const [working, setWorking] = usePref(PREF.workingSection, true);
  const [density, setDensity] = useSidebarDensity();
  const [, setToolbar] = usePref(PREF.toolbar, true);
  const check = (on: boolean) => (on ? <Check size={14} className="ms-auto shrink-0 text-accent" /> : null);
  return (
    <DropdownMenu.Root>
      <Tooltip label="Filter threads">
        <DropdownMenu.Trigger asChild>
          <button type="button" aria-label="Filter threads" className={TOOL_BUTTON}>
            <ListFilter size={14} />
            {/* Something other than the default is on: the button says so. */}
            {filtering && (
              <span aria-hidden className="absolute top-1 right-1 size-1.5 rounded-full bg-accent" />
            )}
          </button>
        </DropdownMenu.Trigger>
      </Tooltip>
      <MenuContent align="end" className="min-w-[220px]">
        <DropdownMenu.Label className={menuLabel}>Show</DropdownMenu.Label>
        <DropdownMenu.CheckboxItem
          className={menuItem}
          checked={onlyProject}
          disabled={!projectCwd}
          onCheckedChange={(on) => setOnlyProject(on === true)}
          onSelect={(e) => e.preventDefault()}
        >
          <span className="min-w-0 truncate">
            {projectCwd ? `Only ${projectName(projectCwd)}` : "Only this project"}
          </span>
          {check(onlyProject)}
        </DropdownMenu.CheckboxItem>
        <DropdownMenu.CheckboxItem
          className={menuItem}
          checked={byProject}
          onCheckedChange={(on) => setByProject(on === true)}
          onSelect={(e) => e.preventDefault()}
        >
          Group by project
          {check(byProject)}
        </DropdownMenu.CheckboxItem>
        <DropdownMenu.CheckboxItem
          className={menuItem}
          checked={working}
          onCheckedChange={(on) => setWorking(on === true)}
          onSelect={(e) => e.preventDefault()}
        >
          Working at the top
          {check(working)}
        </DropdownMenu.CheckboxItem>
        <DropdownMenu.CheckboxItem
          className={menuItem}
          checked={density === "compact"}
          onCheckedChange={(on) => setDensity(on === true ? "compact" : "detailed")}
          onSelect={(e) => e.preventDefault()}
        >
          Compact rows
          {check(density === "compact")}
        </DropdownMenu.CheckboxItem>
        <DropdownMenu.Separator className={menuSeparator} />
        <DropdownMenu.Item
          className={cx(menuItem, "text-fg-muted")}
          onSelect={() => {
            setToolbar(false);
            toast("Toolbar hidden. Turn it back on in Settings › Appearance or ⌘K.", "info", {
              action: { label: "Undo", run: () => setToolbar(true) },
            });
          }}
        >
          Hide toolbar
        </DropdownMenu.Item>
      </MenuContent>
    </DropdownMenu.Root>
  );
}
