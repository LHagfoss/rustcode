import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { defaultTransport, isTauri } from "./acp/transport";
import "./index.css";
import * as store from "./state/store";

// The app's window on macOS is transparent over the sidebar material (index.css, `vibrant`).
if (isTauri() && /Mac/i.test(navigator.userAgent)) {
  document.documentElement.classList.add("vibrant");
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
if (import.meta.env.DEV) Object.assign(window, { __rustcode: store });

void defaultTransport().then(store.connect);
