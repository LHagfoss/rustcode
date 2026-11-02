import { Tooltip } from "radix-ui";
import { lazy, Suspense, useEffect } from "react";
import { applyTheme, resolveTheme } from "./lib/themes";
import {
  cancel,
  closeConfirm,
  openSettings,
  setOnboarding,
  setSwitcher,
  setTerminalOpen,
  setView,
  startNewThread,
  toggleRightPanel,
  toggleSidebar,
  useApp,
} from "./state/store";
import { ChatView } from "./ui/ChatView";
import { CommandPalette } from "./ui/CommandPalette";
import { Lightbox } from "./ui/Lightbox";
import { GrantDialog } from "./ui/Links";
import { cx, Pill } from "./ui/primitives";
import { RightPanel } from "./ui/RightPanel";
// Eager, not lazy: the first ⌘, must not wait for a chunk (it showed an empty pane for frames).
import { Settings } from "./ui/Settings";
import { Sidebar } from "./ui/Sidebar";
import { Toaster } from "./ui/Toaster";

const Onboarding = lazy(() => import("./ui/Onboarding").then((m) => ({ default: m.Onboarding })));
const TerminalPanel = lazy(() => import("./ui/TerminalPanel").then((m) => ({ default: m.TerminalPanel })));
const Board = lazy(() => import("./ui/Board").then((m) => ({ default: m.Board })));
const Review = lazy(() => import("./ui/Review").then((m) => ({ default: m.Review })));
const Compare = lazy(() => import("./ui/Compare").then((m) => ({ default: m.Compare })));

/**
 * Paints the theme on :root (lib/themes.ts) whenever the mode, the picked
 * theme, the engine's themes or the system appearance change. It subscribes
 * outside React, so a theme change re-renders nothing.
 */
function useTheme() {
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const paint = () => {
      const s = useApp.getState();
      const mode = s.theme === "system" ? (media.matches ? "dark" : "light") : s.theme;
      const theme = resolveTheme(s.themes, s.themeActive, mode);
      applyTheme(theme);
      document.documentElement.dataset.theme = mode;
    };
    paint();
    media.addEventListener("change", paint);
    const unsubscribe = useApp.subscribe((s, prev) => {
      if (s.theme !== prev.theme || s.themeActive !== prev.themeActive || s.themes !== prev.themes) paint();
    });
    return () => {
      media.removeEventListener("change", paint);
      unsubscribe();
    };
  }, []);
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
      } else if (mod && e.altKey && e.code === "KeyB") {
        e.preventDefault();
        toggleRightPanel();
      } else if (mod && e.key.toLowerCase() === "b") {
        e.preventDefault();
        toggleSidebar();
      } else if (mod && e.key.toLowerCase() === "j") {
        e.preventDefault();
        setTerminalOpen(!s.terminalOpen);
      } else if (mod && e.key === ".") {
        e.preventDefault();
        void cancel();
      } else if (
        e.key === "Escape" &&
        !s.settings &&
        s.view.kind !== "chat" &&
        !s.switcherOpen &&
        !s.grant &&
        !s.confirm &&
        !(e.target as HTMLElement | null)?.closest("input, textarea, select")
      ) {
        setView({ kind: "chat" });
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
      className="rc-overlay fixed inset-0 z-50 flex items-center justify-center bg-bg/60"
      onMouseDown={(e) => e.target === e.currentTarget && closeConfirm()}
    >
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="rc-confirm-title"
        className="elevated w-[min(420px,calc(100vw-48px))] rounded-xl p-5 rc-in"
        onKeyDown={(e) => e.key === "Escape" && closeConfirm()}
      >
        <h2 id="rc-confirm-title" className="font-semibold text-body">
          {confirm.title}
        </h2>
        <p className="mt-1.5 text-callout text-fg-muted">{confirm.message}</p>
        <div className="mt-4 flex justify-end gap-2">
          <Pill onClick={closeConfirm}>Cancel</Pill>
          {confirm.alt && (
            <Pill
              danger
              onClick={() => {
                closeConfirm();
                confirm.alt?.run();
              }}
            >
              {confirm.alt.action}
            </Pill>
          )}
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

export function App() {
  useTheme();
  useShortcuts();
  const settings = useApp((s) => s.settings !== null);
  const onboarding = useApp((s) => s.onboardingOpen);
  const view = useApp((s) => s.view);
  return (
    <Tooltip.Provider delayDuration={400} skipDelayDuration={300} disableHoverableContent>
      {/*
        One shell, never unmounted. Settings takes the whole window (sidebar and
        chat): the shell stays mounted and laid out underneath, only hidden and
        inert, so closing Settings shows the thread exactly where it was
        (scroll, drafts, open rows) in one frame.
      */}
      <div className="relative h-full w-full overflow-hidden">
        <div
          className={cx("flex h-full w-full overflow-hidden bg-sidebar", settings && "invisible")}
          inert={settings}
        >
          <Sidebar />
          <div className="relative flex min-w-0 flex-1 bg-bg">
            <div className="flex min-w-0 flex-1 flex-col">
              {view.kind === "chat" ? (
                <ChatView />
              ) : (
                <Suspense fallback={<div className="flex-1 bg-bg" />}>
                  {view.kind === "board" && <Board />}
                  {view.kind === "review" && <Review key={view.threadId} threadId={view.threadId} />}
                  {view.kind === "compare" && <Compare fanOut={view.fanOut} />}
                </Suspense>
              )}
              <Suspense fallback={null}>
                <TerminalPanel />
              </Suspense>
            </div>
            <div className="flex">
              <RightPanel />
            </div>
          </div>
        </div>
        {settings && (
          <div className="absolute inset-0 z-20 flex bg-bg">
            <Settings />
          </div>
        )}
      </div>
      <CommandPalette />
      <Suspense fallback={null}>{onboarding && <Onboarding onClose={() => setOnboarding(false)} />}</Suspense>
      <ConfirmDialog />
      <GrantDialog />
      <Toaster />
      <Lightbox />
    </Tooltip.Provider>
  );
}
