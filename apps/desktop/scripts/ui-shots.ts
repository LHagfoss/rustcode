/**
 * `bun scripts/ui-shots.ts capture <url> <dir>` photographs fixed views of the
 * mock build served at `<url>` (light and dark) into `<dir>/<view>.png`.
 * `bun scripts/ui-shots.ts compare <before> <after> <imageBase>` prints the
 * pull request comment for the views whose pictures differ, linking each pair
 * under `<imageBase>/{before,after}/`, and deletes the pictures that did not
 * change so only the pairs worth publishing are left. CI runs both on UI pull requests
 * (`.github/workflows/ui-screenshots.yml`), the base build as "before".
 */

import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { chromium, type Page } from "@playwright/test";
import { changedShots, commentFor, countDiffering, TOLERANCE } from "./uiShots";

const CHROME_PATHS = [
  "/usr/bin/google-chrome-stable",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
];

function chromePath(): string | undefined {
  return process.env.E2E_CHROME || CHROME_PATHS.find((path) => existsSync(path));
}

type View = { name: string; title: string; open: (page: Page) => Promise<void> };

const VIEWS: View[] = [
  { name: "home", title: "New thread", open: async () => {} },
  {
    name: "thread",
    title: "Thread",
    open: async (page) => {
      await page.locator("aside").getByText("Bump the bun lockfile").click();
      await page.clock.runFor(2_000);
      await page.locator("main").getByText("Worked for 9s").waitFor();
    },
  },
  {
    name: "palette",
    title: "Command palette",
    open: async (page) => {
      await page.getByRole("button", { name: "Search threads", exact: true }).click();
      await page.clock.runFor(1_000);
      await page.getByLabel("Search threads and actions").waitFor();
    },
  },
  {
    name: "settings",
    title: "Settings",
    open: async (page) => {
      await page.getByRole("button", { name: "Settings", exact: true }).click();
      await page.clock.runFor(1_000);
      await page.getByRole("navigation", { name: "Settings sections" }).waitFor();
    },
  },
];

const SCHEMES = ["dark", "light"] as const;
/** One instant and one random sequence for every run, so only a UI change moves a pixel. */
const FIXED_TIME = new Date("2026-01-15T12:00:00Z");

async function capture(url: string, dir: string) {
  mkdirSync(dir, { recursive: true });
  const browser = await chromium.launch({ executablePath: chromePath() });
  let failed = 0;
  try {
    for (const colorScheme of SCHEMES) {
      for (const view of VIEWS) {
        const context = await browser.newContext({
          viewport: { width: 1512, height: 945 },
          deviceScaleFactor: 1,
          colorScheme,
          reducedMotion: "reduce",
        });
        await context.addInitScript(() => {
          let seed = 42;
          Math.random = () => {
            seed = (seed * 16807) % 2147483647;
            return (seed - 1) / 2147483646;
          };
        });
        const page = await context.newPage();
        await page.clock.install({ time: FIXED_TIME });
        try {
          await page.goto(url);
          await page.clock.runFor(5_000);
          await page.getByRole("textbox", { name: "Message" }).waitFor({ timeout: 20_000 });
          await view.open(page);
          await page.clock.runFor(5_000);
          await page.screenshot({
            path: join(dir, `${view.name}-${colorScheme}.png`),
            animations: "disabled",
          });
        } catch (error) {
          failed += 1;
          console.error(`${view.name}-${colorScheme}: ${(error as Error).message.split("\n")[0]}`);
        } finally {
          await context.close();
        }
      }
    }
  } finally {
    await browser.close();
  }
  console.log(`captured ${VIEWS.length * SCHEMES.length - failed} views in ${dir}`);
}

/** Decodes both PNGs in Chrome and counts the pixels that moved past `TOLERANCE`. */
async function pixelCounter(page: Page) {
  await page.addScriptTag({ content: `window.countDiffering = ${countDiffering.toString()};` });
  return (a: Buffer, b: Buffer) =>
    page.evaluate(
      async ({ a, b, tolerance }) => {
        const pixels = async (base64: string) => {
          const blob = await (await fetch(`data:image/png;base64,${base64}`)).blob();
          const bitmap = await createImageBitmap(blob);
          const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
          const context = canvas.getContext("2d") as OffscreenCanvasRenderingContext2D;
          context.drawImage(bitmap, 0, 0);
          return context.getImageData(0, 0, bitmap.width, bitmap.height).data;
        };
        const count = (window as unknown as { countDiffering: typeof countDiffering }).countDiffering;
        return count(await pixels(a), await pixels(b), tolerance);
      },
      { a: a.toString("base64"), b: b.toString("base64"), tolerance: TOLERANCE },
    );
}

function shots(dir: string): Map<string, Buffer> {
  if (!existsSync(dir)) return new Map();
  return new Map(
    readdirSync(dir)
      .filter((file) => file.endsWith(".png"))
      .sort()
      .map((file) => [file, readFileSync(join(dir, file))]),
  );
}

if (import.meta.main) {
  const [command, ...rest] = process.argv.slice(2);
  if (command === "capture" && rest.length === 2) {
    await capture(rest[0] as string, rest[1] as string);
  } else if (command === "compare" && rest.length === 3) {
    const before = shots(rest[0] as string);
    const after = shots(rest[1] as string);
    const total = new Set([...before.keys(), ...after.keys()]).size;
    const browser = await chromium.launch({ executablePath: chromePath() });
    const page = await browser.newPage();
    const changed = await changedShots(before, after, await pixelCounter(page));
    await browser.close();
    for (const [dir, pictures] of [
      [rest[0], before],
      [rest[1], after],
    ] as const) {
      for (const file of pictures.keys()) {
        if (!changed.includes(file)) rmSync(join(dir as string, file));
      }
    }
    process.stdout.write(commentFor(changed, total, rest[2] as string));
  } else {
    console.error(
      "usage: ui-shots.ts capture <url> <dir> | compare <before-dir> <after-dir> <image-base-url>",
    );
    process.exit(2);
  }
}
