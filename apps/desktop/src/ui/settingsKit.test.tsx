import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { Group } from "./settingsKit";

const render = (enabled: boolean) =>
  renderToStaticMarkup(
    <Group title="Voice" enabled={enabled} onEnabledChange={() => undefined} footer="A footer">
      <div>Transcribe with</div>
    </Group>,
  );

describe("a group with a feature switch", () => {
  test("off, it is one title row with its switch: the rows and the footer fold away", () => {
    const html = render(false);
    expect(html).toContain('role="switch"');
    expect(html).toContain('aria-checked="false"');
    expect(html).toContain('aria-label="Voice"');
    expect(html).not.toContain("Transcribe with");
    expect(html).not.toContain("A footer");
  });

  test("on, its rows and footer show under the switch", () => {
    const html = render(true);
    expect(html).toContain('aria-checked="true"');
    expect(html).toContain("Transcribe with");
    expect(html).toContain("A footer");
  });

  test("a group without one has no switch", () => {
    const html = renderToStaticMarkup(
      <Group title="Workflows">
        <div>Rows</div>
      </Group>,
    );
    expect(html).not.toContain('role="switch"');
    expect(html).toContain("Rows");
  });
});
