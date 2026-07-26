//! Concept assistant: an OpenAI chat collaborator for brainstorming game ideas and
//! drafting a full `GameConfig` from the conversation. This is the ONE place the LLM
//! is creative — asset prompts stay deterministic (see `prompts::assemble`).

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::model::game_config::{BuyBonusMode, GameConfig, SymbolDef, SymbolRole, WinType};

pub const CHAT_MODEL: &str = "gpt-4o";
const ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

/// One chat turn (crosses the IPC boundary).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

const BRAINSTORM_SYSTEM: &str = "\
You are a slot-game concept collaborator. Help brainstorm: the world/theme, the ONE \
signature mechanic, the symbol cast (animals or objects as the world's 'classes'), and the \
aesthetic. Take the creative direction entirely from what the user brings — do not impose \
a house style. Be concrete, opinionated and brief — short replies, sharp questions, real \
suggestions. Avoid generic fantasy clichés.";

/// The blueprint JSON field spec, shared verbatim by the in-app fill (`draft_system`) and
/// the copyable external-agent prompt (`portable_draft_prompt`) so the two can't drift.
const CONFIG_JSON_SPEC: &str = "\
{\n\
  \"name\": string,\n\
  \"gameId\": string (lowercase_snake_case),\n\
  \"winType\": \"lines\"|\"ways\"|\"scatter\"|\"cluster\",\n\
  \"cols\": number (3-7), \"rows\": number (3-7),\n\
  \"symbols\": [{ \"key\": string (short slug), \"role\": \"high\"|\"low\"|\"wild\"|\"expandingWild\" (wild that expands to cover its reel — derives a second full-column symbol_<key>_expanded asset)|\"scatter\"|\"special\", \"name\": string, \"description\": string (one-sentence art direction), \"animation\": string (one-sentence motion direction for a short seamless loop — spritesheet animations cap at 24 frames ≈ 2 s; complex rigged motion is authored separately) }],\n\
  \"hasFeatureBackground\": boolean, \"hasBuyBonus\": boolean,\n\
  \"buyBonusModes\": [{ \"key\": string, \"name\": string }],\n\
  \"hasMeter\": boolean, \"meterThresholds\": number,\n\
  \"hasMystery\": boolean, \"holdAndSpin\": boolean,\n\
  \"hasMascot\": boolean (an on-screen mascot character beside the reels, like Zeus in Gates of Olympus), \"mascotDescription\": string (who the mascot is — one-sentence art direction),\n\
  \"symbolSizing\": OPTIONAL symbol fit rule { \"low\"/\"high\"/\"wild\"/\"scatter\": { \"ink\": 0-1 cell-area fraction, \"height\": 0-1 cell-height fraction, \"tolerance\": number }, \"areaWeight\": 0-1 (0=height fit, 1=pure ink), \"centroidBias\": 0-1, \"alphaFloor\": 0-255, \"safeW\": 0-1, \"safeH\": 0-1, \"canvas\": px (0 = keep source) },\n\
 \"symbolTone\": OPTIONAL value-budget bands { \"low\"/\"high\"/\"wild\"/\"scatter\": { \"min\": 0-1, \"max\": 0-1 } (median HSV value band; Process gamma-corrects into it), \"alphaFloor\": 0-255, \"ceiling\": 0-1, \"gammaLo\"/\"gammaHi\": clamp },\n\
  \"scene\": OPTIONAL scene-asset system { \"presets\": [{ \"key\", \"width\", \"height\" }], \"guides\": [{ \"key\", \"preset\", \"zones\": [{ \"label\", \"x\", \"y\", \"w\", \"h\" (percent 0-100) }] }], \"assets\": [{ \"key\", \"kind\": \"plate\"|\"layer\"|\"sprite\"|\"fx\"|\"particle\", \"cutouts\": boolean (interior see-through openings, keyed to transparency), \"name\", \"description\" (shared prompt core), \"provider\" (optional), \"wrap\": boolean (tiles horizontally, adds seam check), \"placement\": { \"fit\": \"cover\"|\"anchored\", \"anchor\": [x,y 0-1 in texture], \"pos\": [x,y 0-1 of canvas], \"height\": fraction, \"parallax\": 0-1 depth multiplier, \"overscan\": fraction, \"blend\": \"add\"|\"screen\", \"fps\": number, \"loop\": boolean } (runtime scene.json entry; omit to keep the asset out of the manifest), \"variants\": [{ \"key\" (portrait/tablet emit as manifest overrides), \"preset\", \"extraPrompt\", \"placement\" (overrides) }] }], \"defaultProvider\": string, \"webpQuality\": number } — plates replace the stock backgrounds,\n\
  \"stylePrompt\": string (a rich, hand-authored aesthetic master — medium, linework, palette, lighting, and what to AVOID; this is the anti-slop art direction),\n\
  \"negativePrompt\": string (anti-slop negatives),\n\
  \"brief\": string (one line)\n\
}";

