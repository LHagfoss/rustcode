import { expect, test } from "bun:test";
import { reducesTransparency } from "./transparency";

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
