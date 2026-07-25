//! Local provider: Draw Things (or any A1111-compatible server) via `/sdapi/v1/txt2img`.

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};

use super::{GenOutput, GenRequest, ImageProvider};

pub struct DrawThings {
    base_url: String,
}

impl DrawThings {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }
}

#[derive(Serialize)]
struct Txt2Img<'a> {
    prompt: &'a str,
    negative_prompt: &'a str,
    width: u32,
    height: u32,
    steps: u32,
    cfg_scale: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
}

#[derive(Deserialize)]
struct Txt2ImgResponse {
    images: Vec<String>,
}

#[async_trait]
impl ImageProvider for DrawThings {
    fn id(&self) -> &'static str {
        super::DRAW_THINGS
    }
    fn display_name(&self) -> &'static str {
        "Draw Things (local)"
    }
    fn supports_seed(&self) -> bool {
        true
    }
    fn native_alpha(&self) -> bool {
        false
    }
    fn supports_references(&self) -> bool {
        false // A1111 txt2img — no image inputs.
    }
    fn configured(&self) -> bool {
        !self.base_url.is_empty()
    }

    async fn generate(&self, req: &GenRequest) -> Result<GenOutput, String> {
        // Draw Things has no native alpha; when an asset needs transparency, paint a
        // chroma-key background and remove it in post-processing.
        let (prompt, bg) = if req.transparent {
            (
                format!("{}{}", req.prompt, super::chroma_instruction()),
                super::BgMode::Chroma,
            )
        } else {
            (req.prompt.clone(), super::BgMode::Opaque)
        };
        let body = Txt2Img {
            prompt: &prompt,
            negative_prompt: &req.negative,
            width: req.width,
            height: req.height,
            steps: 30,
            cfg_scale: 5.0,
            seed: req.seed.map(|s| s as i64),
        };

        let url = format!("{}/sdapi/v1/txt2img", self.base_url);
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(600))
            .send()
            .await
            .map_err(|e| format!("Draw Things request failed ({url}): {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Draw Things error {status}: {text}"));
        }

        let parsed: Txt2ImgResponse = resp
            .json()
            .await
            .map_err(|e| format!("bad Draw Things response: {e}"))?;
        let b64 = parsed
            .images
            .into_iter()
            .next()
            .ok_or_else(|| "Draw Things returned no image".to_string())?;
        // A1111 sometimes prefixes a data URL.
        let b64 = b64.split(',').next_back().unwrap_or(&b64);
        let png = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("failed to decode image: {e}"))?;

        Ok(GenOutput {
            png,
            seed: req.seed,
            background: bg,
        })
    }
}