const CONFIG_FILL_RULES: &str = "\
Include 8-12 symbols with at least one wild and one scatter. Fill every field you reasonably \
can. Output JSON only, no prose.";

/// System prompt for the in-app "Fill blueprint": draft the config from the chat so far.
fn draft_system() -> String {
    format!(
        "From the conversation, output ONLY a JSON object describing a slot-game configuration, \
with exactly these fields (camelCase):\n{CONFIG_JSON_SPEC}\n{CONFIG_FILL_RULES}"
    )
}

/// A lenient draft parsed from the LLM (all optional so partial drafts are fine).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfigDraft {
    name: Option<String>,
    game_id: Option<String>,
    win_type: Option<String>,
    cols: Option<u32>,
    rows: Option<u32>,
    symbols: Option<Vec<SymbolDraft>>,
    has_feature_background: Option<bool>,
    has_buy_bonus: Option<bool>,
    buy_bonus_modes: Option<Vec<ModeDraft>>,
    has_meter: Option<bool>,
    meter_thresholds: Option<u32>,
    has_mystery: Option<bool>,
    hold_and_spin: Option<bool>,
    has_mascot: Option<bool>,
    mascot_description: Option<String>,
    scene: Option<crate::model::game_config::SceneConfig>,
    symbol_sizing: Option<crate::model::game_config::SymbolSizing>,
    symbol_tone: Option<crate::model::game_config::SymbolTone>,
    style_prompt: Option<String>,
    negative_prompt: Option<String>,
    brief: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SymbolDraft {
    key: Option<String>,
    role: Option<String>,
    name: Option<String>,
    description: Option<String>,
    animation: Option<String>,
}

impl Default for SymbolDraft {
    fn default() -> Self {
        Self {
            key: None,
            role: None,
            name: None,
            description: None,
            animation: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ModeDraft {
    key: Option<String>,
    name: Option<String>,
}

impl Default for ModeDraft {
    fn default() -> Self {
        Self { key: None, name: None }
    }
}

// ── OpenAI wire types ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct WireMsg<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatReq<'a> {
    model: &'a str,
    messages: Vec<WireMsg<'a>>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Deserialize)]
struct ChatResp {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: RespMsg,
}
#[derive(Deserialize)]
struct RespMsg {
    content: String,
}

fn norm_role(r: &str) -> &'static str {
    match r {
        "assistant" => "assistant",
        "system" => "system",
        _ => "user",
    }
}

fn build_wire<'a>(system: &'a str, messages: &'a [ChatMessage]) -> Vec<WireMsg<'a>> {
    let mut wire = Vec::with_capacity(messages.len() + 1);
    wire.push(WireMsg {
        role: "system",
        content: system,
    });
    for m in messages {
        wire.push(WireMsg {
            role: norm_role(&m.role),
            content: &m.content,
        });
    }
    wire
}

async fn call(
    api_key: &str,
    system: &str,
    messages: &[ChatMessage],
    json: bool,
) -> Result<String, String> {
    let wire = build_wire(system, messages);

    let body = ChatReq {
        model: CHAT_MODEL,
        messages: wire,
        temperature: if json { 0.7 } else { 0.9 },
        response_format: json.then_some(ResponseFormat { kind: "json_object" }),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(ENDPOINT)
        .bearer_auth(api_key)
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("OpenAI error {status}: {}", trunc(&text, 400)));
    }
    let parsed: ChatResp =
        serde_json::from_str(&text).map_err(|e| format!("bad response: {e}"))?;
    parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| "no choices returned".to_string())
}

/// A brainstorming reply (non-streaming).
pub async fn chat(api_key: &str, messages: &[ChatMessage]) -> Result<String, String> {
    call(api_key, BRAINSTORM_SYSTEM, messages, false).await
}

#[derive(Serialize)]
struct ChatReqStream<'a> {
    model: &'a str,
    messages: Vec<WireMsg<'a>>,
    temperature: f32,
    stream: bool,
}

