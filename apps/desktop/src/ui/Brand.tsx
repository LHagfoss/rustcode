// The sidebar's brand: the app's mark, its name, and the version of the app
// that is running, so a screenshot or a bug report says which build it is.
//
// It is the title row's leading content (ui/Titlebar.tsx), past the traffic
// lights, so the sidebar's header is one row rather than a bar with a row under
// it; the name truncates before it reaches the sidebar's toggle.

import { useEffect, useState } from "react";
import { systemInfo } from "../lib/native";
import { useUpdates } from "../state/updates";
import { cx } from "./primitives";

/** `1.21.0` as the sidebar says it, or null when the app cannot say. */
export function versionLabel(version: string | null | undefined): string | null {
  const text = version?.trim().replace(/^v/i, "");
  return text ? `v${text}` : null;
}

/**
 * The running app's version: the shell's own package version (`src-tauri`'s
 * `system_info`), which the launch already asked for (state/updates.ts). The
 * browser — `dev:web` and the mock window — only answers `system_info`, so the
 * app's own version is asked there instead of the line standing empty.
 */
function useAppVersion(): string | null {
  const known = useUpdates((s) => s.version);
  const [asked, setAsked] = useState<string | null>(null);
  useEffect(() => {
    if (known) return;
    let live = true;
    void systemInfo().then((info) => {
      if (live) setAsked(info?.appVersion ?? null);
    });
    return () => {
      live = false;
    };
  }, [known]);
  return known ?? asked;
}

/** The mark and name, with the running version after them in quieter type. */
export function Brand({ className }: { className?: string }) {
  const version = useAppVersion();
  const label = versionLabel(version);
  return (
    <div
      data-tauri-drag-region
      className={cx("@container flex min-w-0 select-none items-center gap-1.5", className)}
      title={version ? `RustCode ${version}` : "RustCode"}
    >
      {/* Nothing here takes a press: the row under it drags the window, its toggle is the one control. */}
      <img
        src="/icon.png"
        alt=""
        width={18}
        height={18}
        draggable={false}
        className="pointer-events-none shrink-0 rounded-xs"
      />
      {/* The name at the step above the list's own 12px labels; the version sits after it, and
          gives way first when the row has no room for the two (a narrow sidebar). */}
      <span className="pointer-events-none min-w-0 truncate font-medium text-callout text-fg">RustCode</span>
      {label && (
        <span className="pointer-events-none hidden shrink-0 text-caption text-fg-faint tabular @min-[130px]:inline">
          {label}
        </span>
      )}
    </div>
  );
}
