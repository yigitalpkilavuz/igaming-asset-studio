import { test, expect } from "@playwright/test";

test.describe("BlueprintModal (headless render)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/harness?c=BlueprintModal&fixture=lightning");
    await expect(page.locator(".sheet")).toBeVisible();
  });

  test("shell: head, tabs (incl. AI Port), and the create footer", async ({ page }) => {
    await expect(page.locator(".head .title")).toContainText("Babewyn Court");
    await expect(page.locator(".head .derived")).toContainText("33");
    // Identity/Mechanics/Symbols/Scenes + the pinned AI Port.
    await expect(page.locator(".tabs .tab")).toHaveCount(5);
    await expect(page.locator(".tabs .tab").last()).toContainText("AI Port");
    // New game → "Create game" primary action.
    await expect(page.locator(".foot .gold")).toContainText("Create game");
    await page.locator(".sheet").screenshot({ path: "tests/visual/__artifacts__/blueprintmodal.png" });
  });

  test("the AI Port tab renders the copy/paste flow", async ({ page }) => {
    await page.locator(".tabs .tab", { hasText: "AI Port" }).click();
    await expect(page.locator(".porter")).toBeVisible();
    await expect(page.getByRole("button", { name: /Copy prompt/i })).toBeVisible();
    await expect(page.locator(".porter textarea")).toBeVisible();
    await page.locator(".sheet").screenshot({ path: "tests/visual/__artifacts__/blueprintmodal-port.png" });
  });

  test("the Mechanics tab groups the toggles into labelled clusters", async ({ page }) => {
    await page.locator(".tabs .tab", { hasText: "Mechanics" }).click();
    await expect(page.locator(".tgroup")).toHaveCount(2);
    await expect(page.locator(".tg-label").first()).toContainText("Features");
    await page.locator(".sheet").screenshot({ path: "tests/visual/__artifacts__/blueprintmodal-mech.png" });
  });

  test("the Symbols tab backfills the tuned fit/tone defaults", async ({ page }) => {
    await page.locator(".tabs .tab", { hasText: "Symbols" }).click();
    // The fixture config omits symbolSizing → ConfigForm backfills from $lib/symbolDefaults.
    // Grid inputs are flat: [low ink,h,tlo,thi, high…, wild…, scatter…]. Confirm the retuned
    // values: low ink 21 (index 0), scatter ink 36 (index 12).
    await expect(page.locator(".fit-grid input").nth(0)).toHaveValue("21");
    await expect(page.locator(".fit-grid input").nth(12)).toHaveValue("36");
    await page.locator(".sheet").screenshot({ path: "tests/visual/__artifacts__/blueprintmodal-symbols.png" });
  });
});