#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}
#[derive(Deserialize)]
struct StreamChoice {
    delta: Delta,
}
#[derive(Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
}

/// Streaming brainstorm: calls `on_delta` for each token chunk as it arrives, and
/// returns the full accumulated text.
pub async fn chat_stream<F: FnMut(&str)>(
    api_key: &str,
    messages: &[ChatMessage],
    mut on_delta: F,
) -> Result<String, String> {
    let body = ChatReqStream {
        model: CHAT_MODEL,
        messages: build_wire(BRAINSTORM_SYSTEM, messages),
        temperature: 0.9,
        stream: true,
    };

    let client = reqwest::Client::new();
    let mut resp = client
        .post(ENDPOINT)
        .bearer_auth(api_key)
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("OpenAI error {status}: {}", trunc(&text, 400)));
    }

    // Parse the SSE stream (`data: {json}\n\n`, terminated by `data: [DONE]`),
    // buffering across chunk boundaries.
    let mut buf = String::new();
    let mut full = String::new();
    while let Some(bytes) = resp
        .chunk()
        .await
        .map_err(|e| format!("stream read failed: {e}"))?
    {
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(nl) = buf.find('\n') {
            let line: String = buf.drain(..=nl).collect();
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                return Ok(full);
            }
            if data.is_empty() {
                continue;
            }
            if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                if let Some(delta) = chunk.choices.into_iter().next().and_then(|c| c.delta.content) {
                    if !delta.is_empty() {
                        on_delta(&delta);
                        full.push_str(&delta);
                    }
                }
            }
        }
    }
    Ok(full)
}

