import { test, expect } from "@playwright/test";

test.describe("SoundStudio (headless render)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/harness?c=SoundStudio&fixture=audiobench");
    await expect(page.locator(".sound")).toBeVisible();
  });

  test("shows the cue rail, the selected cue's settings, and the embedded bench", async ({ page }) => {
    // Grouped rail with the seeded cue + add buttons.
    await expect(page.locator(".rail")).toContainText("Music");
    await expect(page.locator(".rail")).toContainText("Sound effects");
    await expect(page.locator(".rail .add")).toHaveCount(2);
    await expect(page.locator(".rail .cue")).toContainText("Base-game music");

    // The selected cue's settings row (editable name, kind, loop, gain, secs, remove).
    await expect(page.locator(".cue-settings .cue-name")).toHaveValue("Base-game music");
    await expect(page.locator(".cue-settings")).toContainText("loop");
    await expect(page.getByRole("button", { name: "Remove" })).toBeVisible();

    // The AudioBench is embedded for the selected cue.
    await expect(page.locator(".detail .audio-bench")).toBeVisible();
    await expect(page.locator(".detail .take")).toHaveCount(1);

    await page.locator(".sound").screenshot({ path: "tests/visual/__artifacts__/soundstudio.png" });
  });

  test("the audio style master is editable in the top bar", async ({ page }) => {
    await expect(page.locator(".topbar .style")).toHaveValue("dark baroque orchestral");
  });
});
