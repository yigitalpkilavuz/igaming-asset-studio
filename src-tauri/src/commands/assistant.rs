//! Concept-assistant commands (LLM brainstorm + fill config). Uses the OpenAI key.

use tauri::Emitter;

use crate::assistant::{self, ChatMessage};
use crate::model::game_config::GameConfig;
use crate::secrets;

/// Tauri event carrying one streamed token chunk of an assistant reply.
pub const CHUNK_EVENT: &str = "assistant://chunk";

fn require_key() -> Result<String, String> {
    secrets::get(secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())
}

/// Brainstorm reply for a conversation (non-streaming).
#[tauri::command]
#[specta::specta]
pub async fn assistant_chat(messages: Vec<ChatMessage>) -> Result<String, String> {
    let key = require_key()?;
    assistant::chat(&key, &messages).await
}

/// Streaming brainstorm: emits `assistant://chunk` events with token deltas as they
/// arrive, and returns the full reply when done.
#[tauri::command]
#[specta::specta]
pub async fn assistant_chat_stream(
    app: tauri::AppHandle,
    messages: Vec<ChatMessage>,
) -> Result<String, String> {
    let key = require_key()?;
    let app2 = app.clone();
    assistant::chat_stream(&key, &messages, move |delta| {
        let _ = app2.emit(CHUNK_EVENT, delta.to_string());
    })
    .await
}

/// Draft a full config from the conversation, merged over the current config.
#[tauri::command]
#[specta::specta]
pub async fn assistant_fill_config(
    messages: Vec<ChatMessage>,
    current: GameConfig,
) -> Result<GameConfig, String> {
    let key = require_key()?;
    assistant::fill_config(&key, &messages, &current).await
}

/// A copyable prompt for filling the blueprint in an EXTERNAL agent (no API key needed).
#[tauri::command]
#[specta::specta]
pub fn config_draft_prompt(current: GameConfig) -> String {
    assistant::portable_draft_prompt(&current)
}

/// Apply a config JSON pasted back from an external agent, merged over the current config.
#[tauri::command]
#[specta::specta]
pub fn apply_config_json(current: GameConfig, json: String) -> Result<GameConfig, String> {
    assistant::apply_json(&current, &json)
}