/// A self-contained prompt the user can copy and paste into ANY external AI agent (its own
/// chat/context), so drafting the blueprint doesn't consume this app's assistant context.
/// The agent fills the schema and returns JSON, which `apply_json` merges back.
pub fn portable_draft_prompt(current: &GameConfig) -> String {
    // ZERO prose by design: the copied text is one commented JSON document (JSONC).
    // Every instruction and every field explanation lives in `//` comments, so nothing
    // can steer the conversation it gets pasted into. Current config values are embedded
    // as the field values — the template doubles as the current draft.
    let js = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let win = serde_json::to_string(&current.win_type).unwrap_or_else(|_| "\"lines\"".into());

    let symbols = if current.symbols.is_empty() {
        "    { \"key\": \"h1\", \"role\": \"high\", \"name\": \"\", \"description\": \"\", \"animation\": \"\" }".to_string()
    } else {
        current
            .symbols
            .iter()
            .map(|s| {
                format!(
                    "    {{ \"key\": {}, \"role\": {}, \"name\": {}, \"description\": {}, \"animation\": {} }}",
                    js(&s.key),
                    serde_json::to_string(&s.role).unwrap_or_else(|_| "\"high\"".into()),
                    js(&s.name),
                    js(&s.description),
                    js(&s.animation),
                )
            })
            .collect::<Vec<_>>()
            .join(",\n")
    };
    let modes = if current.buy_bonus_modes.is_empty() {
        String::new()
    } else {
        current
            .buy_bonus_modes
            .iter()
            .map(|m| format!("{{ \"key\": {}, \"name\": {} }}", js(&m.key), js(&m.name)))
            .collect::<Vec<_>>()
            .join(", ")
    };

    format!(
        r#"// Slot-game blueprint. Fill or adjust every field for the game being discussed.
// Values below are the current draft — keep what's good, replace the rest.
// Reply with ONLY this JSON object: strict valid JSON, all comments removed,
// no code fences, no prose before or after.
{{
  // Display name of the game.
  "name": {name},
  // Lowercase snake_case id, e.g. "babewyn_court".
  "gameId": {game_id},
  // How wins are evaluated: "lines" | "ways" | "scatter" | "cluster".
  "winType": {win},
  // Grid: reels (cols) x rows, each 3-7.
  "cols": {cols},
  "rows": {rows},
  // 8-12 symbols, at least one wild and one scatter.
  //   key:         short slug, e.g. "h1", "l3", "wild".
  //   role:        "high" | "low" | "wild" | "scatter" | "bonus" | "special".
  //   name:        display name.
  //   description: one-sentence ART direction for the still image.
  //   animation:   one-sentence MOTION direction. Spritesheet animations cap at
  //                24 frames (~2 s), so describe a short SEAMLESS LOOP — sway,
  //                glint, flicker, ripple, drift. No multi-stage sequences; complex
  //                rigged motion is authored separately in the studio.
  "symbols": [
{symbols}
  ],
  // The free-spins / feature mode gets its own background art.
  "hasFeatureBackground": {has_feature_background},
  // Can players buy feature entry? One mode entry per purchasable package,
  // e.g. [{{ "key": "fs10", "name": "10 Free Spins" }}].
  "hasBuyBonus": {has_buy_bonus},
  "buyBonusModes": [{modes}],
  // Collection-meter mechanic, and how many reward thresholds it has.
  "hasMeter": {has_meter},
  "meterThresholds": {meter_thresholds},
  // Mystery symbols (covered until revealed).
  "hasMystery": {has_mystery},
  // Hold-and-spin / respin mechanic.
  "holdAndSpin": {hold_and_spin},
  // On-screen mascot character beside the reels (like Zeus in Gates of Olympus).
  "hasMascot": {has_mascot},
  // Who the mascot is — one-sentence art direction.
  "mascotDescription": {mascot_description},
  // OPTIONAL scene-asset system: fixed-resolution background plates, transparent
  // stacking layers, and standalone set-piece sprites. When "assets" contains a
  // plate, these REPLACE the stock bg_* backgrounds.
  //   presets: named fixed canvases (defaults: landscape 2048x1152, portrait 1242x2208).
  //   guides:  named composition-zone sets per preset — labeled rects in PERCENT of
  //            the canvas (x/y/w/h 0-100), e.g. the reel frame block, HUD bands, the
  //            portrait-surviving centre strip. Rendered as verification overlays and
  //            optionally attached as a layout reference during generation.
  //   assets:  kind "plate"|"layer"|"sprite"; description = shared prompt core;
  //            variants derive "<key>_<variant>" (e.g. bg_base_landscape), each with
  //            a preset and an extraPrompt appended for that variant only.
  "scene": {scene},
  // THE style master: a rich aesthetic definition every image prompt inherits —
  // medium, linework, palette, lighting, finish, and what to AVOID. This is the
  // single aesthetic authority for the whole game; be specific and opinionated.
  "stylePrompt": {style_prompt},
  // Extra negative-prompt terms (what generations must avoid).
  "negativePrompt": {negative_prompt},
  // One-line pitch.
  "brief": {brief}
}}"#,
        name = js(&current.name),
        game_id = js(&current.game_id),
        win = win,
        cols = current.cols,
        rows = current.rows,
        symbols = symbols,
        has_feature_background = current.has_feature_background,
        has_buy_bonus = current.has_buy_bonus,
        modes = modes,
        has_meter = current.has_meter,
        meter_thresholds = current.meter_thresholds,
        has_mystery = current.has_mystery,
        hold_and_spin = current.hold_and_spin,
        has_mascot = current.has_mascot,
        mascot_description = js(&current.mascot_description),
        scene = serde_json::to_string_pretty(&current.scene)
            .map(|s| s.replace('\n', "\n  "))
            .unwrap_or_else(|_| "{}".into()),
        style_prompt = js(&current.style_prompt),
        negative_prompt = js(&current.negative_prompt),
        brief = js(&current.brief),
    )
}

