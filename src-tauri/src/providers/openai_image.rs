//! Cloud provider: OpenAI gpt-image via `/v1/images/generations` (text-only) or
//! `/v1/images/edits` (when style-reference images are supplied). Shares the same API key
//! as prompt generation.

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};

use super::{GenOutput, GenRequest, ImageProvider};

const ENDPOINT: &str = "https://api.openai.com/v1/images/generations";
const EDITS_ENDPOINT: &str = "https://api.openai.com/v1/images/edits";

/// Framing prepended to the prompt when style-reference images are attached: they steer the
/// look, not the content. Without this the edits endpoint tends to redraw the reference itself.
const STYLE_REF_CLAUSE: &str = "The attached image(s) are STYLE REFERENCES ONLY: match their \
art style, colour palette, linework, shading, materials and overall rendering so the new symbol \
belongs to the same set — but draw the NEW subject described below. Do not copy the reference's \
subject, pose or composition. ";

pub struct OpenAiImage {
    api_key: Option<String>,
    model: String,
}

impl OpenAiImage {
    pub fn new(api_key: Option<String>, model: String) -> Self {
        Self { api_key, model }
    }

    /// Generate via `/images/edits` with one or more style-reference images attached as
    /// `image[]` inputs. Returns the produced PNG bytes.
    async fn generate_with_references(
        &self,
        key: &str,
        prompt: &str,
        w: u32,
        h: u32,
        references: &[Vec<u8>],
    ) -> Result<Vec<u8>, String> {
        let mut form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .text("prompt", prompt.to_string())
            .text("size", size_param(&self.model, w, h))
            .text("n", "1");
        for (i, bytes) in references.iter().enumerate() {
            let part = reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(format!("ref_{i}.png"))
                .mime_str("image/png")
                .map_err(|e| format!("build reference part: {e}"))?;
            // gpt-image accepts repeated `image[]` fields for multiple reference images.
            form = form.part("image[]", part);
        }

        let client = reqwest::Client::new();
        let resp = client
            .post(EDITS_ENDPOINT)
            .bearer_auth(key)
            .multipart(form)
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| format!("OpenAI image-edit request failed: {e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("read response failed: {e}"))?;
        if !status.is_success() {
            return Err(format!("OpenAI image-edit error {status}: {text}"));
        }
        let parsed: ImageResponse =
            serde_json::from_str(&text).map_err(|e| format!("bad OpenAI image response: {e}"))?;
        let b64 = parsed
            .data
            .into_iter()
            .next()
            .and_then(|d| d.b64_json)
            .ok_or_else(|| "OpenAI returned no image data".to_string())?;
        base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .map_err(|e| format!("failed to decode image: {e}"))
    }
}

/// The size parameter for the model in use.
///
/// gpt-image-2 accepts CUSTOM sizes: both edges divisible by 16, aspect within
/// 1:3–3:1, max edge 3840 — so it gets the true requested canvas (a 16:9 background
/// generates at real 16:9, not the legacy 3:2). The gpt-image-1 family only offers
/// three fixed sizes; requests snap to the nearest.
fn size_param(model: &str, w: u32, h: u32) -> String {
    if model.starts_with("gpt-image-1") {
        return (if w as f32 > h as f32 * 1.2 {
            "1536x1024"
        } else if h as f32 > w as f32 * 1.2 {
            "1024x1536"
        } else {
            "1024x1024"
        })
        .to_string();
    }
    // Clamp aspect into the supported 1:3–3:1 window, round to /16, cap the long edge.
    let (mut w, mut h) = (w.max(16) as f64, h.max(16) as f64);
    if w > h * 3.0 {
        w = h * 3.0;
    } else if h > w * 3.0 {
        h = w * 3.0;
    }
    let cap = 3840.0;
    let long = w.max(h);
    if long > cap {
        let s = cap / long;
        w *= s;
        h *= s;
    }
    let r16 = |v: f64| (((v / 16.0).round() as u32).max(1) * 16).min(3840);
    format!("{}x{}", r16(w), r16(h))
}

#[derive(Serialize)]
struct ImageRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    size: String,
    /// Only sent to models that support it (gpt-image-1.x). gpt-image-2 rejects it.
    #[serde(skip_serializing_if = "Option::is_none")]
    background: Option<&'a str>,
    n: u32,
}

#[derive(Deserialize)]
struct ImageResponse {
    data: Vec<ImageDatum>,
}

#[derive(Deserialize)]
struct ImageDatum {
    b64_json: Option<String>,
}

