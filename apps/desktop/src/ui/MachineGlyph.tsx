// The glyph a machine draws (docs/desktop.md › Several machines): one of a
// small fixed set, so a paired machine and this computer are told apart at a
// glance. A chosen tint comes from the app's chart palette, never a literal.

import {
  Box,
  CircleHelp,
  Cloud,
  Computer,
  Laptop,
  Monitor,
  Server,
  Smartphone,
  SquareTerminal,
} from "lucide-react";
import type { MachineIcon } from "../state/machines";
import { cx } from "./primitives";

/** The glyph a machine draws, tinted from the app's series palette when it picked one. */
export function DeviceGlyphIcon({
  glyph,
  tint,
  size = 16,
}: {
  glyph: MachineIcon;
  tint?: string | null;
  size?: number;
}) {
  const Icon =
    glyph === "laptop"
      ? Laptop
      : glyph === "desktop"
        ? Monitor
        : glyph === "studio"
          ? Box
          : glyph === "server"
            ? Server
            : glyph === "tower"
              ? Computer
              : glyph === "linux"
                ? SquareTerminal
                : glyph === "cloud"
                  ? Cloud
                  : glyph === "phone"
                    ? Smartphone
                    : CircleHelp;
  const name =
    glyph === "laptop"
      ? "Laptop"
      : glyph === "desktop"
        ? "Desktop"
        : glyph === "studio"
          ? "Mac Studio or mini"
          : glyph === "server"
            ? "Server"
            : glyph === "tower"
              ? "Windows tower"
              : glyph === "linux"
                ? "Linux"
                : glyph === "cloud"
                  ? "Cloud"
                  : glyph === "phone"
                    ? "Phone"
                    : "Custom icon";
  // A token, never a colour literal: the tint follows every theme.
  const style = tint ? { color: `var(--rc-series-${tint})` } : undefined;
  return (
    <Icon
      size={size}
      className={cx("shrink-0", tint ? undefined : "text-fg-muted")}
      style={style}
      aria-label={name}
    />
  );
}
