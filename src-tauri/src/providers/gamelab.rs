//! Gamelab Studio (gamelabstudio.co): AI game-asset generation with a native
//! transparency pipeline (chroma key end-to-end → alpha PNG). `X-API-Key` auth;
//! 1 credit per artwork. Generation is ASYNC: `POST /v1/generate/image` (multipart
//! form) returns `{job_id, status: "queued"}`; we poll `GET /v1/jobs/{id}` until
//! `completed`, then download the artifact.
//!
//! Documented contract (mined from their docs app, 2026-07):
//! - `POST /v1/generate/image` form fields: `prompt` (required), `images` (0..n files,
//!   style/seed references), `transparent_bg` (bool), `chroma_key_hex` (#RRGGBB),
//!   `width`/`height` (px), `centered` (bool), `camera_orientation`
//!   (top|bottom|front|back|left_side|right_side), `subject_orientation_in_frame`.
//! - `GET /v1/jobs/{job_id}` → `{status: queued|processing|completed|failed, error,
//!   artifacts: [{kind, path, sequence, …}]}`.
//! - `GET /v1/assets/{asset_id}/download[?kind=…]` → binary.
//! - Errors: 401 bad key, 402 out of credits, 403 forbidden.
//!
//! The completed-job envelope's exact artifact shape for images is not published, so
//! artifact resolution is deliberately tolerant (URL hunt, then asset-id hunt) and
//! errors carry the raw body — same policy as the SpriteCook provider.

use async_trait::async_trait;

use super::{BgMode, GenOutput, GenRequest, ImageProvider};

pub const API_BASE: &str = "https://api.gamelabstudio.co";

pub struct Gamelab {
    api_key: Option<String>,
}

impl Gamelab {
    pub fn new(api_key: Option<String>) -> Self {
        Self { api_key }
    }
}

/// Depth-first hunt for a direct download URL in an envelope.
pub(crate) fn find_url(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            for k in ["download_url", "url", "image_url", "asset_url"] {
                if let Some(serde_json::Value::String(s)) = map.get(k) {
                    if s.starts_with("http") {
                        return Some(s.clone());
                    }
                }
            }
            map.values().find_map(find_url)
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(find_url),
        _ => None,
    }
}

/// Depth-first hunt for an asset id (artifacts carry one for the download endpoint).
pub(crate) fn find_gl_asset_id(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            for k in ["asset_id", "assetId"] {
                match map.get(k) {
                    Some(serde_json::Value::String(s)) if !s.is_empty() => return Some(s.clone()),
                    Some(serde_json::Value::Number(n)) => return Some(n.to_string()),
                    _ => {}
                }
            }
            for k in ["artifacts", "assets", "data", "result"] {
                if let Some(inner) = map.get(k) {
                    if let Some(id) = find_gl_asset_id(inner) {
                        return Some(id);
                    }
                }
            }
            map.values().find_map(find_gl_asset_id)
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(find_gl_asset_id),
        _ => None,
    }
}

fn trunc(text: &str) -> String {
    text.chars().take(400).collect()
}

/// Map the status codes their docs call out to actionable messages.
fn friendly_status(status: reqwest::StatusCode, body: &str) -> String {
    match status.as_u16() {
        401 => "Gamelab Studio rejected the API key (401) — re-copy it in Settings.".into(),
        402 => "Gamelab Studio: out of credits (402) — top up at gamelabstudio.co.".into(),
        403 => format!("Gamelab Studio: forbidden (403): {}", trunc(body)),
        _ => format!("Gamelab Studio error {status}: {}", trunc(body)),
    }
}

