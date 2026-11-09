import { expect, test } from "bun:test";
import { canBlur, reducesTransparency } from "./transparency";

const none = { user: false, system: false, media: false, blur: true };

test("glass stays glass until something asks for solid", () => {
  expect(reducesTransparency(none)).toBe(false);
});

test("the user's switch, the system, the media query or a web view with no blur each make it solid", () => {
  expect(reducesTransparency({ ...none, user: true })).toBe(true);
  expect(reducesTransparency({ ...none, system: true })).toBe(true);
  expect(reducesTransparency({ ...none, media: true })).toBe(true);
  expect(reducesTransparency({ ...none, blur: false })).toBe(true);
});

test("the glass blurs only where the web view has the filter and composites it", () => {
  // macOS and WebView2: the filter and the layer it is drawn on.
  expect(canBlur(true, true)).toBe(true);
  // Linux with WEBKIT_DISABLE_COMPOSITING_MODE: `CSS.supports` still says yes, nothing is blurred.
  expect(canBlur(true, false)).toBe(false);
  // A web view without the property at all.
  expect(canBlur(false, true)).toBe(false);
  expect(canBlur(false, false)).toBe(false);
});
