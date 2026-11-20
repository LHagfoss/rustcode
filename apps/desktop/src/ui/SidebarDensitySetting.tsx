// Settings › Appearance › Sidebar rows: the sidebar's density, the same stored preference ⌘K's
// "Sidebar: Compact rows" toggles (state/sidebarPrefs.ts), so both stay in step.

import {
  PREF,
  SETTLE_AFTER_CHOICES,
  SETTLE_AFTER_DEFAULT_DAYS,
  SIDEBAR_DENSITIES,
  settleAfterDays,
  usePref,
  useSidebarDensity,
} from "../state/sidebarPrefs";
import { Switch } from "./primitives";
import { Choice, Row } from "./settingsKit";

export function SidebarDensitySetting() {
  const [density, setDensity] = useSidebarDensity();
  return (
    <Row
      title="Sidebar rows"
      reset={density !== "detailed" ? () => setDensity("detailed") : undefined}
      subtitle="Compact shows one line per thread."
      hint="Hover a compact row for its project, branch and model."
    >
      <Choice
        label="Sidebar rows"
        value={density}
        onChange={setDensity}
        options={SIDEBAR_DENSITIES.map((d) => ({ value: d.id, label: d.label }))}
      />
    </Row>
  );
}

/** Settings › Appearance › Sidebar toolbar: the search, sort and New thread row over the list. */
export function SidebarToolbarSetting() {
  const [on, setOn] = usePref(PREF.toolbar, true);
  return (
    <Row
      title="Sidebar toolbar"
      reset={on ? undefined : () => setOn(true)}
      subtitle="New thread and Jobs above the thread list."
      hint="⌘K has every action too."
    >
      <Switch label="Sidebar toolbar" checked={on} onChange={setOn} />
    </Row>
  );
}

/** Settings › Appearance › Settle idle threads: how long a thread rests before Settled takes it. */
export function SettleAfterSetting() {
  const [raw, setDays] = usePref<number>(PREF.settleAfterDays, SETTLE_AFTER_DEFAULT_DAYS);
  const days = settleAfterDays(raw);
  return (
    <Row
      title="Settle idle threads"
      reset={days !== SETTLE_AFTER_DEFAULT_DAYS ? () => setDays(SETTLE_AFTER_DEFAULT_DAYS) : undefined}
      subtitle="An untouched thread moves to the Settled shelf after this long."
      hint="A thread waiting for review stays. Settle or unsettle one by hand from its menu."
    >
      <Choice
        label="Settle idle threads"
        value={String(days)}
        onChange={(value) => setDays(Number(value))}
        options={SETTLE_AFTER_CHOICES.map((c) => ({ value: String(c.days), label: c.label }))}
      />
    </Row>
  );
}
