import { describe, expect, test } from "bun:test";
import { Tooltip } from "radix-ui";
import type {
  ButtonHTMLAttributes,
  ComponentProps,
  ReactElement,
  MouseEvent as ReactMouseEvent,
} from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { CardMenuItem } from "./cardPopover";
import { Copy, Pin } from "./icons";
import { MenuQuickActions } from "./primitives";

function render(actions: ComponentProps<typeof MenuQuickActions>["actions"]) {
  return renderToStaticMarkup(
    <Tooltip.Provider>
      <MenuQuickActions Item={CardMenuItem} actions={actions} />
    </Tooltip.Provider>,
  );
}

let rowElement: ReactElement<ButtonHTMLAttributes<HTMLButtonElement>> | undefined;

/**
 * A menu row's button. The row is called inside a server render (which is what gives it a hook
 * dispatcher) because the props that decide a click are not markup: this suite has no DOM, and
 * what a wrapper does to a row shows only in the props the row passes on.
 */
function RowProbe({ row }: { row: ComponentProps<typeof CardMenuItem> }) {
  rowElement = CardMenuItem(row) as ReactElement<ButtonHTMLAttributes<HTMLButtonElement>>;
  return null;
}

function rowButton(
  row: ComponentProps<typeof CardMenuItem>,
): ReactElement<ButtonHTMLAttributes<HTMLButtonElement>> {
  rowElement = undefined;
  renderToStaticMarkup(<RowProbe row={row} />);
  if (!rowElement) throw new Error("the row rendered no element");
  return rowElement;
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

describe("a menu row a tooltip wraps", () => {
  // `Tooltip` hands the row it wraps an `onClick` of its own through a Radix slot (dismiss the
  // tip), so the row has to run both rather than let one replace the other: every tile in the
  // composer's + menu is wrapped, and a row that lost its action left the menu open doing
  // nothing.
  test("runs the row's action and the tooltip's click", () => {
    const order: string[] = [];
    const row = rowButton({
      onSelect: () => order.push("row"),
      onClick: () => order.push("tip"),
      children: "Attach",
    });
    row.props.onClick?.({} as ReactMouseEvent<HTMLButtonElement>);
    expect(order).toEqual(["row", "tip"]);
  });

  test("needs no wrapper to run its action", () => {
    let selected = 0;
    const row = rowButton({ onSelect: () => (selected += 1), children: "Attach" });
    row.props.onClick?.({} as ReactMouseEvent<HTMLButtonElement>);
    expect(selected).toBe(1);
  });
});
