import { existsSync } from "node:fs";
import { defineConfig } from "@playwright/test";

const CHROME_PATHS = [
  "/usr/bin/google-chrome-stable",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
];
const executablePath = process.env.E2E_CHROME || CHROME_PATHS.find((path) => existsSync(path));
const port = Number(process.env.E2E_PORT) || 4174;

export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["list"], ["github"]] : "list",
  timeout: 30_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    viewport: { width: 1512, height: 945 },
    launchOptions: { executablePath },
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  webServer: {
    command: `bunx vite build --outDir dist-e2e --emptyOutDir --logLevel warn && bunx vite preview --outDir dist-e2e --host 127.0.0.1 --port ${port} --strictPort`,
    env: { VITE_TRANSPORT: "mock" },
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
