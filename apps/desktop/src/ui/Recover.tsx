// A soft reload: the interface mounts again from the store, the engine and
// every running thread carry on. ⌘R does it, and a render error offers it
// instead of a blank window. A webview reload would hand the page a fresh
// engine (src-tauri/src/engine.rs), stopping what runs.

import { RotateCw } from "lucide-react";
import { Component, type ReactNode, useEffect, useState } from "react";
import { refreshHarnesses } from "../state/store";
import { Pill } from "./primitives";

const listeners = new Set<() => void>();

/** Mount the interface again and re-read what the engine lists (harnesses, MCP). */
export function softReload() {
  for (const listener of listeners) listener();
  void refreshHarnesses();
}

class Boundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error) {
    console.error("The interface failed to render:", error);
  }
  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
        <p className="font-semibold text-title3">Something went wrong in this view</p>
        <p className="max-w-[460px] text-callout text-fg-muted">
          Your threads keep running. Reload the view to carry on.
        </p>
        <pre className="selectable m-0 max-w-[560px] overflow-auto rounded-md bg-code-bg p-2 text-left font-mono text-caption text-fg-muted">
          {error.message}
        </pre>
        <Pill prominent size="lg" onClick={softReload}>
          <RotateCw size={13} /> Reload view <span className="text-bg/60">⌘R</span>
        </Pill>
      </div>
    );
  }
}

/** The app under an error boundary, remounted by `softReload` (⌘R). */
export function Recoverable({ children }: { children: ReactNode }) {
  const [generation, setGeneration] = useState(0);
  useEffect(() => {
    const listener = () => setGeneration((g) => g + 1);
    listeners.add(listener);
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "r") {
        e.preventDefault();
        softReload();
      }
    };
    // Capture, so ⌘R works while the boundary shows and before the webview's own reload.
    window.addEventListener("keydown", onKey, true);
    return () => {
      listeners.delete(listener);
      window.removeEventListener("keydown", onKey, true);
    };
  }, []);
  return <Boundary key={generation}>{children}</Boundary>;
}