/// Strip `//` line comments from JSONC, respecting string literals (and `\"` escapes) —
/// agents sometimes paste the commented template back with values filled in.
fn strip_jsonc_comments(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                // Skip to end of line (keep the newline itself).
                for c2 in chars.by_ref() {
                    if c2 == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Parse a config JSON (as produced by any agent, possibly wrapped in prose / code fences)
/// and merge it over `current`.
pub fn apply_json(current: &GameConfig, raw: &str) -> Result<GameConfig, String> {
    // Tolerate JSONC paste-backs (the porter template carries comments).
    let raw = strip_jsonc_comments(raw);
    let start = raw.find('{').ok_or("no JSON object found in the pasted text")?;
    let end = raw.rfind('}').ok_or("no JSON object found in the pasted text")?;
    if end < start {
        return Err("no JSON object found in the pasted text".into());
    }
    let json = &raw[start..=end];
    let draft: ConfigDraft = serde_json::from_str(json)
        .map_err(|e| format!("could not parse the pasted config JSON: {e}"))?;
    Ok(apply_draft(current, draft))
}

/// A structured config draft from the conversation, applied over `current`.
pub async fn fill_config(
    api_key: &str,
    messages: &[ChatMessage],
    current: &GameConfig,
) -> Result<GameConfig, String> {
    let content = call(api_key, &draft_system(), messages, true).await?;
    let draft: ConfigDraft = serde_json::from_str(&content)
        .map_err(|e| format!("could not parse config JSON: {e}; got: {}", trunc(&content, 300)))?;
    Ok(apply_draft(current, draft))
}

/// Merge a lenient draft over the current config, validating/clamping every field.
pub fn apply_draft(current: &GameConfig, d: ConfigDraft) -> GameConfig {
    let mut c = current.clone();
    if let Some(v) = d.name {
        c.name = v;
    }
    if let Some(v) = d.game_id {
        let id = sanitize_id(&v);
        if !id.is_empty() {
            c.game_id = id;
        }
    } else if c.game_id.is_empty() {
        c.game_id = sanitize_id(&c.name);
    }
    if let Some(v) = d.win_type {
        c.win_type = parse_win_type(&v).unwrap_or(c.win_type);
    }
    if let Some(v) = d.cols {
        c.cols = v.clamp(1, 10);
    }
    if let Some(v) = d.rows {
        c.rows = v.clamp(1, 10);
    }
    if let Some(syms) = d.symbols {
        let mapped: Vec<SymbolDef> = syms
            .into_iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let name = s.name.clone().unwrap_or_default();
                let key = s
                    .key
                    .map(|k| sanitize_id(&k))
                    .filter(|k| !k.is_empty())
                    .unwrap_or_else(|| format!("sym{}", i + 1));
                if name.is_empty() && s.description.is_none() {
                    return None;
                }
                Some(SymbolDef {
                    key,
                    role: s
                        .role
                        .and_then(|r| parse_role(&r))
                        .unwrap_or(SymbolRole::High),
                    name,
                    description: s.description.unwrap_or_default(),
                    animation: s.animation.unwrap_or_default(),
                    size_nudge: 1.0,
            tone_target: None,
                })
            })
            .collect();
        if !mapped.is_empty() {
            c.symbols = mapped;
        }
    }
    if let Some(v) = d.has_feature_background {
        c.has_feature_background = v;
    }
    if let Some(v) = d.has_buy_bonus {
        c.has_buy_bonus = v;
    }
    if let Some(modes) = d.buy_bonus_modes {
        c.buy_bonus_modes = modes
            .into_iter()
            .enumerate()
            .map(|(i, m)| BuyBonusMode {
                key: m
                    .key
                    .map(|k| sanitize_id(&k))
                    .filter(|k| !k.is_empty())
                    .unwrap_or_else(|| format!("mode{}", i + 1)),
                name: m.name.unwrap_or_default(),
            })
            .collect();
    }
    if let Some(v) = d.has_meter {
        c.has_meter = v;
    }
    if let Some(v) = d.meter_thresholds {
        c.meter_thresholds = v.clamp(0, 20);
    }
    if let Some(v) = d.has_mystery {
        c.has_mystery = v;
    }
    if let Some(v) = d.hold_and_spin {
        c.hold_and_spin = v;
    }
    if let Some(v) = d.has_mascot {
        c.has_mascot = v;
    }
    if let Some(v) = d.mascot_description {
        c.mascot_description = v.trim().to_string();
    }
    if let Some(v) = d.scene {
        c.scene = v;
    }
    if let Some(v) = d.symbol_sizing {
        c.symbol_sizing = v;
    }
    if let Some(v) = d.symbol_tone {
        c.symbol_tone = v;
    }
    if let Some(v) = d.style_prompt {
        if !v.trim().is_empty() {
            c.style_prompt = v;
        }
    }
    if let Some(v) = d.negative_prompt {
        if !v.trim().is_empty() {
            c.negative_prompt = v;
        }
    }
    if let Some(v) = d.brief {
        c.brief = v;
    }
    c
}

fn parse_win_type(s: &str) -> Option<WinType> {
    match s.to_lowercase().as_str() {
        "lines" => Some(WinType::Lines),
        "ways" => Some(WinType::Ways),
        "scatter" => Some(WinType::Scatter),
        "cluster" => Some(WinType::Cluster),
        _ => None,
    }
}

fn parse_role(s: &str) -> Option<SymbolRole> {
    match s.to_lowercase().as_str() {
        "high" => Some(SymbolRole::High),
        "low" => Some(SymbolRole::Low),
        "wild" => Some(SymbolRole::Wild),
        "scatter" => Some(SymbolRole::Scatter),
        "bonus" => Some(SymbolRole::Bonus),
        "special" => Some(SymbolRole::Special),
        "expandingwild" | "expanding_wild" | "expanding wild" => Some(SymbolRole::ExpandingWild),
        _ => None,
    }
}

