import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await page.goto("/");
});

async function crashEngine(page: Page) {
  await page.evaluate(() =>
    (globalThis as unknown as { __mockEngine: { crash(): void } }).__mockEngine.crash(),
  );
}

async function send(page: Page, text: string) {
  const composer = page.getByRole("textbox", { name: "Message" });
  await composer.fill(text);
  await composer.press("Enter");
}

const APPROVAL = "Allow edit src/app.ts?";
const DONE = "Done. I updated the greeting in src/app.ts";

test("a turn cut off by a crash continues once and its approval still works", async ({ page }) => {
  const main = page.locator("main");
  await send(page, "Crash during approval");
  await expect(main.getByText(APPROVAL)).toBeVisible();

  await crashEngine(page);

  await expect(main.getByText("Continued after a restart")).toHaveCount(1);
  await expect(main.getByText(APPROVAL)).toHaveCount(1);
  await main.getByRole("button", { name: /^Approve/ }).click();
  await expect(main.getByText(DONE)).toBeVisible();
  await expect(main.getByText(APPROVAL)).toHaveCount(0);
  await expect(main.getByText("Continued after a restart")).toHaveCount(1);
});

test("a message sent after a crash runs on the restarted engine", async ({ page }) => {
  const main = page.locator("main");
  await crashEngine(page);
  await expect(page.locator("aside").getByText("Bump the bun lockfile")).toBeVisible();

  await send(page, "After the crash");
  await expect(main.getByText(APPROVAL)).toBeVisible();
  await main.getByRole("button", { name: /^Approve/ }).click();
  await expect(main.getByText(DONE)).toBeVisible();
});

test("the server cannot be restarted from the palette while a turn runs", async ({ page }) => {
  await send(page, "Restart during approval");
  await expect(page.locator("main").getByText(APPROVAL)).toBeVisible();
  await page.getByRole("button", { name: "Search threads", exact: true }).click();
  await page.getByLabel("Search threads and actions").fill("Restart server");
  const palette = page.getByRole("dialog");
  await expect(palette.getByText("A turn is running")).toBeVisible();
  await page.getByLabel("Search threads and actions").press("Enter");
  await expect(page.getByText("Restarting the server…")).toHaveCount(0);
  await expect(page.locator("main").getByText(APPROVAL)).toBeVisible();
});

test("a thread reopened after a crash shows its transcript once", async ({ page }) => {
  const thread = page.locator("aside").getByText("Bump the bun lockfile");
  await thread.click();
  const main = page.locator("main");
  await expect(main.getByText("Worked for 9s")).toHaveCount(1);

  await crashEngine(page);
  await page.locator("aside").getByText("Tidy the pricing page copy").click();
  await thread.click();

  await expect(main.getByText("Worked for 9s")).toHaveCount(1);
});
