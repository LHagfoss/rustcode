// The sidebar's toolbar, over the thread list: a search that narrows the list as you type, the
// sort and filter menus (what shows and how dense the rows are) and New thread. Every option in it is the
// same stored preference ⌘K toggles (state/sidebarPrefs.ts), so the two never disagree; the
// toolbar itself can be turned off in Settings › Appearance or from ⌘K.
//
// A filter that hides threads always says so: "Only <project>" shows as a chip under the field,
// with its own clear button, so a short list is never a mystery.

import { DropdownMenu } from "radix-ui";
import type { KeyboardEvent, Ref } from "react";
import { projectName } from "../acp/types";
import {
  DEFAULT_SIDEBAR_SORT,
  PREF,
  SIDEBAR_SORTS,
  usePref,
  useSidebarDensity,
  useSidebarSort,
} from "../state/sidebarPrefs";
import { toast, useApp, useShortcutHint } from "../state/store";
import { applySidebarSort } from "../state/threadActions";
import { startNewThreadFromContext } from "../state/threadOpen";
import { ArrowDownUp, Check, ListFilter, SquarePen, X } from "./icons";
import { cx, MenuContent, MenuSearchField, menuItem, menuLabel, menuSeparator, Tooltip } from "./primitives";

const TOOL_BUTTON =
  "relative inline-flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg data-[state=open]:bg-fill data-[state=open]:text-fg";

export function SidebarToolbar({
  query,
  onQuery,
  onOpenFirst,
  inputRef,
}: {
  query: string;
  onQuery: (query: string) => void;
  /** ⏎ in the field: open the first thread the search left. */
  onOpenFirst: () => void;
  inputRef?: Ref<HTMLInputElement>;
}) {
  const newKey = useShortcutHint("new-thread");
  const projectCwd = useApp((s) => s.projectCwd);
  const [onlyProject, setOnlyProject] = usePref(PREF.onlyProject, false);
  const filtering = onlyProject && !!projectCwd;
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      if (query) onQuery("");
      else e.currentTarget.blur();
    } else if (e.key === "Enter" && query.trim()) {
      e.preventDefault();
      onOpenFirst();
    }
  };
  return (
    <div className="flex shrink-0 flex-col gap-1.5 px-3 pb-2">
      <button
        type="button"
        aria-label="New thread"
        className="flex h-[38px] w-full shrink-0 items-center gap-2 rounded-md px-3 font-medium text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
        onClick={() => startNewThreadFromContext()}
      >
        <SquarePen size={14} />
        <span className="flex-1 text-left">New thread</span>
        {newKey && <span className="text-caption text-fg-faint">{newKey}</span>}
      </button>
      <div className="flex items-center gap-1">
        <MenuSearchField
          size="sm"
          className="flex-1"
          value={query}
          onChange={onQuery}
          onClear={() => onQuery("")}
          onKeyDown={onKeyDown}
          inputRef={inputRef}
          label="Search threads"
          placeholder="Search threads"
        />
        <SortMenu />
        <FilterMenu filtering={filtering} />
      </div>
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

/** Ordering is independent of which threads the list shows. */
function SortMenu() {
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
function FilterMenu({ filtering }: { filtering: boolean }) {
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
