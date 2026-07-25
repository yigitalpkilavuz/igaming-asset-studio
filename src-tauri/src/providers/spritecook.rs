//! SpriteCook (spritecook.ai): style-locked game-asset generation, "detailed/HD" mode.
//! Bearer-key API (`sc_live_…`, stored in the keychain). Stills ride the synchronous
//! `/v1/api/generate-sync` endpoint with `bg_mode: "transparent"` → native alpha.
//!
//! The response envelope isn't publicly documented, so parsing is deliberately tolerant:
//! we hunt for the first asset URL across the field names the official MCP tooling uses
//! (`sprite_url`, `spritesheet_url`, `url`, `output`, `assets[]`) and surface the raw
//! body in errors so drift is diagnosable from the UI.

use async_trait::async_trait;

use super::{BgMode, GenOutput, GenRequest, ImageProvider};

pub const API_BASE: &str = "https://api.spritecook.ai";

pub struct SpriteCook {
    api_key: Option<String>,
    model: String,
}

impl SpriteCook {
    pub fn new(api_key: Option<String>, model: String) -> Self {
        let model = if model.trim().is_empty() {
            "gemini-3.1-flash-image".to_string()
        } else {
            model
        };
        Self { api_key, model }
    }
}

/// Nearest supported aspect ratio ("1:1" | "16:9" | "9:16").
fn nearest_aspect(w: u32, h: u32) -> &'static str {
    let r = w as f64 / h.max(1) as f64;
    let candidates: [(&str, f64); 3] = [("1:1", 1.0), ("16:9", 16.0 / 9.0), ("9:16", 9.0 / 16.0)];
    candidates
        .iter()
        .min_by(|a, b| (a.1 - r).abs().total_cmp(&(b.1 - r).abs()))
        .map(|(s, _)| *s)
        .unwrap_or("1:1")
}

/// Tolerant hunt for an asset id in the (undocumented) import envelope.
pub(crate) fn find_asset_id(v: &serde_json::Value) -> Option<String> {
    const KEYS: [&str; 3] = ["asset_id", "id", "assetId"];
    match v {
        serde_json::Value::Object(map) => {
            for k in KEYS {
                if let Some(serde_json::Value::String(s)) = map.get(k) {
                    if !s.is_empty() {
                        return Some(s.clone());
                    }
                }
            }
            for k in ["asset", "data", "result"] {
                if let Some(inner) = map.get(k) {
                    if let Some(id) = find_asset_id(inner) {
                        return Some(id);
                    }
                }
            }
            None
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(find_asset_id),
        _ => None,
    }
}

/// Cache of uploaded reference images → SpriteCook asset ids (keyed by content hash),
/// so the style anchor is imported ONCE per run — not once per generation.
static REF_ASSETS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Import a reference image as a SpriteCook asset and return its id (cached by content).
/// Powers `reference_asset_id` — SpriteCook's own style-consistency mechanism.
async fn import_reference(
    client: &reqwest::Client,
    api_key: &str,
    png: &[u8],
) -> Result<String, String> {
    use base64::Engine;
    let sha = crate::studio::sha256_hex(png);
    if let Some(id) = REF_ASSETS.lock().unwrap().get(&sha) {
        return Ok(id.clone());
    }
    let body = serde_json::json!({
        "image": format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        ),
        "pixel": false,
        "display_name": "style reference",
        "file_name": "reference.png",
    });
    let resp = client
        .post(format!("{API_BASE}/v1/api/assets/import"))
        .bearer_auth(api_key)
        .timeout(std::time::Duration::from_secs(60))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("SpriteCook reference import failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read import failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("SpriteCook reference import error {status}: {}", trunc_body(&text)));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad import response: {e}"))?;
    let id = find_asset_id(&parsed)
        .ok_or_else(|| format!("reference import returned no asset id: {}", trunc_body(&text)))?;
    REF_ASSETS.lock().unwrap().insert(sha, id.clone());
    Ok(id)
}

