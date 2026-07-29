import { test, expect, type Locator } from "@playwright/test";

const box = async (l: Locator) => {
  const b = await l.boundingBox();
  expect(b, "element has a layout box").not.toBeNull();
  return b!;
};

test.describe("GamePreview (headless render)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/harness?c=GamePreview&fixture=lightning");
    await expect(page.locator(".harness-root .stage")).toBeVisible();
    // wait for the async data load → symbols painted
    await expect(page.locator(".cell img").first()).toBeVisible();
  });

  test("symbols render at non-zero size, one per cell (the vanish regression)", async ({ page }) => {
    const syms = page.locator(".cell img");
    expect(await syms.count()).toBe(25); // 5×5
    for (let i = 0; i < 25; i++) {
      const b = await box(syms.nth(i));
      expect(b.width, `cell ${i} img width`).toBeGreaterThan(4);
      expect(b.height, `cell ${i} img height`).toBeGreaterThan(4);
    }
  });

  test("each symbol is centered in its cell (flex, not top-left drift)", async ({ page }) => {
    const cells = page.locator(".cell");
    for (let i = 0; i < 5; i++) {
      const cell = await box(cells.nth(i));
      const img = await box(cells.nth(i).locator("img"));
      const dx = Math.abs(img.x + img.width / 2 - (cell.x + cell.width / 2));
      const dy = Math.abs(img.y + img.height / 2 - (cell.y + cell.height / 2));
      expect(dx, `cell ${i} horizontal centering`).toBeLessThan(cell.width * 0.12);
      expect(dy, `cell ${i} vertical centering`).toBeLessThan(cell.height * 0.12);
    }
  });

  test("the board is centered on the stage", async ({ page }) => {
    const stage = await box(page.locator(".harness-root .stage"));
    const first = await box(page.locator(".cell").first()); // top-left cell
    const last = await box(page.locator(".cell").last()); // bottom-right cell
    const boardCx = (first.x + last.x + last.width) / 2;
    const boardCy = (first.y + last.y + last.height) / 2;
    expect(Math.abs(boardCx - (stage.x + stage.width / 2))).toBeLessThan(stage.width * 0.04);
    expect(Math.abs(boardCy - (stage.y + stage.height / 2))).toBeLessThan(stage.height * 0.06);
  });

  test("the scene layer stack composites (more than one background layer)", async ({ page }) => {
    const layers = page.locator(".scene-cover, .scene-sprite");
    expect(await layers.count()).toBeGreaterThan(1);
    await expect(layers.first()).toBeVisible();
  });

  test("the Layers panel lists all scene layers and toggles one off", async ({ page }) => {
    await page.getByRole("button", { name: /Layers/ }).click();
    const pop = page.locator(".layers-pop");
    await expect(pop).toBeVisible();
    // includes the not-generated layer (greyed / disabled checkbox)
    await expect(pop.locator(".layer-row.missing")).toHaveCount(1);

    const before = await page.locator(".scene-cover, .scene-sprite").count();
    await pop.locator(".layer-row input:enabled").first().uncheck();
    await expect(page.locator(".scene-cover, .scene-sprite")).toHaveCount(before - 1);
  });

  test("save a screenshot artifact for review", async ({ page }) => {
    await page.waitForTimeout(150); // let images decode
    // Artifact only (not a pixel-diff baseline — SVG/font rendering varies across machines). The
    // assertions above are the environment-independent regression guard.
    await page.locator(".harness-root .stage").screenshot({ path: "tests/visual/__artifacts__/preview-lightning.png" });
  });
});