/// Sanitize into a folder-safe slug: lowercase, non-alnum → `_`, collapse repeats.
fn sanitize_id(s: &str) -> String {
    let mut out = String::new();
    let mut prev_us = false;
    for ch in s.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_us = false;
        } else if ch == '-' {
            out.push('-');
            prev_us = false;
        } else if !prev_us && !out.is_empty() {
            out.push('_');
            prev_us = true;
        }
    }
    out.trim_matches('_').to_string()
}

fn trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> GameConfig {
        GameConfig {
            game_id: String::new(),
            name: String::new(),
            brief: String::new(),
            style_prompt: String::new(),
            negative_prompt: "keep this".into(),
            win_type: WinType::Lines,
            cols: 5,
            rows: 3,
            symbols: vec![],
            has_feature_background: true,
            has_buy_bonus: false,
            buy_bonus_modes: vec![],
            has_meter: false,
            meter_thresholds: 0,
            has_mystery: false,
            hold_and_spin: false,
            has_mascot: false,
            mascot_description: String::new(),
            symbol_sizing: Default::default(),
            symbol_tone: Default::default(),
            symbol_provider: String::new(),
            scene: Default::default(),
        }
    }

    #[test]
    fn applies_and_validates_draft() {
        let json = r#"{
            "name": "Hollow Orbit",
            "winType": "cluster",
            "cols": 99, "rows": 5,
            "symbols": [
                {"role":"high","name":"Captain","description":"a dead captain still at his post"},
                {"key":"WILD 1","role":"wild","name":"Anomaly"}
            ],
            "hasBuyBonus": true,
            "stylePrompt": "cold retro-futurist decay",
            "negativePrompt": ""
        }"#;
        let d: ConfigDraft = serde_json::from_str(json).unwrap();
        let c = apply_draft(&base(), d);

        assert_eq!(c.name, "Hollow Orbit");
        assert_eq!(c.game_id, "hollow_orbit"); // derived + sanitized from name
        assert!(matches!(c.win_type, WinType::Cluster));
        assert_eq!(c.cols, 10); // clamped from 99
        assert_eq!(c.symbols.len(), 2);
        assert_eq!(c.symbols[0].key, "sym1"); // no key given -> indexed
        assert_eq!(c.symbols[1].key, "wild_1"); // sanitized
        assert!(matches!(c.symbols[1].role, SymbolRole::Wild));
        assert_eq!(c.style_prompt, "cold retro-futurist decay");
        assert_eq!(c.negative_prompt, "keep this"); // empty draft field preserved
    }

    #[test]
    fn portable_prompt_is_pure_commented_json() {
        let mut cfg = base();
        cfg.name = "Test \"Game\"".into(); // quotes must survive escaping
        cfg.symbols.push(crate::model::game_config::SymbolDef {
            key: "h1".into(),
            role: SymbolRole::High,
            name: "Hollow King".into(),
            description: "a crowned skull".into(),
            animation: "the crown glints once".into(),
            size_nudge: 1.0,
            tone_target: None,
        });
        let prompt = portable_draft_prompt(&cfg);

        // Every non-empty line is either a comment or part of the JSON — zero prose.
        for line in prompt.lines() {
            let t = line.trim();
            assert!(
                t.is_empty()
                    || t.starts_with("//")
                    || t.starts_with('{')
                    || t.starts_with('}')
                    || t.starts_with('"')
                    || t.starts_with("],")
                    || t.starts_with(']'),
                "prose leaked into the porter prompt: {t:?}"
            );
        }
        // Stripping comments yields strict, parseable JSON carrying the current draft.
        let stripped = strip_jsonc_comments(&prompt);
        let v: serde_json::Value = serde_json::from_str(stripped.trim()).unwrap();
        assert_eq!(v["name"], "Test \"Game\"");
        assert_eq!(v["symbols"][0]["animation"], "the crown glints once");
        // And the template itself round-trips through apply_json (JSONC tolerated).
        let applied = apply_json(&base(), &prompt).unwrap();
        assert_eq!(applied.symbols[0].animation, "the crown glints once");
    }

    #[test]
    fn comment_stripper_respects_strings() {
        let s = r#"{ "url": "https://x/a", "note": "a // not a comment" } // trailing"#;
        let out = strip_jsonc_comments(s);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["url"], "https://x/a");
        assert_eq!(v["note"], "a // not a comment");
    }
}
