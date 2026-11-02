import { Tooltip } from "radix-ui";
import { lazy, Suspense, useEffect } from "react";
import {
  cancel,
  closeConfirm,
  openSettings,
  setOnboarding,
  setPanel,
  setSwitcher,
  setTerminalOpen,
  startNewThread,
  toggleSidebar,
  useApp,
} from "./state/store";
import { AgentsPanel } from "./ui/AgentViews";
import { ChatView } from "./ui/ChatView";
import { CommandPalette } from "./ui/CommandPalette";
import { cx, Pill } from "./ui/primitives";
import { Sidebar } from "./ui/Sidebar";

const Settings = lazy(() => import("./ui/Settings").then((m) => ({ default: m.Settings })));
const Onboarding = lazy(() => import("./ui/Onboarding").then((m) => ({ default: m.Onboarding })));
const ChangesPanel = lazy(() => import("./ui/ChangesPanel").then((m) => ({ default: m.ChangesPanel })));
const TerminalPanel = lazy(() => import("./ui/TerminalPanel").then((m) => ({ default: m.TerminalPanel })));
const PullsPanel = lazy(() => import("./ui/PullRequests").then((m) => ({ default: m.PullsPanel })));

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
      } else if (mod && e.key.toLowerCase() === "j") {
        e.preventDefault();
        setTerminalOpen(!s.terminalOpen);
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

function ConfirmDialog() {
  const confirm = useApp((s) => s.confirm);
  if (!confirm) return null;
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/25"
      onMouseDown={(e) => e.target === e.currentTarget && closeConfirm()}
    >
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="rc-confirm-title"
        className="surface w-[340px] rounded-[18px] border border-hairline p-5 shadow-panel rc-in"
        onKeyDown={(e) => e.key === "Escape" && closeConfirm()}
      >
        <h2 id="rc-confirm-title" className="font-semibold text-body">
          {confirm.title}
        </h2>
        <p className="mt-1.5 text-callout text-fg-2">{confirm.message}</p>
        <div className="mt-4 flex justify-end gap-2">
          <Pill onClick={closeConfirm}>Cancel</Pill>
          <Pill
            prominent
            autoFocus
            className={confirm.destructive ? "!bg-danger" : undefined}
            onClick={() => {
              closeConfirm();
              confirm.run();
            }}
          >
            {confirm.action}
          </Pill>
        </div>
      </div>
    </div>
  );
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
  const panel = useApp((s) => s.panel);
  return (
    <Tooltip.Provider delayDuration={600}>
      {/* Hidden, not unmounted, under Settings: its sidebar shows the window's material through. */}
      <div className={cx("flex h-full w-full overflow-hidden", settings && "invisible")}>
        <Sidebar />
        <div className="flex min-w-0 flex-1 flex-col">
          <ChatView />
          <Suspense fallback={null}>
            <TerminalPanel />
          </Suspense>
        </div>
        <Suspense fallback={null}>
          {panel === "changes" && <ChangesPanel onClose={() => setPanel(null)} />}
          {panel === "agents" && <AgentsPanel />}
          {panel === "pulls" && <PullsPanel />}
        </Suspense>
      </div>
      <CommandPalette />
      <Suspense fallback={null}>
        {settings && <Settings />}
        {onboarding && <Onboarding onClose={() => setOnboarding(false)} />}
      </Suspense>
      <ConfirmDialog />
      <Toasts />
    </Tooltip.Provider>
  );
}
