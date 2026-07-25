//! Cloud provider: Google "Nano Banana" (Gemini image generation) via the Gemini API
//! `generateContent` endpoint. No native transparency → subjects use the magenta chroma
//! path like gpt-image-2. Style-reference images ride along as inline image parts.

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use super::{GenOutput, GenRequest, ImageProvider};

/// Framing for style references (mirrors the OpenAI provider's clause).
const STYLE_REF_CLAUSE: &str = "The attached image(s) are STYLE REFERENCES ONLY: match their \
art style, colour palette, linework, shading, materials and overall rendering so the new image \
belongs to the same set — but draw the NEW subject described below. Do not copy the reference's \
subject, pose or composition. ";

pub struct GeminiImage {
    api_key: Option<String>,
    model: String,
}

impl GeminiImage {
    pub fn new(api_key: Option<String>, model: String) -> Self {
        Self { api_key, model }
    }
}

/// Aspect ratios the Gemini image models accept — pick the closest to the request.
fn nearest_aspect(w: u32, h: u32) -> &'static str {
    const RATIOS: &[(&str, f64)] = &[
        ("1:1", 1.0),
        ("2:3", 2.0 / 3.0),
        ("3:2", 3.0 / 2.0),
        ("3:4", 3.0 / 4.0),
        ("4:3", 4.0 / 3.0),
        ("4:5", 4.0 / 5.0),
        ("5:4", 5.0 / 4.0),
        ("9:16", 9.0 / 16.0),
        ("16:9", 16.0 / 9.0),
        ("21:9", 21.0 / 9.0),
    ];
    let target = w.max(1) as f64 / h.max(1) as f64;
    RATIOS
        .iter()
        .min_by(|a, b| {
            let da = (a.1.ln() - target.ln()).abs();
            let db = (b.1.ln() - target.ln()).abs();
            da.partial_cmp(&db).unwrap()
        })
        .map(|(name, _)| *name)
        .unwrap_or("1:1")
}

#[derive(Deserialize)]
struct GenerateResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default, rename = "promptFeedback")]
    prompt_feedback: Option<PromptFeedback>,
}
#[derive(Deserialize)]
struct PromptFeedback {
    #[serde(default, rename = "blockReason")]
    block_reason: Option<String>,
}
#[derive(Deserialize)]
struct Candidate {
    content: Option<Content>,
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
}
#[derive(Deserialize)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}
#[derive(Deserialize)]
struct Part {
    #[serde(default, alias = "inline_data", rename = "inlineData")]
    inline_data: Option<InlineData>,
    #[serde(default)]
    text: Option<String>,
}
#[derive(Deserialize)]
struct InlineData {
    #[serde(default)]
    data: String,
}

/// A 200 with no image part means Gemini answered but withheld the image. Say WHY:
/// the prompt-level block reason, the candidate's finish reason, and whatever the
/// model said in text (it usually explains its refusal there) all beat "try rephrasing".
fn no_image_error(parsed: &GenerateResponse) -> String {
    if let Some(reason) = parsed
        .prompt_feedback
        .as_ref()
        .and_then(|f| f.block_reason.as_deref())
    {
        return format!(
            "Gemini blocked the prompt ({reason}) — reword the flagged part and retry"
        );
    }
    let finish = parsed
        .candidates
        .iter()
        .filter_map(|c| c.finish_reason.as_deref())
        .find(|r| *r != "STOP");
    let said: String = parsed
        .candidates
        .iter()
        .filter_map(|c| c.content.as_ref())
        .flat_map(|c| &c.parts)
        .filter_map(|p| p.text.as_deref())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect();
    match (finish, said.trim().is_empty()) {
        (Some(reason), true) => {
            format!("Gemini stopped without an image (finish reason {reason}) — try rephrasing")
        }
        (Some(reason), false) => format!(
            "Gemini stopped without an image (finish reason {reason}). It said: \"{}\"",
            said.trim()
        ),
        (None, false) => format!(
            "Gemini answered with text instead of an image: \"{}\"",
            said.trim()
        ),
        (None, true) => "Gemini returned no image and no reason — retry, or try another model"
            .to_string(),
    }
}

