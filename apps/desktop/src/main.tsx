import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { defaultTransport, isTauri } from "./acp/transport";
import "./index.css";
import { applyCachedTheme } from "./lib/themes";
import * as store from "./state/store";

// The last theme, before the first render, so a custom theme does not flash the default.
applyCachedTheme();

// Surfaces are opaque (docs/design.md); the few frosted ones go solid under Reduce transparency.
if (isTauri() && /Mac/i.test(navigator.userAgent)) {
  void import("./lib/shell")
    .then(({ shellCall }) => shellCall<boolean>("reduce_transparency"))
    .then((reduce) => document.documentElement.classList.toggle("reduce-transparency", reduce))
    .catch(() => undefined);
}

const root = document.getElementById("root");
if (!root) throw new Error("No #root element");

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

// Dev builds expose the store and its actions, so an agent can drive and inspect the UI
// (`window.__rustcode.useApp.getState()`), as DebugDriver does for the SwiftUI app.
// `?transport=perf` exposes them in a production build too, for `bun run perf`.
if (import.meta.env.DEV || new URLSearchParams(window.location.search).get("transport") === "perf")
  Object.assign(window, { __rustcode: store });

void defaultTransport().then(store.connect);

// ⌘⌥F: the frame-rate overlay (docs/desktop.md › Native frame rate). Dev builds only.
if (import.meta.env.DEV) void import("./lib/perfOverlay").then((m) => m.installPerfOverlay());
