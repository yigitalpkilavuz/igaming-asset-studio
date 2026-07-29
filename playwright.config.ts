import { defineConfig, devices } from "@playwright/test";

/**
 * Headless component-render tests. WebKit ONLY — the app runs in a Tauri WKWebView, so WebKit is
 * the engine that reproduces its layout bugs (e.g. the img-in-grid vanish). Chrome/jsdom would
 * pass while the real app breaks. Starts `vite dev` (the same 1420 the app uses) and drives the
 * client-only /harness route with a mocked backend.
 */
export default defineConfig({
  testDir: "tests/visual",
  timeout: 30_000,
  expect: { timeout: 5_000 },
  fullyParallel: true,
  reporter: [["list"]],
  use: {
    baseURL: "http://localhost:1420",
    screenshot: "only-on-failure",
  },
  // WebKit is the accurate engine (the app runs in a WKWebView). Chromium is a fallback guard for
  // machines where the Playwright WebKit build won't launch — it still catches most regressions,
  // just not WebKit-specific ones. Pick with `--project=webkit` / `--project=chromium`.
  projects: [
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
  ],
  webServer: {
    command: "pnpm dev",
    url: "http://localhost:1420/harness?c=GamePreview",
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
