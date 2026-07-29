//! Google "Lyria" on Vertex AI — text-to-music (not SFX).
//!
//! Gambling-safe: Google's Generative-AI Prohibited Use Policy has no gambling ban. Output
//! carries an inaudible SynthID watermark. Auth is a Google Cloud OAuth **access token** pasted
//! into Settings (obtain via `gcloud auth print-access-token`); a full service-account credential
//! flow is a future enhancement. Verify the predict route + response shape against current Vertex
//! docs if it errors.

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;

use super::{AudioOutput, AudioProvider, AudioRequest};

pub struct Lyria {
    token: Option<String>,
    model: String,
    project: String,
    location: String,
}

impl Lyria {
    pub fn new(token: Option<String>, model: String, project: String, location: String) -> Self {
        Self {
            token: token.filter(|t| !t.is_empty()),
            model,
            project,
            location,
        }
    }
}

#[derive(Deserialize)]
struct PredictResp {
    #[serde(default)]
    predictions: Vec<Prediction>,
}

#[derive(Deserialize)]
struct Prediction {
    #[serde(rename = "bytesBase64Encoded")]
    bytes_base64_encoded: Option<String>,
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
}

#[async_trait]
impl AudioProvider for Lyria {
    fn id(&self) -> &'static str {
        super::LYRIA
    }
    fn display_name(&self) -> &'static str {
        "Google Lyria"
    }
    fn does_sfx(&self) -> bool {
        false // Lyria is a music model
    }
    fn configured(&self) -> bool {
        self.token.is_some() && !self.project.is_empty()
    }

    async fn generate(&self, req: &AudioRequest) -> Result<AudioOutput, String> {
        let token = self
            .token
            .as_deref()
            .ok_or("Vertex access token is not set (add it in Settings).")?;
        if self.project.is_empty() {
            return Err("Vertex project id is not set (add it in Settings).".into());
        }
        let url = format!(
            "https://{loc}-aiplatform.googleapis.com/v1/projects/{proj}/locations/{loc}/publishers/google/models/{model}:predict",
            loc = self.location,
            proj = self.project,
            model = self.model,
        );
        let body = serde_json::json!({
            "instances": [{ "prompt": req.prompt }],
            "parameters": { "sampleCount": 1, "seed": req.seed },
        });

        let resp = reqwest::Client::new()
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| format!("Lyria request failed: {e}"))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("read Lyria response failed: {e}"))?;
        if !status.is_success() {
            let trunc: String = text.chars().take(400).collect();
            return Err(format!("Lyria error {status}: {trunc}"));
        }
        let parsed: PredictResp =
            serde_json::from_str(&text).map_err(|e| format!("bad Lyria response: {e}"))?;
        let pred = parsed
            .predictions
            .into_iter()
            .next()
            .ok_or("Lyria returned no audio")?;
        let b64 = pred
            .bytes_base64_encoded
            .ok_or("Lyria prediction had no audio bytes")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("decode Lyria audio: {e}"))?;
        let format = pred
            .mime_type
            .as_deref()
            .and_then(|m| m.rsplit('/').next())
            .unwrap_or("wav")
            .to_string();
        Ok(AudioOutput {
            bytes,
            format,
            duration_secs: req.target_secs,
        })
    }
}
