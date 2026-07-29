import { test, expect } from "@playwright/test";

test.describe("AudioBench (headless render)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/harness?c=AudioBench&fixture=audiobench");
    await expect(page.locator(".audio-bench")).toBeVisible();
  });

  test("renders provider picker, prompt, and the processed take with player + report", async ({ page }) => {
    await expect(page.locator("select.dd")).toBeVisible(); // provider picker
    await expect(page.locator("select.dd option", { hasText: "Stable Audio" })).toBeAttached();
    await expect(page.getByRole("button", { name: /Generate/ })).toBeVisible();
    await expect(page.locator("textarea.prompt")).toBeVisible();

    // The seeded (active, processed) take shows a player + its loudness report.
    await expect(page.locator(".take")).toHaveCount(1);
    await expect(page.locator(".take .player")).toBeAttached();
    await expect(page.locator(".take .report")).toContainText("LUFS");
    await expect(page.locator(".take.active")).toHaveCount(1);
    await page.locator(".audio-bench").screenshot({ path: "tests/visual/__artifacts__/audiobench.png" });
  });

  test("Generate keeps a take (mock returns the record)", async ({ page }) => {
    await page.getByRole("button", { name: /Generate/ }).click();
    await expect(page.locator(".take")).toHaveCount(1);
    await expect(page.locator(".msg")).toBeVisible();
  });
});
