import { test as base, expect } from "@playwright/test";

export const test = base.extend<{ pageErrors: Error[] }>({
  pageErrors: [
    async ({ page }, use) => {
      const errors: Error[] = [];
      page.on("pageerror", (error) => errors.push(error));
      await use(errors);
      expect(errors.map((error) => error.message)).toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };
