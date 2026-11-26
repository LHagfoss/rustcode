/** Mock `_rustcode/browser/*` responses for unit tests and `VITE_TRANSPORT=mock`. */

import { type Json, type JsonObject, str } from "./jsonrpc";

const MOCK_FRAME_JPEG =
  "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AVN//2Q==";

const MOCK_CROPPED_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

interface MockSessionPreview {
  url: string;
  title: string;
  history: string[];
  index: number;
  viewport: "desktop" | "tablet" | "mobile";
  width: number;
  height: number;
  inspectMode: boolean;
  screencastActive: boolean;
}

const sessions = new Map<string, MockSessionPreview>();

function sessionOf(id: string): MockSessionPreview {
  const existing = sessions.get(id);
  if (existing) return existing;
  const created: MockSessionPreview = {
    url: "",
    title: "",
    history: [],
    index: -1,
    viewport: "desktop",
    width: 1280,
    height: 800,
    inspectMode: false,
    screencastActive: false,
  };
  sessions.set(id, created);
  return created;
}

function snapshot(s: MockSessionPreview): JsonObject {
  return {
    url: s.url,
    title: s.title,
    canGoBack: s.index > 0,
    canGoForward: s.index >= 0 && s.index + 1 < s.history.length,
    viewport: s.viewport,
    width: s.width,
    height: s.height,
    inspectMode: s.inspectMode,
    screencastActive: s.screencastActive,
    frame: s.url
      ? {
          data: MOCK_FRAME_JPEG,
          width: s.width,
          height: s.height,
          deviceScaleFactor: s.viewport === "desktop" ? 1 : 2,
          timestampMs: Date.now(),
        }
      : null,
  };
}

/** Handles `_rustcode/browser/*` requests in the mock ACP transport. */
export function mockBrowserCall(method: string, params: JsonObject): Json {
  const id = str(params.sessionId) || "default";
  const s = sessionOf(id);

  switch (method) {
    case "_rustcode/browser/open": {
      const url = str(params.url) ?? "http://localhost:5173/";
      s.url = url;
      s.title = "Preview App";
      s.history = [...s.history.slice(0, s.index + 1), url];
      s.index = s.history.length - 1;
      s.screencastActive = true;
      return snapshot(s);
    }
    case "_rustcode/browser/state":
      return snapshot(s);
    case "_rustcode/browser/stream":
      s.screencastActive = params.active !== false;
      return snapshot(s);
    case "_rustcode/browser/navigate": {
      const action = str(params.action) ?? "reload";
      if (action === "goto") {
        const url = str(params.url) ?? s.url;
        s.url = url;
        s.history = [...s.history.slice(0, s.index + 1), url];
        s.index = s.history.length - 1;
      } else if (action === "back" && s.index > 0) {
        s.index -= 1;
        s.url = s.history[s.index] ?? s.url;
      } else if (action === "forward" && s.index + 1 < s.history.length) {
        s.index += 1;
        s.url = s.history[s.index] ?? s.url;
      }
      return snapshot(s);
    }
    case "_rustcode/browser/viewport": {
      const preset = str(params.preset);
      s.viewport = preset === "tablet" || preset === "mobile" ? preset : "desktop";
      s.width =
        typeof params.width === "number"
          ? params.width
          : s.viewport === "tablet"
            ? 768
            : s.viewport === "mobile"
              ? 390
              : 1280;
      s.height =
        typeof params.height === "number"
          ? params.height
          : s.viewport === "tablet"
            ? 1024
            : s.viewport === "mobile"
              ? 844
              : 800;
      return snapshot(s);
    }
    case "_rustcode/browser/input":
      return { ok: true };
    case "_rustcode/browser/inspect":
      s.inspectMode = params.enabled === true;
      return snapshot(s);
    case "_rustcode/browser/pick": {
      s.inspectMode = false;
      const x = typeof params.x === "number" ? params.x : 120;
      const y = typeof params.y === "number" ? params.y : 80;
      return {
        tagName: "button",
        selector: 'button[data-testid="checkout-submit"]',
        accessibleName: "Submit order",
        role: "button",
        reactComponents: ["CheckoutButton", "OrderSummary"],
        textSnippet: "Submit order",
        attributes: {
          "data-testid": "checkout-submit",
          type: "submit",
        },
        computedStyles: {
          display: "inline-flex",
          padding: "8px 16px",
          borderRadius: "8px",
          fontWeight: "600",
        },
        rect: { x, y, width: 140, height: 36 },
        url: s.url || "http://localhost:5173/",
        title: s.title || "Preview App",
        screenshotPng: MOCK_CROPPED_PNG,
      };
    }
    default:
      return { error: `Unknown browser method: ${method}` };
  }
}
