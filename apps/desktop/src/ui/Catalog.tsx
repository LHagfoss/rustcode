// `?catalog` in a dev build: every shared control on one page, so a change to a recipe, or a
// control that drifts from it, shows at a glance. Not part of the app (main.tsx).

import { Copy, Plus, Settings, Trash2 } from "lucide-react";
import { DropdownMenu, Tooltip as RadixTooltip } from "radix-ui";
import { type ReactNode, useState } from "react";
import {
  Callout,
  ChromeButton,
  Cluster,
  ClusterButton,
  inputClass,
  Kbd,
  menuItem,
  menuItemDanger,
  menuLabel,
  menuSeparator,
  menuSurface,
  Pill,
  Segments,
  Select,
  Skeleton,
  Spinner,
  Switch,
} from "./primitives";

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-3">
      <h2 className="font-semibold text-title3">{title}</h2>
      <div className="flex flex-wrap items-center gap-3 rounded-lg bg-surface p-3.5">{children}</div>
    </section>
  );
}

const TYPE = ["caption", "sub", "callout", "body", "title3", "title2", "title"] as const;
const RADII = ["xs", "sm", "md", "lg", "xl", "2xl", "full"] as const;
const OPTIONS = [
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
];

export function Catalog() {
  const [segment, setSegment] = useState("medium");
  const [choice, setChoice] = useState("medium");
  const [on, setOn] = useState(true);
  return (
    <RadixTooltip.Provider delayDuration={400} skipDelayDuration={300} disableHoverableContent>
      <main className="selectable h-screen overflow-y-auto bg-bg text-fg">
        <div className="mx-auto flex max-w-[840px] flex-col gap-6 px-6 py-10">
          <h1 className="font-semibold text-title2">Controls</h1>
          <Section title="Type">
            <div className="flex w-full flex-col gap-1">
              {TYPE.map((t) => (
                <span key={t} className={`text-${t}`}>
                  {t}: The quick brown fox
                </span>
              ))}
            </div>
          </Section>
          <Section title="Radius">
            {RADII.map((r) => (
              <span
                key={r}
                className={`flex size-14 items-center justify-center bg-fill text-caption rounded-${r}`}
              >
                {r}
              </span>
            ))}
          </Section>
          <Section title="Pill">
            <Pill size="sm">Small</Pill>
            <Pill>Secondary</Pill>
            <Pill size="lg">Large</Pill>
            <Pill prominent>Primary</Pill>
            <Pill danger>Danger</Pill>
            <Pill quiet>
              <Plus size={12} /> Quiet
            </Pill>
            <Pill disabled>Disabled</Pill>
          </Section>
          <Section title="Icon buttons">
            <ChromeButton label="Copy">
              <Copy size={14} />
            </ChromeButton>
            <Cluster>
              <ClusterButton label="Settings">
                <Settings size={14} />
              </ClusterButton>
              <ClusterButton label="On" on>
                <Copy size={14} />
              </ClusterButton>
            </Cluster>
          </Section>
          <Section title="Choices">
            <Segments label="Segments" value={segment} onChange={setSegment} options={OPTIONS} />
            <Select label="Select" value={choice} onChange={setChoice} options={OPTIONS} />
            <Select label="Small select" size="sm" value={choice} onChange={setChoice} options={OPTIONS} />
            <Select label="Off" disabled value={choice} onChange={setChoice} options={OPTIONS} />
            <Switch label="Switch" checked={on} onChange={setOn} />
          </Section>
          <Section title="Field">
            <input className={inputClass} placeholder="Placeholder" />
            <Kbd>⌘K</Kbd>
          </Section>
          <Section title="Menu">
            <DropdownMenu.Root modal={false}>
              <DropdownMenu.Trigger asChild>
                <Pill>Open menu</Pill>
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content align="start" sideOffset={6} className={menuSurface}>
                  <DropdownMenu.Label className={menuLabel}>Label</DropdownMenu.Label>
                  <DropdownMenu.Item className={menuItem}>
                    <Copy size={13} className="text-fg-muted" /> Item
                  </DropdownMenu.Item>
                  <DropdownMenu.Item disabled className={menuItem}>
                    Disabled
                  </DropdownMenu.Item>
                  <DropdownMenu.Separator className={menuSeparator} />
                  <DropdownMenu.Item className={`${menuItem} ${menuItemDanger}`}>
                    <Trash2 size={13} /> Destructive
                  </DropdownMenu.Item>
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
          </Section>
          <Section title="Status">
            <Spinner />
            <Skeleton className="h-3 w-40" />
            <Callout tone="warning">What happened and what to do.</Callout>
          </Section>
        </div>
      </main>
    </RadixTooltip.Provider>
  );
}
