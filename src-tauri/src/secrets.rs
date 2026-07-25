//! Secret storage. API keys never touch `project.json` or the webview.
//!
//! **Release builds** use the OS keychain (macOS Keychain).
//!
//! **Debug builds** use a mode-0600 JSON file in the app's data dir instead. Keychain
//! ACLs are bound to the binary's code signature, and every `cargo tauri dev` rebuild
//! produces a new unsigned binary — macOS would re-prompt for the login password on
//! every restart, once per key. On first debug read of a key that isn't in the file yet,
//! we fall back to the keychain once (one last prompt) and migrate the value into the
//! file, so existing keys carry over and the prompts stop.

use keyring::Entry;

const SERVICE: &str = "com.wishfell.assetpipeline";

pub const OPENAI_KEY: &str = "openai_api_key";
pub const GEMINI_KEY: &str = "gemini_api_key";
pub const SPRITECOOK_KEY: &str = "spritecook_api_key";
pub const GAMELAB_KEY: &str = "gamelab_api_key";

fn entry(key: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, key).map_err(|e| format!("keychain error: {e}"))
}

#[cfg_attr(debug_assertions, allow(dead_code))]
fn keychain_set(key: &str, value: &str) -> Result<(), String> {
    let entry = entry(key)?;
    if value.is_empty() {
        // Deleting a missing entry is not an error for our purposes.
        let _ = entry.delete_credential();
        return Ok(());
    }
    entry
        .set_password(value)
        .map_err(|e| format!("failed to store secret: {e}"))
}

fn keychain_get(key: &str) -> Result<Option<String>, String> {
    match entry(key)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("failed to read secret: {e}")),
    }
}

// ── Debug-only file store (dev DX: no keychain prompts on every rebuild) ────────
#[cfg(debug_assertions)]
mod devstore {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn path() -> Result<PathBuf, String> {
        let home = std::env::var("HOME").map_err(|_| "no HOME env var".to_string())?;
        Ok(PathBuf::from(home)
            .join("Library/Application Support")
            .join(super::SERVICE)
            .join("dev_secrets.json"))
    }

    fn read_all() -> Result<BTreeMap<String, String>, String> {
        let p = path()?;
        if !p.is_file() {
            return Ok(BTreeMap::new());
        }
        let bytes = std::fs::read(&p).map_err(|e| format!("read dev secrets: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("parse dev secrets: {e}"))
    }

    fn write_all(map: &BTreeMap<String, String>) -> Result<(), String> {
        let p = path()?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create secrets dir: {e}"))?;
        }
        let json = serde_json::to_vec_pretty(map).map_err(|e| format!("encode secrets: {e}"))?;
        std::fs::write(&p, json).map_err(|e| format!("write dev secrets: {e}"))?;
        // Owner-only: this file holds API keys in the clear (debug builds only).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn set(key: &str, value: &str) -> Result<(), String> {
        let mut map = read_all()?;
        if value.is_empty() {
            map.remove(key);
        } else {
            map.insert(key.to_string(), value.to_string());
        }
        write_all(&map)
    }

    pub fn get(key: &str) -> Result<Option<String>, String> {
        if let Some(v) = read_all()?.get(key) {
            return Ok(Some(v.clone()));
        }
        // One-time migration: pull an existing key out of the keychain (single prompt)
        // and persist it here so future dev launches never prompt again.
        match super::keychain_get(key) {
            Ok(Some(v)) if !v.is_empty() => {
                let _ = set(key, &v);
                Ok(Some(v))
            }
            Ok(_) => Ok(None),
            // Keychain refusal/cancel in dev shouldn't brick the app — treat as unset.
            Err(_) => Ok(None),
        }
    }
}

/// Store (or overwrite) a secret. An empty value deletes it.
pub fn set(key: &str, value: &str) -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        devstore::set(key, value)
    }
    #[cfg(not(debug_assertions))]
    {
        keychain_set(key, value)
    }
}

/// Read a secret, or `None` if unset.
pub fn get(key: &str) -> Result<Option<String>, String> {
    #[cfg(debug_assertions)]
    {
        devstore::get(key)
    }
    #[cfg(not(debug_assertions))]
    {
        keychain_get(key)
    }
}

/// Whether a secret is set (and non-empty).
pub fn present(key: &str) -> bool {
    matches!(get(key), Ok(Some(v)) if !v.is_empty())
}
