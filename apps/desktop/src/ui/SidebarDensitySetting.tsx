// Settings › Appearance › Sidebar rows: the sidebar's density, the same stored preference ⌘K's
// "Sidebar: Compact rows" toggles (state/sidebarPrefs.ts), so both stay in step.

import { SIDEBAR_DENSITIES, useSidebarDensity } from "../state/sidebarPrefs";
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
