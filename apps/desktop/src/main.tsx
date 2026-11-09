import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { defaultTransport, isTauri } from "./acp/transport";
import "./index.css";
import { installFocusModality } from "./lib/focusModality";
import { applyPlatform } from "./lib/platform";
import { quietNativeChrome } from "./lib/quietNative";
import { applyCachedTheme } from "./lib/themes";
import {
  installReduceTransparency,
  setSystemReduceTransparency,
  setUserReduceTransparency,
} from "./lib/transparency";
import { startMachines } from "./state/machines";
import * as store from "./state/store";
import { Recoverable } from "./ui/Recover";

// The OS look (fonts, glass, motion; docs/design.md › Platforms), before the first render.
const platform = applyPlatform();

// The last theme, before the first render, so a custom theme does not flash the default.
applyCachedTheme();

// Glass goes solid when the user, the system or a web view that cannot blur asks (lib/transparency.ts).
// The kept switch is read here, not in App's effect, so the first frame is already solid for someone
// who asked for that; the system's answer follows below, once the shell has it.
installReduceTransparency();
setUserReduceTransparency(store.useApp.getState().reduceTransparency);

// Surfaces are opaque (docs/design.md); the few frosted ones go solid under Reduce transparency.
if (isTauri() && platform === "mac") {
  void import("./lib/shell")
    .then(({ shellCall }) => shellCall<boolean>("reduce_transparency"))
    .then(setSystemReduceTransparency)
    .catch(() => undefined);
}

// Focus rings only while the keyboard moves focus: on from a Tab, off at the next click.
installFocusModality();

// No native title tooltips or Writing Tools: the app has its own tooltips (lib/quietNative.ts).
quietNativeChrome();

const root = document.getElementById("root");
if (!root) throw new Error("No #root element");

// `?catalog` in a dev build: the shared controls on one page, without an engine (ui/Catalog.tsx).
const catalog = import.meta.env.DEV && new URLSearchParams(window.location.search).has("catalog");

if (catalog) {
  void import("./ui/Catalog").then(({ Catalog }) => createRoot(root).render(<Catalog />));
} else {
  createRoot(root).render(
    <StrictMode>
      <Recoverable>
        <App />
      </Recoverable>
    </StrictMode>,
  );
}

// Dev builds expose the store and its actions, so an agent can drive and inspect the UI
// (`window.__rustcode.useApp.getState()`), as DebugDriver does for the SwiftUI app.
// `?transport=perf` exposes them in a production build too, for `bun run perf`.
if (import.meta.env.DEV || new URLSearchParams(window.location.search).get("transport") === "perf")
  Object.assign(window, { __rustcode: store });

if (!catalog)
  void defaultTransport().then(async (transport) => {
    await store.connect(transport);
    // The other computers this app is paired with (Settings › Machines): their own supervisors.
    await startMachines();
  });

// `?queuePreview` in a dev browser with the mock: a thread's queue, for screenshots (state/queuePreview.ts).
if (import.meta.env.DEV) void import("./state/queuePreview").then((m) => m.installQueuePreview());

// A draft written in the last moment before the window closes still reaches app storage.
window.addEventListener("beforeunload", () => store.flushDrafts());

// ⌘⌥F: the frame-rate overlay (docs/desktop.md › Native frame rate). Dev builds only.
if (import.meta.env.DEV) void import("./lib/perfOverlay").then((m) => m.installPerfOverlay());
