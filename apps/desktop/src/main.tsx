import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { defaultTransport } from "./acp/transport";
import "./index.css";
import { connect } from "./state/store";

const root = document.getElementById("root");
if (!root) throw new Error("No #root element");

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

void defaultTransport().then(connect);
