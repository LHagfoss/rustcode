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
      subtitle="Compact puts each thread on one line; hover a row for its project, branch and model."
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