/// Poll `GET /v1/jobs/{id}` until the job completes (2.5 s cadence).
async fn await_job(
    client: &reqwest::Client,
    api_key: &str,
    job_id: &str,
    timeout: std::time::Duration,
) -> Result<serde_json::Value, String> {
    let url = format!("{API_BASE}/v1/jobs/{job_id}");
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > timeout {
            return Err(format!(
                "Gamelab Studio job timed out after {}s: {url}",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

        let resp = client
            .get(&url)
            .header("X-API-Key", api_key)
            .send()
            .await
            .map_err(|e| format!("Gamelab Studio poll failed: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("read poll failed: {e}"))?;
        if !status.is_success() {
            return Err(friendly_status(status, &text));
        }
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("bad job response: {e}"))?;
        match v.get("status").and_then(|s| s.as_str()).unwrap_or("") {
            "completed" => return Ok(v),
            "failed" | "cancelled" | "error" => {
                let err = v
                    .get("error")
                    .and_then(|e| e.as_str())
                    .filter(|e| !e.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| trunc(&text));
                return Err(format!("Gamelab Studio job failed: {err}"));
            }
            _ => {} // queued | processing → keep polling
        }
    }
}

/// Download the completed job's image: a direct URL when the envelope has one,
/// else the assets endpoint by artifact asset id (retrying once with `?kind=image`).
async fn download_result(
    client: &reqwest::Client,
    api_key: &str,
    done: &serde_json::Value,
) -> Result<Vec<u8>, String> {
    let candidates: Vec<String> = if let Some(url) = find_url(done) {
        vec![url]
    } else if let Some(id) = find_gl_asset_id(done) {
        vec![
            format!("{API_BASE}/v1/assets/{id}/download"),
            format!("{API_BASE}/v1/assets/{id}/download?kind=image"),
        ]
    } else {
        return Err(format!(
            "Gamelab Studio job completed without a downloadable artifact: {}",
            trunc(&done.to_string())
        ));
    };

    let mut last_err = String::new();
    for url in &candidates {
        let resp = client
            .get(url)
            .header("X-API-Key", api_key)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("download failed: {e}"))?;
        if resp.status().is_success() {
            let bytes =
                resp.bytes().await.map_err(|e| format!("read download failed: {e}"))?;
            return Ok(bytes.to_vec());
        }
        last_err = format!("HTTP {} at {url}", resp.status());
    }
    Err(format!("Gamelab Studio artifact download failed: {last_err}"))
}

#[async_trait]
impl ImageProvider for Gamelab {
    fn id(&self) -> &'static str {
        super::GAMELAB
    }
    fn display_name(&self) -> &'static str {
        "Gamelab Studio"
    }
    fn supports_seed(&self) -> bool {
        false
    }
    fn native_alpha(&self) -> bool {
        true
    }
    fn supports_references(&self) -> bool {
        true // `images` form field takes one or more reference files
    }
    fn configured(&self) -> bool {
        self.api_key.as_deref().is_some_and(|k| !k.is_empty())
    }

    async fn generate(&self, req: &GenRequest) -> Result<GenOutput, String> {
        let key = self.api_key.as_deref().filter(|k| !k.is_empty()).ok_or_else(|| {
            "Gamelab Studio API key is not set (add it in Settings).".to_string()
        })?;

        let mut prompt = req.prompt.clone();
        if !req.negative.trim().is_empty() {
            // No dedicated negative field — fold it into the instruction.
            prompt = format!("{prompt}\n\nStrictly AVOID all of the following: {}", req.negative);
        }

        let mut form = reqwest::multipart::Form::new()
            .text("prompt", prompt)
            .text("width", req.width.to_string())
            .text("height", req.height.to_string());
        if req.transparent {
            form = form
                .text("transparent_bg", "true")
                .text("chroma_key_hex", "#FF00FF");
        }
        for (i, bytes) in req.references.iter().enumerate() {
            form = form.part(
                "images",
                reqwest::multipart::Part::bytes(bytes.clone())
                    .file_name(format!("reference_{i}.png"))
                    .mime_str("image/png")
                    .map_err(|e| format!("reference part failed: {e}"))?,
            );
        }

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{API_BASE}/v1/generate/image"))
            .header("X-API-Key", key)
            .multipart(form)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("Gamelab Studio request failed: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
        if !status.is_success() {
            return Err(friendly_status(status, &text));
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("bad response: {e}"))?;
        let job_id = parsed
            .get("job_id")
            .and_then(|j| j.as_str())
            .ok_or_else(|| format!("Gamelab Studio returned no job id: {}", trunc(&text)))?;

        let done = await_job(&client, key, job_id, std::time::Duration::from_secs(240)).await?;
        let bytes = download_result(&client, key, &done).await?;

        // Normalise to PNG, and read the ACTUAL background off the pixels: their
        // transparency pipeline should deliver alpha, but if a roll comes back as an
        // unkeyed flat magenta, declaring it Chroma lets our own keyer finish the job.
        let img = image::load_from_memory(&bytes)
            .map_err(|e| format!("decode Gamelab Studio image: {e}"))?;
        let rgba = img.to_rgba8();
        let has_alpha = rgba.pixels().any(|p| p.0[3] < 250);
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| format!("encode png failed: {e}"))?;

        let background = if !req.transparent {
            BgMode::Opaque
        } else if has_alpha {
            BgMode::Native
        } else {
            BgMode::Chroma
        };
        Ok(GenOutput { png: buf.into_inner(), seed: None, background })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_hunt_covers_documented_and_likely_shapes() {
        // Documented completed-job artifact list (path is a server-side abs path — not
        // downloadable by us; an asset id or URL must be found instead).
        let with_id = serde_json::json!({
            "job_id": "2b10", "status": "completed",
            "artifacts": [{ "kind": "output", "path": "/data/jobs/x/output/image_000.png",
                            "sequence": 0, "asset_id": 123 }]
        });
        assert_eq!(find_gl_asset_id(&with_id).as_deref(), Some("123"));

        let with_url = serde_json::json!({
            "artifacts": [{ "kind": "output", "download_url": "https://x/a.png" }]
        });
        assert_eq!(find_url(&with_url).as_deref(), Some("https://x/a.png"));

        // Nothing downloadable → both hunts empty (caller surfaces the body).
        let bare = serde_json::json!({
            "status": "completed",
            "artifacts": [{ "kind": "output", "path": "/data/x.png", "sequence": 0 }]
        });
        assert!(find_url(&bare).is_none());
        assert!(find_gl_asset_id(&bare).is_none());
    }

    #[test]
    fn status_codes_map_to_actionable_messages() {
        assert!(friendly_status(reqwest::StatusCode::UNAUTHORIZED, "").contains("API key"));
        assert!(friendly_status(reqwest::StatusCode::PAYMENT_REQUIRED, "").contains("credits"));
        assert!(friendly_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "boom")
            .contains("error 500"));
    }
}
