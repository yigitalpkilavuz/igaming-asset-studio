import { test, expect } from "@playwright/test";

test.describe("MotionLoop (headless render)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/harness?c=MotionLoop&fixture=lightning");
    await expect(page.locator(".motion-loop")).toBeVisible();
  });

  test("the source toggle offers both AI and Video lanes", async ({ page }) => {
    const segs = page.locator(".src-toggle .seg");
    await expect(segs).toHaveCount(2);
    await expect(segs.first()).toContainText("animate still");
    await expect(segs.last()).toContainText("Video file");
    // Default AI lane, no SpriteCook key in the fixture → the key-needed hint (not the form).
    await expect(page.locator(".motion-loop")).toContainText("SpriteCook key");
    await page.locator(".motion-loop").screenshot({ path: "tests/visual/__artifacts__/motionloop.png" });
  });

  test("the Video lane reveals the clip → sheet controls", async ({ page }) => {
    await page.locator(".src-toggle .seg", { hasText: "Video file" }).click();
    await expect(page.locator(".file-pick")).toContainText("Choose a video");
    // Background-matte + loop-mode + frame-count selects, and the bake action.
    await expect(page.locator("select").first()).toContainText("magenta bg → key");
    await expect(page.locator("select").nth(1)).toContainText("ping-pong loop");
    await expect(page.getByRole("button", { name: /Bake loop/i })).toBeVisible();
    // No file chosen yet → baking is disabled.
    await expect(page.getByRole("button", { name: /Bake loop/i })).toBeDisabled();
    await page.locator(".motion-loop").screenshot({ path: "tests/visual/__artifacts__/motionloop-video.png" });
  });
});
