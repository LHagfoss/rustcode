import { beforeEach, describe, expect, test } from "bun:test";
import { MockTransport } from "../acp/mock";
import {
  attachPickedElementToComposer,
  formatPickedElementChipLabel,
  formatPickedElementMarkdown,
  normalizePreviewUrl,
  onBrowserFrameNotification,
  openBrowserPreview,
  type PickedElement,
  pickBrowserElement,
  pickedElementAttachments,
  resetBrowserPreviewForTests,
  scalePointerCoords,
  setBrowserViewport,
  useBrowserPreview,
} from "./browserPreview";
import { connect, resetForTests, useApp } from "./store";

beforeEach(() => {
  resetForTests();
  resetBrowserPreviewForTests();
});

describe("browserPreview helpers", () => {
  test("normalizes URLs and rejects unsafe schemes or credentials", () => {
    expect(normalizePreviewUrl("localhost:5173")).toBe("http://localhost:5173/");
    expect(normalizePreviewUrl("127.0.0.1:3000/login")).toBe("http://127.0.0.1:3000/login");
    expect(normalizePreviewUrl("example.com/docs")).toBe("https://example.com/docs");
    expect(normalizePreviewUrl("javascript:alert(1)")).toBeNull();
    expect(normalizePreviewUrl("file:///etc/passwd")).toBeNull();
    expect(normalizePreviewUrl("http://user:secret@localhost:3000")).toBeNull();
    expect(normalizePreviewUrl("   ")).toBeNull();
  });

  test("scales pointer coordinates cleanly into viewport dimensions", () => {
    const rect = { left: 100, top: 50, width: 400, height: 250 };
    expect(scalePointerCoords(300, 175, rect, 1280, 800)).toEqual({ x: 640, y: 400 });
    expect(scalePointerCoords(0, 0, rect, 1280, 800)).toEqual({ x: 0, y: 0 });
    expect(scalePointerCoords(900, 900, rect, 1280, 800)).toEqual({ x: 1280, y: 800 });
  });

  test("formats picked element bundle and composer attachments", () => {
    const el: PickedElement = {
      tagName: "button",
      selector: 'button[data-testid="submit"]',
      accessibleName: "Submit order",
      role: "button",
      reactComponents: ["CheckoutButton", "CartFooter"],
      textSnippet: "Submit order",
      attributes: { "data-testid": "submit", "aria-label": "[redacted]" },
      computedStyles: { display: "flex", borderRadius: "8px" },
      rect: { x: 12, y: 24, width: 120, height: 36 },
      url: "http://localhost:5173/cart",
      title: "Cart",
      screenshotPng: "iVBORw0KGgo=",
    };

    expect(formatPickedElementChipLabel(el)).toBe('CheckoutButton <button> "Submit order"');
    const md = formatPickedElementMarkdown(el);
    expect(md).toContain("React stack: CheckoutButton < CartFooter");
    expect(md).toContain('role="button"');
    expect(md).toContain('aria-label="[redacted]"');
    expect(md).toContain("borderRadius: 8px");

    const attachments = pickedElementAttachments(el);
    expect(attachments).toHaveLength(2);
    expect(attachments[0]?.kind).toBe("file");
    expect(attachments[1]?.kind).toBe("image");

    attachPickedElementToComposer("t-1", el);
    expect(useApp.getState().extras["t-1"]?.attachments).toHaveLength(2);
  });

  test("routes inbound frame notifications for both local and remote threads", () => {
    onBrowserFrameNotification({
      sessionId: "local-1",
      frame: { data: "abc", width: 1280, height: 800, deviceScaleFactor: 1, timestampMs: 10 },
    });
    expect(useBrowserPreview.getState().byThread["local-1"]?.frame?.data).toBe("abc");

    onBrowserFrameNotification(
      {
        sessionId: "remote-1",
        frame: { data: "xyz", width: 390, height: 844, deviceScaleFactor: 2, timestampMs: 20 },
      },
      "studio",
    );
    expect(useBrowserPreview.getState().byThread["studio/remote-1"]?.frame?.data).toBe("xyz");
    expect(useBrowserPreview.getState().byThread["studio/remote-1"]?.width).toBe(390);
  });
});

describe("browserPreview ACP flow", () => {
  test("opens preview, switches viewport, and picks an element into the composer", async () => {
    await connect(new MockTransport());
    const ok = await openBrowserPreview("t-preview", "localhost:5173");
    expect(ok).toBe(true);
    expect(useApp.getState().panel).toBe("preview");
    expect(useBrowserPreview.getState().byThread["t-preview"]?.url).toBe("http://localhost:5173/");
    expect(useBrowserPreview.getState().byThread["t-preview"]?.frame?.data).toBeTruthy();

    await setBrowserViewport("t-preview", "mobile");
    expect(useBrowserPreview.getState().byThread["t-preview"]?.viewport).toBe("mobile");
    expect(useBrowserPreview.getState().byThread["t-preview"]?.width).toBe(390);

    const picked = await pickBrowserElement("t-preview", 120, 80);
    expect(picked?.tagName).toBe("button");
    expect(picked?.reactComponents).toContain("CheckoutButton");
    expect(useApp.getState().extras["t-preview"]?.attachments.length).toBe(2);
  });
});
