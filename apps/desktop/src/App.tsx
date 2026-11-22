import { Tooltip } from "radix-ui";
import { lazy, Suspense, useEffect, useState } from "react";
import {
  CRASH_REPORT_NOTICE_ID,
  crashReportNoticeSeen,
  crashReportsConfigured,
  crashReportsEnabled,
  markCrashReportNoticeSeen,
  setCrashReportsEnabled,
  subscribeCrashReportConfigured,
  syncCrashReporting,
} from "./lib/crashReport";
import { rememberFocus } from "./lib/focusAreas";
import { matchWindowTheme } from "./lib/material";
import { applyTheme, resolveTheme } from "./lib/themes";
import { paintBlur, paintTransparency, setUserReduceTransparency } from "./lib/transparency";
import { restoreZoom } from "./lib/zoom";
import { startDeepLinks } from "./state/deepLinks";
import { sessionsListed } from "./state/engine";
import { runShortcut } from "./state/shortcuts";
import {
  closeConfirm,
  dismissToast,
  openSettings,
  setOnboarding,
  setView,
  shortcutFor,
  toast,
  useApp,
} from "./state/store";
import { startUpdateChecks } from "./state/updates";
import { ChatView } from "./ui/ChatView";
import { CommandPalette } from "./ui/CommandPalette";
import { ConnectRequests } from "./ui/ConnectRequests";
import { Inbox } from "./ui/Inbox";
import { Lightbox } from "./ui/Lightbox";
import { GrantDialog } from "./ui/Links";
import { useLoopBadges } from "./ui/LoopBadge";
import { OutsideApprovals } from "./ui/OutsideApprovals";
import { Pill } from "./ui/primitives";
// Eager, not lazy: the first ⌘, must not wait for a chunk (it showed an empty pane for frames).
import { Settings } from "./ui/Settings";
import { Sidebar } from "./ui/Sidebar";
import { Toaster } from "./ui/Toaster";
import { WhatsNewDialog } from "./ui/Updates";
import { useAwayWatch } from "./ui/WhileAway";
import { WindowControls } from "./ui/WindowControls";

const Onboarding = lazy(() => import("./ui/Onboarding").then((m) => ({ default: m.Onboarding })));
const Board = lazy(() => import("./ui/Board").then((m) => ({ default: m.Board })));
const Loops = lazy(() => import("./ui/Loops").then((m) => ({ default: m.Loops })));
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
      matchWindowTheme(s.theme === "system" ? null : mode);
      document.documentElement.dataset.font = s.font;
      document.documentElement.dataset.textSize = s.textSize;
      document.documentElement.dataset.chatWidth = s.chatWidth;
      setUserReduceTransparency(s.reduceTransparency);
      paintTransparency(s.transparency);
      paintBlur(s.glassBlur);
    };
    paint();
    media.addEventListener("change", paint);
    const unsubscribe = useApp.subscribe((s, prev) => {
      if (
        s.theme !== prev.theme ||
        s.themeActive !== prev.themeActive ||
        s.themes !== prev.themes ||
        s.font !== prev.font ||
        s.textSize !== prev.textSize ||
        s.reduceTransparency !== prev.reduceTransparency ||
        s.transparency !== prev.transparency ||
        s.glassBlur !== prev.glassBlur ||
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
      if (
        command &&
        ((e.repeat && (command === "toggle-model-picker" || command === "toggle-panel")) ||
          runShortcut(command))
      ) {
        e.preventDefault();
        return;
      }
      const s = useApp.getState();
      // A menu or dialog that closed on this Escape (Radix prevents its default) keeps the page.
      if (e.key === "Escape" && e.defaultPrevented) return;
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
    // Each area's last focused control, so F6 returns to it (lib/focusAreas.ts).
    document.addEventListener("focusin", rememberFocus);
    return () => {
      window.removeEventListener("keydown", onKey);
      document.removeEventListener("focusin", rememberFocus);
    };
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

/**
 * Syncs crash reporting status with the shell and shows a one-time notice when
 * automatic crash reporting is active in a build with a configured DSN.
 */
function useCrashReportNotice() {
  const ready = useApp((s) => s.engine.phase === "ready");
  const onboarding = useApp((s) => s.onboardingOpen);
  const [configured, setConfigured] = useState(crashReportsConfigured);

  useEffect(() => subscribeCrashReportConfigured(() => setConfigured(crashReportsConfigured())), []);

  useEffect(() => {
    if (!ready) return;
    void syncCrashReporting();
  }, [ready]);

  useEffect(() => {
    if (!ready || !configured || onboarding) return;
    if (!crashReportsEnabled() || crashReportNoticeSeen()) return;
    toast("Anonymous crash reports are on (stack traces, app version, OS — never prompts or code).", "info", {
      id: CRASH_REPORT_NOTICE_ID,
      persist: true,
      action: {
        label: "Turn off",
        run: () => {
          setCrashReportsEnabled(false);
          dismissToast(CRASH_REPORT_NOTICE_ID);
          toast("Crash reports turned off.", "info");
        },
      },
    });
    let shown = true;
    return useApp.subscribe((state) => {
      const present = state.toasts.some((t) => t.id === CRASH_REPORT_NOTICE_ID && !t.leaving);
      if (shown && !present) {
        shown = false;
        markCrashReportNoticeSeen();
      }
    });
  }, [ready, configured, onboarding]);
}

export function App() {
  useEffect(() => startDeepLinks(), []);
  useTheme();
  useShortcuts();
  useAwayWatch();
  useCrashReportNotice();
  useEffect(() => restoreZoom(), []);
  useEffect(() => startUpdateChecks(), []);
  // The sidebar's Loop badges: which threads loops ran in (issue 3937).
  useLoopBadges(useApp((s) => sessionsListed(s.engine)));
  const settings = useApp((s) => s.settings !== null);
  const onboarding = useApp((s) => s.onboardingOpen);
  const view = useApp((s) => s.view);
  return (
    // Every tip times itself and shares one open/close behaviour (lib/hover.ts); the provider is
    // what Radix's Tooltip and HoverCard require, and nothing here waits on it.
    <Tooltip.Provider>
      {/*
        One shell, never unmounted. Settings floats over it (docs/design.md ›
        Settings): the shell stays mounted and laid out underneath, inert, so
        closing Settings shows the thread exactly where it was (scroll, drafts,
        open rows) in one frame.
      */}
      <div className="relative h-full w-full overflow-hidden">
        {/*
          The window's own controls on Windows, at its top right over the main column's top row:
          they belong to the window, not to a view, so they stay with the sidebar hidden and
          whatever view is open (ui/WindowControls.tsx). macOS's traffic lights and Linux's
          native title bar draw themselves.
        */}
        <WindowControls className="absolute top-0 right-0 z-40" />
        <div className="flex h-full w-full overflow-hidden bg-sidebar-glass" inert={settings}>
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
                  {view.kind === "loops" && <Loops />}
                  {view.kind === "review" && <Review key={view.threadId} threadId={view.threadId} />}
                  {view.kind === "compare" && <Compare fanOut={view.fanOut} />}
                </Suspense>
              )}
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
      <ConnectRequests />
      <OutsideApprovals />
      <WhatsNewDialog />
      <Lightbox />
    </Tooltip.Provider>
  );
}
