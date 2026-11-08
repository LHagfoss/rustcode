import { expect, test } from "bun:test";
import { applyMaterial, materialFrom } from "./material";

test("only the shell's mica turns the material on", () => {
  expect(materialFrom("mica")).toBe("mica");
  expect(materialFrom("none")).toBe("none");
  expect(materialFrom("acrylic")).toBe("none");
  expect(materialFrom(undefined)).toBe("none");
});

test("outside the app there is no material to ask for", async () => {
  expect(await applyMaterial()).toBe("none");
});