#[async_trait]
impl ImageProvider for OpenAiImage {
    fn id(&self) -> &'static str {
        super::OPENAI_IMAGE
    }
    fn display_name(&self) -> &'static str {
        "OpenAI gpt-image (cloud)"
    }
    fn supports_seed(&self) -> bool {
        false
    }
    fn native_alpha(&self) -> bool {
        true
    }
    fn configured(&self) -> bool {
        self.api_key.as_ref().is_some_and(|k| !k.is_empty())
    }

    async fn generate(&self, req: &GenRequest) -> Result<GenOutput, String> {
        let key = self
            .api_key
            .as_ref()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;

        // Transparent background is only supported by the gpt-image-1 family; gpt-image-2
        // returns a 400 for it. For unsupported models that still need alpha, paint a
        // chroma-key background and remove it in post-processing.
        let native_ok = req.transparent && self.model.starts_with("gpt-image-1");
        let (mut prompt, bg) = if native_ok {
            (req.prompt.clone(), super::BgMode::Native)
        } else if req.transparent {
            (
                format!("{}{}", req.prompt, super::chroma_instruction()),
                super::BgMode::Chroma,
            )
        } else {
            (req.prompt.clone(), super::BgMode::Opaque)
        };

        // Style references → use the multipart /images/edits endpoint (gpt-image), passing the
        // references as image inputs and steering the model to match their look only.
        if !req.references.is_empty() {
            prompt = format!("{STYLE_REF_CLAUSE}{prompt}");
            let png = self
                .generate_with_references(key, &prompt, req.width, req.height, &req.references)
                .await?;
            return Ok(GenOutput { png, seed: None, background: bg });
        }

        let body = ImageRequest {
            model: &self.model,
            prompt: &prompt,
            size: size_param(&self.model, req.width, req.height),
            background: native_ok.then_some("transparent"),
            n: 1,
        };

        let client = reqwest::Client::new();
        let resp = client
            .post(ENDPOINT)
            .bearer_auth(key)
            .json(&body)
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| format!("OpenAI image request failed: {e}"))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("read response failed: {e}"))?;
        if !status.is_success() {
            return Err(format!("OpenAI image error {status}: {text}"));
        }

        let parsed: ImageResponse =
            serde_json::from_str(&text).map_err(|e| format!("bad OpenAI image response: {e}"))?;
        let b64 = parsed
            .data
            .into_iter()
            .next()
            .and_then(|d| d.b64_json)
            .ok_or_else(|| "OpenAI returned no image data".to_string())?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .map_err(|e| format!("failed to decode image: {e}"))?;

        Ok(GenOutput {
            png,
            seed: None,
            background: bg,
        })
    }
}

/// gpt-image-2: edit-capable with strong spatial awareness and high-fidelity image inputs.
const EDIT_MODEL: &str = "gpt-image-2";

/// One-shot `/v1/images/edits` call on a single source image (API upscale, inpainting).
/// Returns the produced PNG bytes. `size` is a gpt-image size like `1024x1536`.
/// `mask`, when given, is a same-size PNG whose TRANSPARENT pixels mark the regions the
/// model may regenerate (the OpenAI edits contract); opaque pixels are preserved intent.
pub async fn edit_image(
    api_key: &str,
    source_png: &[u8],
    prompt: &str,
    size: &str,
    mask: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let image_part = reqwest::multipart::Part::bytes(source_png.to_vec())
        .file_name("source.png")
        .mime_str("image/png")
        .map_err(|e| format!("build image part: {e}"))?;

    let mut form = reqwest::multipart::Form::new()
        .text("model", EDIT_MODEL)
        .text("prompt", prompt.to_string())
        .text("size", size.to_string())
        .text("n", "1")
        .part("image", image_part);
    if let Some(mask_png) = mask {
        let mask_part = reqwest::multipart::Part::bytes(mask_png.to_vec())
            .file_name("mask.png")
            .mime_str("image/png")
            .map_err(|e| format!("build mask part: {e}"))?;
        form = form.part("mask", mask_part);
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(EDITS_ENDPOINT)
        .bearer_auth(api_key)
        .multipart(form)
        .timeout(std::time::Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| format!("image edit request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("OpenAI image-edit error {status}: {text}"));
    }
    let parsed: ImageResponse =
        serde_json::from_str(&text).map_err(|e| format!("bad edit response: {e}"))?;
    let b64 = parsed
        .data
        .into_iter()
        .next()
        .and_then(|d| d.b64_json)
        .ok_or_else(|| "image edit returned no data".to_string())?;
    base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| format!("decode edited image: {e}"))
}

/// Edit canvas size for gpt-image-2: matches the source aspect, long side ~1536px,
/// each dimension divisible by 16, aspect clamped to the model's supported 1:3..3:1.
pub fn edit_size(w: u32, h: u32) -> String {
    let (w, h) = (w.max(1) as f64, h.max(1) as f64);
    let long = 1536.0;
    let minor_min = long / 3.0; // 3:1 aspect cap
    let (tw, th) = if w >= h {
        (long, (long * h / w).clamp(minor_min, long))
    } else {
        ((long * w / h).clamp(minor_min, long), long)
    };
    let r16 = |v: f64| (((v / 16.0).round()).max(1.0) * 16.0) as u32;
    format!("{}x{}", r16(tw), r16(th))
}

#[cfg(test)]
mod tests {
    use super::size_param;

    #[test]
    fn gpt_image_2_gets_true_custom_sizes() {
        // 16:9 background generates at real 16:9 (both edges /16), not legacy 3:2.
        assert_eq!(size_param("gpt-image-2", 2048, 1152), "2048x1152");
        // Symbol square passes through.
        assert_eq!(size_param("gpt-image-2", 1024, 1024), "1024x1024");
        // Extreme column aspect clamps into the 1:3 window.
        assert_eq!(size_param("gpt-image-2", 512, 2560), "512x1536");
        // Oversized requests cap at the 3840 long edge, aspect preserved.
        assert_eq!(size_param("gpt-image-2", 8192, 4608), "3840x2160");
        // Odd dims round to /16.
        assert_eq!(size_param("gpt-image-2", 1000, 700), "1008x704");
    }

    #[test]
    fn gpt_image_1_snaps_to_its_three_fixed_sizes() {
        assert_eq!(size_param("gpt-image-1", 2048, 1152), "1536x1024");
        assert_eq!(size_param("gpt-image-1-mini", 1024, 1536), "1024x1536");
        assert_eq!(size_param("gpt-image-1", 1100, 1024), "1024x1024");
    }
}