#[async_trait]
impl ImageProvider for GeminiImage {
    fn id(&self) -> &'static str {
        super::GEMINI_IMAGE
    }
    fn display_name(&self) -> &'static str {
        "Nano Banana (Gemini, cloud)"
    }
    fn supports_seed(&self) -> bool {
        false
    }
    fn native_alpha(&self) -> bool {
        false
    }
    fn configured(&self) -> bool {
        self.api_key.as_ref().is_some_and(|k| !k.is_empty())
    }

    async fn generate(&self, req: &GenRequest) -> Result<GenOutput, String> {
        let key = self
            .api_key
            .as_ref()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| "Gemini API key is not set (add it in Settings).".to_string())?;

        // No transparent output — subjects get the magenta chroma-key background.
        let (mut prompt, bg) = if req.transparent {
            (
                format!("{}{}", req.prompt, super::chroma_instruction()),
                super::BgMode::Chroma,
            )
        } else {
            (req.prompt.clone(), super::BgMode::Opaque)
        };
        if !req.negative.trim().is_empty() {
            // Gemini has no negative-prompt field; fold it into the instruction.
            prompt = format!("{prompt}\n\nStrictly AVOID all of the following: {}", req.negative);
        }

        let mut parts: Vec<serde_json::Value> = Vec::new();
        if !req.references.is_empty() {
            prompt = format!("{STYLE_REF_CLAUSE}{prompt}");
            for bytes in &req.references {
                parts.push(json!({
                    "inline_data": {
                        "mime_type": "image/png",
                        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                    }
                }));
            }
        }
        parts.push(json!({ "text": prompt }));

        let body = json!({
            "contents": [{ "parts": parts }],
            "generationConfig": {
                "responseModalities": ["TEXT", "IMAGE"],
                "imageConfig": { "aspectRatio": nearest_aspect(req.width, req.height) },
            },
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            self.model
        );
        let resp = reqwest::Client::new()
            .post(&url)
            .header("x-goog-api-key", key)
            .json(&body)
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| format!("Gemini request failed: {e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("read response failed: {e}"))?;
        if !status.is_success() {
            let trunc: String = text.chars().take(500).collect();
            return Err(format!("Gemini error {status}: {trunc}"));
        }

        let parsed: GenerateResponse =
            serde_json::from_str(&text).map_err(|e| format!("bad Gemini response: {e}"))?;
        let b64 = parsed
            .candidates
            .iter()
            .filter_map(|c| c.content.as_ref())
            .flat_map(|c| &c.parts)
            .filter_map(|p| p.inline_data.as_ref())
            .map(|d| d.data.clone())
            .find(|d| !d.is_empty())
            .ok_or_else(|| no_image_error(&parsed))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .map_err(|e| format!("decode image failed: {e}"))?;

        // Normalize to PNG (the API may return PNG/JPEG/WebP).
        let png = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            bytes
        } else {
            let img = image::load_from_memory(&bytes)
                .map_err(|e| format!("decode Gemini image: {e}"))?;
            let mut buf = std::io::Cursor::new(Vec::new());
            img.write_to(&mut buf, image::ImageFormat::Png)
                .map_err(|e| format!("encode png: {e}"))?;
            buf.into_inner()
        };

        Ok(GenOutput { png, seed: None, background: bg })
    }
}

#[cfg(test)]
mod tests {
    use super::{nearest_aspect, no_image_error, GenerateResponse};

    fn err_for(json: &str) -> String {
        no_image_error(&serde_json::from_str::<GenerateResponse>(json).unwrap())
    }

    #[test]
    fn no_image_errors_surface_the_real_reason() {
        // Prompt-level block: the request never reached generation.
        assert_eq!(
            err_for(r#"{"promptFeedback":{"blockReason":"PROHIBITED_CONTENT"}}"#),
            "Gemini blocked the prompt (PROHIBITED_CONTENT) — reword the flagged part and retry"
        );
        // Candidate stopped for a non-STOP reason (e.g. output image safety).
        assert_eq!(
            err_for(r#"{"candidates":[{"finishReason":"IMAGE_SAFETY"}]}"#),
            "Gemini stopped without an image (finish reason IMAGE_SAFETY) — try rephrasing"
        );
        // Text-only refusal: quote what the model actually said.
        assert_eq!(
            err_for(
                r#"{"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"I can't create that image."}]}}]}"#
            ),
            "Gemini answered with text instead of an image: \"I can't create that image.\""
        );
        // Nothing at all — don't pretend to know it was safety.
        assert_eq!(
            err_for(r#"{}"#),
            "Gemini returned no image and no reason — retry, or try another model"
        );
    }

    #[test]
    fn picks_closest_supported_aspect() {
        assert_eq!(nearest_aspect(1024, 1024), "1:1");
        assert_eq!(nearest_aspect(1200, 1800), "2:3"); // mascot portrait
        assert_eq!(nearest_aspect(3200, 1800), "16:9"); // landscape canvas
        assert_eq!(nearest_aspect(1800, 3200), "9:16");
        assert_eq!(nearest_aspect(2048, 128), "21:9"); // extreme strip clamps to widest
    }
}
