import { expect, test } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await page.goto("/");
});

test("boots to an empty composer and the thread list", async ({ page }) => {
  await expect(page).toHaveTitle("RustCode");
  await expect(page.getByRole("textbox", { name: "Message" })).toBeVisible();
  await expect(page.getByRole("heading", { name: /What should we build in/ })).toBeVisible();
  await expect(page.locator("aside").getByText("Bump the bun lockfile")).toBeVisible();
});

test("sends a message, approves the edit and finishes the turn", async ({ page }) => {
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill("Hello from e2e");
  await composer.press("Enter");

  const main = page.locator("main");
  await expect(main.getByText("Allow edit src/app.ts?")).toBeVisible();
  await expect(page.locator("aside").getByText("Hello from e2e")).toBeVisible();

  await main.getByRole("button", { name: /^Approve/ }).click();
  await expect(main.getByText("Done. I updated the greeting in src/app.ts")).toBeVisible();
  await expect(main.getByText("Allow edit src/app.ts?")).toHaveCount(0);
});

test("opens a thread from the sidebar", async ({ page }) => {
  await page.locator("aside").getByText("Bump the bun lockfile").click();
  const main = page.locator("main");
  await expect(main.getByText("Worked for 9s")).toBeVisible();
  await expect(main.getByRole("heading", { name: /What should we build in/ })).toHaveCount(0);
});

test("finds a thread with the command palette", async ({ page }) => {
  await page.getByRole("button", { name: "Search threads", exact: true }).click();
  const search = page.getByLabel("Search threads and actions");
  await search.fill("pricing");
  const palette = page.getByRole("dialog");
  await expect(palette.getByText("Tidy the pricing page copy")).toBeVisible();
  await expect(palette.getByText("Bump the bun lockfile")).toHaveCount(0);

  await search.press("Enter");
  await expect(palette).toHaveCount(0);
  await expect(page.locator("main").getByText("Tidy the pricing page copy").first()).toBeVisible();
});

test("opens Settings", async ({ page }) => {
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByRole("navigation", { name: "Settings sections" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Notifications" })).toBeVisible();
});