/// Depth-first hunt for the first plausible asset URL in an undocumented envelope.
pub(crate) fn find_asset_url(v: &serde_json::Value) -> Option<String> {
    const KEYS: [&str; 5] = ["spritesheet_url", "sprite_url", "url", "output", "image_url"];
    match v {
        serde_json::Value::Object(map) => {
            for k in KEYS {
                if let Some(serde_json::Value::String(s)) = map.get(k) {
                    if s.starts_with("http") {
                        return Some(s.clone());
                    }
                }
            }
            // Common containers first, then everything else.
            for k in ["assets", "output", "result", "data", "job"] {
                if let Some(inner) = map.get(k) {
                    if let Some(u) = find_asset_url(inner) {
                        return Some(u);
                    }
                }
            }
            map.values().find_map(find_asset_url)
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(find_asset_url),
        _ => None,
    }
}

/// Resolve a SpriteCook response to its FINISHED envelope. The `-sync` endpoints enqueue
/// slow jobs anyway and return `{ job_id, status: "queued", poll_url, … }` with all
/// output fields null — so when no asset URL is present yet, poll `GET {poll_url}` until
/// the job finishes, fails, or `timeout` elapses.
pub(crate) async fn await_job(
    client: &reqwest::Client,
    api_key: &str,
    initial: serde_json::Value,
    raw_text: &str,
    timeout: std::time::Duration,
    mut on_poll: impl FnMut(&serde_json::Value),
) -> Result<serde_json::Value, String> {
    if find_asset_url(&initial).is_some() {
        return Ok(initial); // already finished synchronously
    }
    let poll_url = initial
        .get("poll_url")
        .and_then(|v| v.as_str())
        .map(|p| format!("{API_BASE}{p}"))
        .or_else(|| {
            initial
                .get("job_id")
                .and_then(|v| v.as_str())
                .map(|id| format!("{API_BASE}/v1/api/jobs/{id}"))
        })
        .ok_or_else(|| format!("SpriteCook returned no result and no job: {}", trunc_body(raw_text)))?;

    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > timeout {
            return Err(format!(
                "SpriteCook job timed out after {}s (still {}): {poll_url}",
                timeout.as_secs(),
                initial.get("status").and_then(|s| s.as_str()).unwrap_or("pending"),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

        let resp = client
            .get(&poll_url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|e| format!("SpriteCook poll failed: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("read poll failed: {e}"))?;
        if !status.is_success() {
            return Err(format!("SpriteCook poll error {status}: {}", trunc_body(&text)));
        }
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("bad poll response: {e}"))?;
        on_poll(&v);

        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            if !err.is_empty() {
                return Err(format!("SpriteCook job failed: {err}"));
            }
        }
        let job_status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
        if matches!(job_status, "failed" | "error" | "cancelled") {
            return Err(format!("SpriteCook job {job_status}: {}", trunc_body(&text)));
        }
        if find_asset_url(&v).is_some() {
            return Ok(v);
        }
        // Finished but no URL anywhere → surface the body instead of spinning forever.
        if v.get("finished_at").is_some_and(|f| !f.is_null()) {
            return Err(format!(
                "SpriteCook job finished without a result URL: {}",
                trunc_body(&text)
            ));
        }
    }
}

/// Fetch a result URL and normalize the bytes to PNG.
pub(crate) async fn fetch_png(url: &str) -> Result<Vec<u8>, String> {
    let resp = reqwest::get(url).await.map_err(|e| format!("download result failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download result failed: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("read result failed: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("decode result failed: {e}"))?;
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("encode png failed: {e}"))?;
    Ok(buf.into_inner())
}

pub(crate) fn trunc_body(text: &str) -> String {
    text.chars().take(400).collect()
}

#[async_trait]
impl ImageProvider for SpriteCook {
    fn id(&self) -> &'static str {
        super::SPRITECOOK
    }
    fn display_name(&self) -> &'static str {
        "SpriteCook"
    }
    fn supports_seed(&self) -> bool {
        false
    }
    fn native_alpha(&self) -> bool {
        true
    }
    fn supports_references(&self) -> bool {
        // The FIRST reference (the style anchor rides first) is imported as a SpriteCook
        // asset and passed as `reference_asset_id` — the API takes exactly one.
        true
    }
    fn configured(&self) -> bool {
        self.api_key.as_deref().is_some_and(|k| !k.is_empty())
    }

    async fn generate(&self, req: &GenRequest) -> Result<GenOutput, String> {
        let key = self.api_key.as_deref().filter(|k| !k.is_empty()).ok_or_else(|| {
            "SpriteCook API key is not set (add it in Settings).".to_string()
        })?;

        let mut prompt = req.prompt.clone();
        if !req.negative.trim().is_empty() {
            // No dedicated negative field — fold it into the instruction.
            prompt = format!("{prompt}\n\nStrictly AVOID all of the following: {}", req.negative);
        }
        let mut body = serde_json::json!({
            "prompt": prompt,
            "pixel": false,
            "bg_mode": if req.transparent { "transparent" } else { "include" },
            "aspect_ratio": nearest_aspect(req.width, req.height),
            // Long side ≥ ~1200 (backgrounds) earns the 2K tier; symbols stay at 1K.
            "resolution": if req.width.max(req.height) >= 1200 { "2K" } else { "1K" },
            "model": self.model,
            "variations": 1,
            "smart_crop": false,
            "mode": "assets",
        });

        let client = reqwest::Client::new();
        // Style consistency: SpriteCook takes ONE reference asset. The game's style
        // anchor always rides first in `references`, so that's the one that ships
        // (extra manual references are ignored by this provider).
        if let Some(reference) = req.references.first() {
            let id = import_reference(&client, key, reference).await?;
            body["reference_asset_id"] = serde_json::json!(id);
        }
        let resp = client
            .post(format!("{API_BASE}/v1/api/generate-sync"))
            .bearer_auth(key)
            .timeout(std::time::Duration::from_secs(120))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("SpriteCook request failed: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
        if !status.is_success() {
            return Err(format!("SpriteCook error {status}: {}", trunc_body(&text)));
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("bad SpriteCook response: {e}"))?;
        // Slow jobs come back queued with a poll_url — wait for the finished envelope.
        let done =
            await_job(&client, key, parsed, &text, std::time::Duration::from_secs(240), |_| {})
                .await?;
        let url = find_asset_url(&done)
            .ok_or_else(|| "SpriteCook finished without an asset URL".to_string())?;
        let png = fetch_png(&url).await?;

        Ok(GenOutput {
            png,
            seed: None,
            background: if req.transparent { BgMode::Native } else { BgMode::Opaque },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_maps_to_nearest_supported() {
        assert_eq!(nearest_aspect(1024, 1024), "1:1");
        assert_eq!(nearest_aspect(3200, 1800), "16:9");
        assert_eq!(nearest_aspect(1800, 3200), "9:16");
        assert_eq!(nearest_aspect(1200, 1000), "1:1");
    }

    #[test]
    fn await_job_passthrough_and_missing_job_error() {
        let client = reqwest::Client::new();
        // Already-finished envelope passes straight through, no polling.
        let done = serde_json::json!({ "status": "completed", "sprite_url": "https://x/a.png" });
        let out = futures::executor::block_on(await_job(
            &client,
            "k",
            done.clone(),
            "{}",
            std::time::Duration::from_secs(1),
            |_| {},
        ))
        .unwrap();
        assert_eq!(out, done);
        // No result URL and no job reference → immediate, diagnosable error.
        let bad = serde_json::json!({ "status": "ok" });
        let err = futures::executor::block_on(await_job(
            &client,
            "k",
            bad,
            "{\"status\":\"ok\"}",
            std::time::Duration::from_secs(1),
            |_| {},
        ))
        .unwrap_err();
        assert!(err.contains("no result and no job"), "{err}");
    }

    #[test]
    fn url_hunt_covers_known_envelope_shapes() {
        let shapes = [
            serde_json::json!({ "sprite_url": "https://x/a.png" }),
            serde_json::json!({ "assets": [{ "url": "https://x/a.png" }] }),
            serde_json::json!({ "job": { "output": { "spritesheet_url": "https://x/a.png" } } }),
            serde_json::json!({ "data": [{ "sprite_url": "https://x/a.png" }] }),
        ];
        for s in &shapes {
            assert_eq!(find_asset_url(s).as_deref(), Some("https://x/a.png"), "{s}");
        }
        // Non-URL strings and unrelated fields don't match.
        assert!(find_asset_url(&serde_json::json!({ "status": "done", "id": "abc" })).is_none());
    }
}
