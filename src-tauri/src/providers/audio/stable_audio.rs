//! Stability AI "Stable Audio" — text-to-audio for music beds AND sound effects.
//!
//! Gambling-safe: Stability's Acceptable Use Policy has no real-money-gambling ban, the model is
//! trained on licensed data, and you own the output. Endpoint/params follow Stability's v2beta
//! audio API; if a request 400s, verify them against the current docs (the model/route naming
//! shifts between `stable-audio-2` / `2.5`).

use async_trait::async_trait;

use super::{AudioOutput, AudioProvider, AudioRequest};

const ENDPOINT: &str = "https://api.stability.ai/v2beta/audio/stable-audio-2/text-to-audio";

pub struct StableAudio {
    api_key: Option<String>,
    model: String,
}

impl StableAudio {
    pub fn new(api_key: Option<String>, model: String) -> Self {
        Self {
            api_key: api_key.filter(|k| !k.is_empty()),
            model,
        }
    }
}

#[async_trait]
impl AudioProvider for StableAudio {
    fn id(&self) -> &'static str {
        super::STABLE_AUDIO
    }
    fn display_name(&self) -> &'static str {
        "Stable Audio"
    }
    fn configured(&self) -> bool {
        self.api_key.is_some()
    }

    async fn generate(&self, req: &AudioRequest) -> Result<AudioOutput, String> {
        let key = self
            .api_key
            .as_deref()
            .ok_or("Stability API key is not set (add it in Settings).")?;
        // Stable Audio tops out at ~3 minutes; keep the request in range.
        let secs = req.target_secs.clamp(1.0, 180.0);
        let mut prompt = req.prompt.clone();
        if req.looped {
            prompt.push_str(" Seamless loop, no fade in or out.");
        }

        let mut form = reqwest::multipart::Form::new()
            .text("prompt", prompt)
            .text("output_format", "wav")
            .text("duration", format!("{secs:.0}"))
            .text("model", self.model.clone());
        if let Some(seed) = req.seed {
            form = form.text("seed", seed.to_string());
        }

        let resp = reqwest::Client::new()
            .post(ENDPOINT)
            .bearer_auth(key)
            .header("accept", "audio/*")
            .multipart(form)
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| format!("Stable Audio request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let trunc: String = body.chars().take(400).collect();
            return Err(format!("Stable Audio error {status}: {trunc}"));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("read audio failed: {e}"))?
            .to_vec();
        if bytes.is_empty() {
            return Err("Stable Audio returned no audio bytes.".into());
        }
        Ok(AudioOutput {
            bytes,
            format: "wav".into(),
            duration_secs: secs,
        })
    }
}
