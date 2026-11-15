import { describe, expect, test } from "bun:test";
import { Copy, Pin } from "lucide-react";
import { Tooltip } from "radix-ui";
import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { CardMenuItem } from "./cardPopover";
import { MenuQuickActions } from "./primitives";

function render(actions: ComponentProps<typeof MenuQuickActions>["actions"]) {
  return renderToStaticMarkup(
    <Tooltip.Provider>
      <MenuQuickActions Item={CardMenuItem} actions={actions} />
    </Tooltip.Provider>,
  );
}

describe("menu quick actions", () => {
  test("uses equal square columns for three to five actions", () => {
    for (const count of [3, 4, 5]) {
      const html = render(
        Array.from({ length: count }, (_, i) => ({
          label: `Action ${i}`,
          icon: <Copy />,
          onSelect: () => {},
        })),
      );
      expect(html).toContain(`repeat(${count}, minmax(0, 1fr))`);
      expect(html.match(/aspect-square/g)?.length).toBe(count);
      expect(html).toContain('aria-label="Quick actions"');
    }
  });

  test("keeps a short visible label, full accessible name, toggle and disabled state", () => {
    const html = render([
      { label: "Unpin", description: "Unpin 3 Threads", icon: <Pin />, checked: true, onSelect: () => {} },
      { label: "Pin", icon: <Pin />, checked: false, onSelect: () => {} },
      { label: "Copy", description: "Copy Link", icon: <Copy />, disabled: true, onSelect: () => {} },
    ]);
    expect(html).toContain('role="menuitemcheckbox"');
    expect(html).toContain('aria-checked="true"');
    expect(html).toContain('aria-checked="false"');
    expect(html).toContain('aria-label="Unpin 3 Threads"');
    expect(html).toContain('aria-label="Copy Link"');
    expect(html).toContain('disabled=""');
    expect(html).toContain('aria-hidden="true"');
    expect(html).toContain(">Unpin</span>");
  });

  test("an empty action set adds no section", () => {
    expect(render([])).toBe("");
  });
});
