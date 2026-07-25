import { writeText } from "@tauri-apps/plugin-clipboard-manager";

/**
 * Copy text to the system clipboard, reliably, from anywhere.
 *
 * Inside the Tauri webview the web APIs are treacherous: `navigator.clipboard` is often
 * blocked outright, and `document.execCommand("copy")` only works synchronously inside a
 * user gesture — one `await` before it (e.g. an IPC call to build the text) and it fails.
 * The clipboard-manager plugin goes through the Rust side and has neither restriction.
 * The web APIs remain as fallbacks so this also works in a plain browser (dev harnesses).
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    await writeText(text);
    return true;
  } catch {
    /* not in Tauri, or plugin unavailable — fall through */
  }
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    /* blocked — fall through */
  }
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.style.position = "fixed";
  ta.style.top = "-9999px";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.focus();
  ta.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  document.body.removeChild(ta);
  return ok;
}
