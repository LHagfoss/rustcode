import { Tooltip } from "radix-ui";
import { lazy, Suspense, useEffect } from "react";
import { applyTheme, resolveTheme } from "./lib/themes";
import { restoreZoom } from "./lib/zoom";
import { runShortcut } from "./state/shortcuts";
import { closeConfirm, openSettings, setOnboarding, setView, shortcutFor, useApp } from "./state/store";
import { startUpdateChecks } from "./state/updates";
import { ChatView } from "./ui/ChatView";
import { CommandPalette } from "./ui/CommandPalette";
import { Inbox } from "./ui/Inbox";
import { Lightbox } from "./ui/Lightbox";
import { GrantDialog } from "./ui/Links";
import { Pill } from "./ui/primitives";
// Eager, not lazy: the first ⌘, must not wait for a chunk (it showed an empty pane for frames).
import { Settings } from "./ui/Settings";
import { Sidebar } from "./ui/Sidebar";
import { Toaster } from "./ui/Toaster";
import { WhatsNewDialog } from "./ui/Updates";
import { useAwayWatch } from "./ui/WhileAway";

const Onboarding = lazy(() => import("./ui/Onboarding").then((m) => ({ default: m.Onboarding })));
const TerminalPanel = lazy(() => import("./ui/TerminalPanel").then((m) => ({ default: m.TerminalPanel })));
const Board = lazy(() => import("./ui/Board").then((m) => ({ default: m.Board })));
const Review = lazy(() => import("./ui/Review").then((m) => ({ default: m.Review })));
const Compare = lazy(() => import("./ui/Compare").then((m) => ({ default: m.Compare })));

/**
 * Paints the appearance on :root (lib/themes.ts) whenever the mode, the picked
 * theme, the engine's themes, the system appearance, the font or the chat width
 * change. It subscribes outside React, so a theme change re-renders nothing.
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
      document.documentElement.dataset.font = s.roundedFont ? "rounded" : "standard";
      document.documentElement.dataset.chatWidth = s.chatWidth;
    };
    paint();
    media.addEventListener("change", paint);
    const unsubscribe = useApp.subscribe((s, prev) => {
      if (
        s.theme !== prev.theme ||
        s.themeActive !== prev.themeActive ||
        s.themes !== prev.themes ||
        s.roundedFont !== prev.roundedFont ||
        s.chatWidth !== prev.chatWidth
      )
        paint();
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
      // One table, one matcher (lib/keys.ts): Settings › Keyboard rebinds it and
      // this handler needs no change. A command the panel can change runs here;
      // the transcript matches "find in thread" itself.
      const command = shortcutFor(e);
      if (command && runShortcut(command)) {
        e.preventDefault();
        return;
      }
      const s = useApp.getState();
      if (
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
        className="elevated w-[min(420px,calc(100vw-48px))] p-5 rc-in"
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
  useAwayWatch();
  useEffect(() => restoreZoom(), []);
  useEffect(() => startUpdateChecks(), []);
  const settings = useApp((s) => s.settings !== null);
  const onboarding = useApp((s) => s.onboardingOpen);
  const view = useApp((s) => s.view);
  return (
    <Tooltip.Provider delayDuration={400} skipDelayDuration={300} disableHoverableContent>
      {/*
        One shell, never unmounted. Settings floats over it (docs/design.md ›
        Settings): the shell stays mounted and laid out underneath, inert, so
        closing Settings shows the thread exactly where it was (scroll, drafts,
        open rows) in one frame.
      */}
      <div className="relative h-full w-full overflow-hidden">
        <div className="flex h-full w-full overflow-hidden bg-sidebar" inert={settings}>
          <Sidebar />
          <div className="relative flex min-w-0 flex-1 bg-bg">
            {/*
              The view and the terminal under it. The right panel is docked inside
              the chat view's own row (ui/ChatView.tsx), so it is never drawn over
              the board, the review or the compare view.
            */}
            <div className="flex min-w-0 flex-1 flex-col">
              {view.kind === "chat" ? (
                <ChatView />
              ) : view.kind === "inbox" ? (
                <Inbox />
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
          </div>
        </div>
        {settings && <Settings />}
      </div>
      <CommandPalette />
      <Suspense fallback={null}>{onboarding && <Onboarding onClose={() => setOnboarding(false)} />}</Suspense>
      <ConfirmDialog />
      <GrantDialog />
      <Toaster />
      <WhatsNewDialog />
      <Lightbox />
    </Tooltip.Provider>
  );
}
