import { test, expect } from "@playwright/test";

test.describe("FontStudio (headless render)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/harness?c=FontStudio&fixture=fonts");
    await expect(page.locator(".fonts")).toBeVisible();
  });

  test("lists the fonts, shows the selected editor + live preview", async ({ page }) => {
    // Rail lists the seeded fonts with swatches + an add button.
    await expect(page.locator(".rail .font")).toHaveCount(2);
    await expect(page.locator(".rail")).toContainText("Gold win");
    await expect(page.locator(".rail .add")).toBeVisible();

    // The selected font's editor: typeface picker, size, fill/outline colors.
    await expect(page.locator(".settings")).toContainText("Typeface");
    await expect(page.locator(".settings select")).toBeVisible();
    await expect(page.locator('.settings input[type="color"]')).toHaveCount(3); // fill top/bottom + outline
    await expect(page.getByRole("button", { name: "Remove font" })).toBeVisible();

    // The live preview <img> is rendered (mock returns a PNG data URL).
    await expect(page.locator(".preview .prev-img")).toBeAttached();
    // The preview sample box is editable.
    await expect(page.locator(".topbar .sample")).toBeVisible();

    await page.locator(".fonts").screenshot({ path: "tests/visual/__artifacts__/fontstudio.png" });
  });
});
