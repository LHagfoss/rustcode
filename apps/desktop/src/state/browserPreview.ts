/**
 * Browser preview panel state and ACP controller (ADR 0012).
 *
 * Drives the thread's shared engine Chrome through `_rustcode/browser/*` on
 * `connectionForThread(threadId)` so local and paired remote threads behave
 * identically over the existing ACP gateway.
 */

import { create } from "zustand";
import { type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import type { Attachment } from "./compose";
import { connectionForThread, setExtras, showPanel, toast } from "./store";
import { localId } from "./thread";
import { splitThreadKey, threadKey } from "./threadKeys";

/** Responsive device viewport presets supported by the preview panel. */
export type ViewportPreset = "desktop" | "tablet" | "mobile";

/** Default logical viewport dimensions for each responsive preset. */
export const VIEWPORT_PRESETS: Record<ViewportPreset, { label: string; width: number; height: number }> = {
  desktop: { label: "Desktop", width: 1280, height: 800 },
  tablet: { label: "Tablet", width: 768, height: 1024 },
  mobile: { label: "Mobile", width: 390, height: 844 },
};

/** One live JPEG frame pushed over `_rustcode/browser/frame`. */
export interface ScreencastFrame {
  data: string;
  width: number;
  height: number;
  deviceScaleFactor: number;
  timestampMs: number;
}

/** Sanitized bounding box of a picked DOM element. */
export interface ElementRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Sanitized element bundle returned by `_rustcode/browser/pick`. */
export interface PickedElement {
  tagName: string;
  selector: string;
  accessibleName?: string | null;
  role?: string | null;
  reactComponents: string[];
  textSnippet: string;
  attributes: Record<string, string>;
  computedStyles: Record<string, string>;
  rect: ElementRect;
  url: string;
  title: string;
  screenshotPng?: string | null;
}

/** Per-thread preview state shown in `BrowserPreviewPanel`. */
export interface ThreadBrowserPreview {
  url: string;
  title: string;
  canGoBack: boolean;
  canGoForward: boolean;
  viewport: ViewportPreset;
  width: number;
  height: number;
  inspectMode: boolean;
  screencastActive: boolean;
  frame: ScreencastFrame | null;
  loading: boolean;
  picking: boolean;
  error: string | null;
}

/** Input event forwarded to `_rustcode/browser/input`. */
export type BrowserInputEvent =
  | {
      kind: "mouse";
      type: "mouseMoved" | "mousePressed" | "mouseReleased" | "mouseWheel";
      x: number;
      y: number;
      button?: "left" | "middle" | "right";
      clickCount?: number;
      deltaX?: number;
      deltaY?: number;
      modifiers?: number;
    }
  | {
      kind: "key";
      type: "keyDown" | "keyUp" | "rawKeyDown";
      key?: string;
      code?: string;
      text?: string;
      windowsVirtualKeyCode?: number;
      modifiers?: number;
    }
  | {
      kind: "text";
      text: string;
    };

const EMPTY_PREVIEW: ThreadBrowserPreview = {
  url: "",
  title: "",
  canGoBack: false,
  canGoForward: false,
  viewport: "desktop",
  width: 1280,
  height: 800,
  inspectMode: false,
  screencastActive: false,
  frame: null,
  loading: false,
  picking: false,
  error: null,
};

interface BrowserPreviewStore {
  byThread: Record<string, ThreadBrowserPreview>;
}

export const useBrowserPreview = create<BrowserPreviewStore>(() => ({
  byThread: {},
}));

/** Resets preview state between unit tests. */
export function resetBrowserPreviewForTests(): void {
  useBrowserPreview.setState({ byThread: {} });
}

function currentOf(threadId: string): ThreadBrowserPreview {
  return useBrowserPreview.getState().byThread[threadId] ?? EMPTY_PREVIEW;
}

function patchThread(threadId: string, patch: Partial<ThreadBrowserPreview>): void {
  useBrowserPreview.setState((state) => ({
    byThread: {
      ...state.byThread,
      [threadId]: {
        ...(state.byThread[threadId] ?? EMPTY_PREVIEW),
        ...patch,
      },
    },
  }));
}

/** Normalizes user-entered preview URLs and rejects unsafe schemes. */
export function normalizePreviewUrl(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  const candidate = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(trimmed)
    ? trimmed
    : /^(localhost|127\.0\.0\.1|\[::1\])(?::\d+)?(\/|$)/i.test(trimmed)
      ? `http://${trimmed}`
      : `https://${trimmed}`;
  try {
    const parsed = new URL(candidate);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
    if (parsed.username || parsed.password) return null;
    return parsed.toString();
  } catch {
    return null;
  }
}

/** Maps a pointer client position on the rendered image to Chrome viewport coordinates. */
export function scalePointerCoords(
  clientX: number,
  clientY: number,
  rect: { left: number; top: number; width: number; height: number },
  frameWidth: number,
  frameHeight: number,
): { x: number; y: number } {
  if (rect.width <= 0 || rect.height <= 0 || frameWidth <= 0 || frameHeight <= 0) {
    return { x: 0, y: 0 };
  }
  const nx = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
  const ny = Math.min(1, Math.max(0, (clientY - rect.top) / rect.height));
  return {
    x: Math.round(nx * frameWidth),
    y: Math.round(ny * frameHeight),
  };
}

/** Compact chip name for a picked DOM element in the composer strip. */
export function formatPickedElementChipLabel(el: PickedElement): string {
  const component = el.reactComponents[0];
  const tag = `<${el.tagName}>`;
  const name = el.accessibleName?.trim();
  const shortName = name ? (name.length > 28 ? `${name.slice(0, 27)}…` : name) : "";
  if (component && shortName) return `${component} ${tag} "${shortName}"`;
  if (component) return `${component} ${tag}`;
  if (shortName) return `${tag} "${shortName}"`;
  return el.selector ? `${tag} ${el.selector}` : tag;
}

/** Formats a picked element into a compact Markdown context block for the prompt. */
export function formatPickedElementMarkdown(el: PickedElement): string {
  const lines: string[] = ["Picked element from Browser Preview:"];
  if (el.reactComponents.length > 0) {
    lines.push(`- React stack: ${el.reactComponents.join(" < ")}`);
  }
  const label = [
    `<${el.tagName}>`,
    el.role ? `role="${el.role}"` : "",
    el.accessibleName ? `name="${el.accessibleName}"` : "",
  ]
    .filter(Boolean)
    .join(" ");
  lines.push(`- Element: ${label}`);
  if (el.selector) lines.push(`- Selector: \`${el.selector}\``);
  if (el.url) lines.push(`- Page: ${el.url}${el.title ? ` (${el.title})` : ""}`);
  if (el.rect.width > 0 || el.rect.height > 0) {
    lines.push(
      `- Box: ${Math.round(el.rect.width)}×${Math.round(el.rect.height)} at (${Math.round(el.rect.x)}, ${Math.round(el.rect.y)})`,
    );
  }
  if (el.textSnippet) lines.push(`- Text: ${JSON.stringify(el.textSnippet)}`);
  const attrEntries = Object.entries(el.attributes);
  if (attrEntries.length > 0) {
    const formatted = attrEntries.map(([k, v]) => `${k}=${JSON.stringify(v)}`).join(", ");
    lines.push(`- Attributes: ${formatted}`);
  }
  const styleEntries = Object.entries(el.computedStyles);
  if (styleEntries.length > 0) {
    const formatted = styleEntries.map(([k, v]) => `${k}: ${v}`).join("; ");
    lines.push(`- Computed styles: ${formatted}`);
  }
  return lines.join("\n");
}

/** Builds the composer attachments (element context chip + optional cropped PNG) for a picked element. */
export function pickedElementAttachments(el: PickedElement): Attachment[] {
  const label = formatPickedElementChipLabel(el);
  const attachments: Attachment[] = [
    {
      id: localId("element"),
      kind: "file",
      name: label,
      text: formatPickedElementMarkdown(el),
    },
  ];
  if (el.screenshotPng) {
    attachments.push({
      id: localId("element-shot"),
      kind: "image",
      name: `${el.tagName}-preview.png`,
      mimeType: "image/png",
      data: el.screenshotPng,
    });
  }
  return attachments;
}

function parseFrame(raw: unknown): ScreencastFrame | null {
  const o = obj(raw);
  const data = str(o?.data);
  if (!data) return null;
  return {
    data,
    width: typeof o?.width === "number" ? o.width : 1280,
    height: typeof o?.height === "number" ? o.height : 800,
    deviceScaleFactor: typeof o?.deviceScaleFactor === "number" ? o.deviceScaleFactor : 1,
    timestampMs: typeof o?.timestampMs === "number" ? o.timestampMs : Date.now(),
  };
}

function parseViewport(raw: unknown): ViewportPreset {
  return raw === "tablet" || raw === "mobile" ? raw : "desktop";
}

function applyPanelResponse(threadId: string, raw: unknown): boolean {
  const o = obj(raw);
  if (!o) return false;
  const err = str(o.error);
  if (err) {
    patchThread(threadId, { loading: false, picking: false, error: err });
    return false;
  }
  const frame = parseFrame(o.frame);
  const current = currentOf(threadId);
  patchThread(threadId, {
    url: str(o.url) ?? current.url,
    title: str(o.title) ?? current.title,
    canGoBack: typeof o.canGoBack === "boolean" ? o.canGoBack : current.canGoBack,
    canGoForward: typeof o.canGoForward === "boolean" ? o.canGoForward : current.canGoForward,
    viewport: o.viewport ? parseViewport(o.viewport) : current.viewport,
    width: typeof o.width === "number" ? o.width : current.width,
    height: typeof o.height === "number" ? o.height : current.height,
    inspectMode: typeof o.inspectMode === "boolean" ? o.inspectMode : current.inspectMode,
    screencastActive: typeof o.screencastActive === "boolean" ? o.screencastActive : current.screencastActive,
    frame: frame ?? current.frame,
    loading: false,
    error: null,
  });
  return true;
}

/** Handles inbound `_rustcode/browser/frame` notifications from local or remote engines. */
export function onBrowserFrameNotification(params: JsonObject, host = ""): void {
  const rawSessionId = str(params.sessionId);
  if (!rawSessionId) return;
  const key = host ? threadKey(host, rawSessionId) : rawSessionId;
  const frame = parseFrame(params.frame);
  if (!frame) return;
  patchThread(key, {
    frame,
    width: frame.width,
    height: frame.height,
    screencastActive: true,
    error: null,
  });
}

function remoteSessionId(threadId: string): string {
  return splitThreadKey(threadId).id;
}

/** Opens the Preview side panel and navigates the thread's engine Chrome to `url`. */
export async function openBrowserPreview(threadId: string, url: string): Promise<boolean> {
  const normalized = normalizePreviewUrl(url);
  if (!normalized) {
    toast("Enter a valid http:// or https:// URL.", "error");
    return false;
  }
  showPanel("preview");
  const conn = connectionForThread(threadId);
  if (!conn) {
    patchThread(threadId, { error: "The server is not connected.", loading: false });
    return false;
  }
  patchThread(threadId, { loading: true, error: null, url: normalized });
  try {
    const result = await conn.request(
      "_rustcode/browser/open",
      { sessionId: remoteSessionId(threadId), url: normalized },
      45_000,
    );
    return applyPanelResponse(threadId, result);
  } catch (error) {
    patchThread(threadId, {
      loading: false,
      error: error instanceof Error ? error.message : String(error),
    });
    return false;
  }
}

/** Reads the thread browser's current state without launching a new Chrome if none runs. */
export async function refreshBrowserPreviewState(threadId: string): Promise<void> {
  const conn = connectionForThread(threadId);
  if (!conn) return;
  try {
    const result = await conn.request(
      "_rustcode/browser/state",
      { sessionId: remoteSessionId(threadId) },
      15_000,
    );
    applyPanelResponse(threadId, result);
  } catch {
    return;
  }
}

/** Starts or stops live screencast frame streaming for `threadId`. */
export async function setBrowserStreamActive(threadId: string, active: boolean): Promise<void> {
  const conn = connectionForThread(threadId);
  if (!conn) return;
  try {
    const result = await conn.request(
      "_rustcode/browser/stream",
      { sessionId: remoteSessionId(threadId), active },
      20_000,
    );
    applyPanelResponse(threadId, result);
  } catch {
    return;
  }
}

/** Navigates (`goto`, `reload`, `back`, `forward`) in the thread's preview browser. */
export async function navigateBrowserPreview(
  threadId: string,
  action: "goto" | "reload" | "back" | "forward",
  url?: string,
): Promise<boolean> {
  const conn = connectionForThread(threadId);
  if (!conn) return false;
  let targetUrl: string | undefined;
  if (action === "goto") {
    const normalized = normalizePreviewUrl(url ?? "");
    if (!normalized) {
      toast("Enter a valid http:// or https:// URL.", "error");
      return false;
    }
    targetUrl = normalized;
  }
  patchThread(threadId, { loading: true, error: null });
  try {
    const params: JsonObject = {
      sessionId: remoteSessionId(threadId),
      action,
      ...(targetUrl ? { url: targetUrl } : {}),
    };
    const result = await conn.request("_rustcode/browser/navigate", params, 45_000);
    return applyPanelResponse(threadId, result);
  } catch (error) {
    patchThread(threadId, {
      loading: false,
      error: error instanceof Error ? error.message : String(error),
    });
    return false;
  }
}

/** Switches the responsive device viewport preset (`desktop`, `tablet`, `mobile`). */
export async function setBrowserViewport(threadId: string, preset: ViewportPreset): Promise<void> {
  const spec = VIEWPORT_PRESETS[preset];
  patchThread(threadId, { viewport: preset, width: spec.width, height: spec.height });
  const conn = connectionForThread(threadId);
  if (!conn) return;
  try {
    const result = await conn.request(
      "_rustcode/browser/viewport",
      { sessionId: remoteSessionId(threadId), preset, width: spec.width, height: spec.height },
      20_000,
    );
    applyPanelResponse(threadId, result);
  } catch (error) {
    patchThread(threadId, {
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

/** Forwards a mouse, wheel, keyboard, or text event to the thread's Chrome. */
export function sendBrowserInput(threadId: string, event: BrowserInputEvent): void {
  const conn = connectionForThread(threadId);
  if (!conn) return;
  void conn
    .request(
      "_rustcode/browser/input",
      { sessionId: remoteSessionId(threadId), event: event as unknown as Json },
      10_000,
    )
    .catch(() => undefined);
}

/** Toggles CDP `Overlay.setInspectMode` element highlighting. */
export async function setBrowserInspectMode(threadId: string, enabled: boolean): Promise<void> {
  patchThread(threadId, { inspectMode: enabled });
  const conn = connectionForThread(threadId);
  if (!conn) return;
  try {
    const result = await conn.request(
      "_rustcode/browser/inspect",
      { sessionId: remoteSessionId(threadId), enabled },
      15_000,
    );
    applyPanelResponse(threadId, result);
  } catch {
    patchThread(threadId, { inspectMode: false });
  }
}

/** Attaches a sanitized picked element and its cropped screenshot to the thread's composer. */
export function attachPickedElementToComposer(threadId: string, element: PickedElement): void {
  const added = pickedElementAttachments(element);
  setExtras(threadId, (extras) => ({
    ...extras,
    attachments: [...extras.attachments, ...added],
  }));
}

/** Picks the DOM element at `(x, y)` and attaches its sanitized bundle + cropped PNG to the composer. */
export async function pickBrowserElement(
  threadId: string,
  x: number,
  y: number,
): Promise<PickedElement | null> {
  const conn = connectionForThread(threadId);
  if (!conn) return null;
  patchThread(threadId, { picking: true, inspectMode: false, error: null });
  try {
    const raw = obj(
      await conn.request("_rustcode/browser/pick", { sessionId: remoteSessionId(threadId), x, y }, 20_000),
    );
    const err = str(raw?.error);
    if (err || !raw || !str(raw.tagName)) {
      patchThread(threadId, { picking: false, error: err ?? "Could not inspect that element." });
      return null;
    }
    const element: PickedElement = {
      tagName: str(raw.tagName) ?? "div",
      selector: str(raw.selector) ?? "",
      accessibleName: str(raw.accessibleName) ?? null,
      role: str(raw.role) ?? null,
      reactComponents: Array.isArray(raw.reactComponents)
        ? raw.reactComponents.filter((v): v is string => typeof v === "string")
        : [],
      textSnippet: str(raw.textSnippet) ?? "",
      attributes:
        obj(raw.attributes) !== undefined
          ? Object.fromEntries(
              Object.entries(obj(raw.attributes)!).filter(
                (entry): entry is [string, string] => typeof entry[1] === "string",
              ),
            )
          : {},
      computedStyles:
        obj(raw.computedStyles) !== undefined
          ? Object.fromEntries(
              Object.entries(obj(raw.computedStyles)!).filter(
                (entry): entry is [string, string] => typeof entry[1] === "string",
              ),
            )
          : {},
      rect: {
        x: typeof obj(raw.rect)?.x === "number" ? (obj(raw.rect)!.x as number) : 0,
        y: typeof obj(raw.rect)?.y === "number" ? (obj(raw.rect)!.y as number) : 0,
        width: typeof obj(raw.rect)?.width === "number" ? (obj(raw.rect)!.width as number) : 0,
        height: typeof obj(raw.rect)?.height === "number" ? (obj(raw.rect)!.height as number) : 0,
      },
      url: str(raw.url) ?? "",
      title: str(raw.title) ?? "",
      screenshotPng: str(raw.screenshotPng) ?? null,
    };
    attachPickedElementToComposer(threadId, element);
    patchThread(threadId, { picking: false, inspectMode: false });
    toast(`Attached ${formatPickedElementChipLabel(element)} to composer`, "success");
    return element;
  } catch (error) {
    patchThread(threadId, {
      picking: false,
      error: error instanceof Error ? error.message : String(error),
    });
    return null;
  }
}
