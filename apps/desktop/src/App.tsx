import { Tooltip } from "radix-ui";
import { lazy, Suspense, useEffect, useState } from "react";
import {
  cancel,
  openSettings,
  setOnboarding,
  setSwitcher,
  startNewThread,
  toggleSidebar,
  useApp,
} from "./state/store";
import { ChatView } from "./ui/ChatView";
import { cx } from "./ui/primitives";
import { Sidebar } from "./ui/Sidebar";
import { Switcher } from "./ui/Switcher";

const Settings = lazy(() => import("./ui/Settings").then((m) => ({ default: m.Settings })));
const Onboarding = lazy(() => import("./ui/Onboarding").then((m) => ({ default: m.Onboarding })));
const ChangesPanel = lazy(() => import("./ui/ChangesPanel").then((m) => ({ default: m.ChangesPanel })));

function useTheme() {
  const theme = useApp((s) => s.theme);
  useEffect(() => {
    if (theme === "system") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = theme;
  }, [theme]);
}

function useShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      const s = useApp.getState();
      if (mod && !e.shiftKey && e.key.toLowerCase() === "n") {
        e.preventDefault();
        startNewThread();
      } else if (mod && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSwitcher(!s.switcherOpen);
      } else if (mod && e.key === ",") {
        e.preventDefault();
        openSettings(s.settings ? null : "general");
      } else if (mod && e.key.toLowerCase() === "b") {
        e.preventDefault();
        toggleSidebar();
      } else if (mod && e.key === ".") {
        e.preventDefault();
        void cancel();
      } else if (e.key === "Escape" && s.settings && !s.onboardingOpen) {
        const target = e.target as HTMLElement | null;
        if (!target?.closest("input, textarea")) openSettings(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

function Toasts() {
  const toasts = useApp((s) => s.toasts);
  return (
    <div
      className="pointer-events-none fixed top-12 right-0 left-0 z-50 flex flex-col items-center gap-2"
      aria-live="polite"
    >
      {toasts.map((t) => (
        <div
          key={t.id}
          className={cx(
            "surface pointer-events-auto max-w-[480px] rounded-full px-4 py-1.5 text-callout rc-in",
            t.tone === "warning" && "text-attention",
          )}
        >
          {t.text}
        </div>
      ))}
    </div>
  );
}

export function App() {
  useTheme();
  useShortcuts();
  const settings = useApp((s) => s.settings);
  const onboarding = useApp((s) => s.onboardingOpen);
  const [changes, setChanges] = useState(false);
  return (
    <Tooltip.Provider delayDuration={600}>
      <div className="flex h-full w-full overflow-hidden">
        <Sidebar />
        <ChatView changesOpen={changes} onToggleChanges={() => setChanges(!changes)} />
        {changes && (
          <Suspense fallback={null}>
            <ChangesPanel onClose={() => setChanges(false)} />
          </Suspense>
        )}
      </div>
      <Switcher />
      <Suspense fallback={null}>
        {settings && <Settings />}
        {onboarding && <Onboarding onClose={() => setOnboarding(false)} />}
      </Suspense>
      <Toasts />
    </Tooltip.Provider>
  );
}
