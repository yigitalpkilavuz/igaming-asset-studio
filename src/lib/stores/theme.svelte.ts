/** Light/dark theme, persisted in localStorage and applied via `data-theme` on <html>.
 *  Dark is the default (and the token baseline); light overrides live in app.css. */

export type Theme = "dark" | "light";

const KEY = "wf-theme";

function initial(): Theme {
  if (typeof localStorage !== "undefined" && localStorage.getItem(KEY) === "light") {
    return "light";
  }
  return "dark";
}

export const themeState = $state<{ theme: Theme }>({ theme: initial() });

function apply(theme: Theme) {
  if (typeof document === "undefined") return;
  if (theme === "light") {
    document.documentElement.dataset.theme = "light";
  } else {
    delete document.documentElement.dataset.theme;
  }
}

export function setTheme(theme: Theme) {
  themeState.theme = theme;
  apply(theme);
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    /* private mode etc. — theme just won't persist */
  }
}

// Apply on module load (client only; prerender has no document).
apply(themeState.theme);
