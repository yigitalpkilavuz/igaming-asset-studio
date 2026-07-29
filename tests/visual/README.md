# Headless component render harness

Renders app components in a **real WebKit engine** (Playwright) with a **mocked Tauri backend**, so
regressions in layout/rendering are caught automatically — without opening the desktop app.

WebKit specifically, because the app ships in a Tauri **WKWebView**: the bugs we actually hit
(e.g. a symbol `<img>` collapsing inside a CSS grid cell) reproduce in WebKit but *pass* in
Chrome/jsdom.

## Run

```bash
pnpm exec playwright install chromium webkit   # one-time: download browser builds
pnpm test:visual                               # Chromium (runnable everywhere)
pnpm test:visual:webkit                        # WebKit — the accurate WKWebView engine
pnpm test:visual -- --headed                   # watch it in a real window
```

`test:visual*` reuses a running dev server on :1420 if one is up (e.g. `tauri dev`), else starts
`vite dev` itself.

**WebKit is the accurate engine** (the app ships in a WKWebView), but Playwright's WebKit build
currently **segfaults on launch under macOS 26** — so `test:visual` defaults to **Chromium**, which
runs everywhere and catches most regressions (element counts, sizes, centering, layer toggles),
just not WebKit-specific layout quirks. Run `test:visual:webkit` on any machine where its build
launches (older macOS / CI); when the upstream build supports macOS 26 it'll "just work".

## How it works

- **`/harness?c=<Component>&fixture=<name>`** (`src/routes/harness/`) — a client-only route that
  installs the mock IPC, then mounts the requested component.
- **`src/lib/harness/fixtures.ts`** — canned IPC responses. It replaces
  `window.__TAURI_INTERNALS__.invoke` (the one hook every `commands.*` call funnels through), so
  `getProject` / `listAssetRecords` / image commands return fixture data. Imagery is inline SVG
  data URLs — no binary fixtures.
- **`tests/visual/*.spec.ts`** — drive the harness and assert on the rendered DOM (element counts,
  non-zero box sizes, centering, layer toggles). A screenshot lands in `__artifacts__/` for
  eyeballing (not a pixel-diff baseline — SVG/font rendering varies per machine).

## Add a component or fixture

1. Import the component in `src/routes/harness/+page.svelte` and add a `{:else if component === "…"}`
   branch.
2. Add a fixture to `FIXTURES` in `fixtures.ts` (a `project`, `descriptors`, `records`, and an
   `imageFor(key)`), and mock any extra commands the component calls in `mockInvoke`.
3. Add a spec under `tests/visual/`.
