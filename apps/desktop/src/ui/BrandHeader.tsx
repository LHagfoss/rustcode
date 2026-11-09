// The sidebar's brand: the app's mark, its name, and the version of the app
// that is running, so a screenshot or a bug report says which build it is.
//
// It is the row under the title bar's, not part of it: the traffic lights take
// the left of that row (TitlebarRow's 92px inset), so a name there would start
// past them and drift further right as the sidebar is dragged wider, while this
// row keeps the list's own 12px edge at every width and on every platform.

import { useEffect, useState } from "react";
import { systemInfo } from "../lib/native";
import { useUpdates } from "../state/updates";

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

export function BrandHeader() {
  const version = useAppVersion();
  const label = versionLabel(version);
  return (
    <div
      className="flex shrink-0 select-none items-center gap-2 px-3 pt-0.5 pb-1.5"
      title={version ? `RustCode ${version}` : "RustCode"}
    >
      <img src="/icon.png" alt="" width={18} height={18} draggable={false} className="shrink-0 rounded-xs" />
      <span className="flex min-w-0 flex-col">
        {/* The name at the step above the list's own 12px labels, the version quiet under it. */}
        <span className="truncate font-medium text-callout text-fg">RustCode</span>
        {label && <span className="truncate text-caption text-fg-muted tabular">{label}</span>}
      </span>
    </div>
  );
}
