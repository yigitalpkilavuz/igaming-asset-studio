import { test, expect } from "@playwright/test";

test.describe("SettingsModal (headless render)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/harness?c=SettingsModal&fixture=settings");
    await expect(page.locator(".sheet")).toBeVisible();
  });

  test("renders the provider overview, grouped cards, and status pills", async ({ page }) => {
    // Overview chips reflect readiness (configured → no "set up ↓", not configured → chip is a button).
    await expect(page.locator(".overview .chip")).toHaveCount(5);
    await expect(page.locator(".overview")).toContainText("set up ↓");

    // Grouped labels.
    await expect(page.locator(".group-label")).toContainText(["Image generation", "Audio", "Application"]);

    // Every provider card carries a status pill; configured keys read "Ready".
    await expect(page.locator("#set-openai .pill")).toHaveText("Ready");
    await expect(page.locator("#set-gemini .pill")).toHaveText("Ready");
    await expect(page.locator("#set-spritecook .pill")).toHaveText("Not set");

    // Audio card holds both sub-providers with their own pills.
    await expect(page.locator("#set-audio .pill")).toHaveCount(2);

    // Model fields auto-save on change (no per-field Save button clutter).
    await expect(page.locator("#set-openai .f input").first()).toHaveValue("gpt-image-2");

    await page.locator(".sheet").screenshot({ path: "tests/visual/__artifacts__/settingsmodal.png" });
  });

  test("light theme also renders cleanly", async ({ page }) => {
    await page.locator(".seg button", { hasText: "Light" }).click();
    await page.locator(".sheet").screenshot({ path: "tests/visual/__artifacts__/settingsmodal-light.png" });
  });
});
