//! AI depth-layer proposals: gpt-4o vision looks at the approved scene and returns the
//! requested number of depth bands, farthest first, each with SAM seed points spread
//! across every disjoint region of that band. Shares the vision HTTP plumbing with the
//! studio's part proposer (`segment::vision_json`).

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::studio::doc::{SamLabel, SamPrompt};
use crate::studio::segment::{slugify, vision_json};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LayerProposal {
    pub id: String,
    pub name: String,
    /// SAM seed prompts, normalized 0..1. Empty for the farthest (catch-all) layer.
    pub points: Vec<SamPrompt>,
    /// Suggested parallax multiplier, monotonically increasing with nearness.
    pub speed: f64,
}

const LAYERS_PROPOSE_SYSTEM: &str = "\
You are planning a parallax split of ONE 2D game background scene into depth layers. The \
scene will be separated into stacked layers that scroll at different speeds, so each layer \
must be a contiguous DEPTH BAND of the scene (by distance from the camera), NOT an object \
category. Rules:\n\
- Output exactly the requested number of layers, FARTHEST first (sky/void), NEAREST last.\n\
- The FIRST layer is the catch-all backdrop: give it an empty points array — it \
automatically receives every pixel no nearer layer claims.\n\
- Every OTHER layer: a short lowercase slug id (a-z, 0-9, underscore), a display name, and \
3-8 positive `points` placed WELL INSIDE that band's visible pixels. Each point seeds its \
own selection and the results are combined, so spread them: if the band has several \
disjoint regions (e.g. a tree on the left and rocks on the right), place at least one \
point in EVERY region, and 2+ points on large regions. Optionally add 1-2 negative points \
just outside the band on a neighbouring depth.\n\
- `speed` is the parallax multiplier per layer: 0.02-0.1 for the farthest, rising \
monotonically to 0.9-1.1 for the nearest.\n\
- Points are normalized 0..1 (x right, y down). label is \"positive\" or \"negative\".\n\
Output ONLY JSON: {\"layers\":[{\"id\":string,\"name\":string,\"speed\":number,\
\"points\":[{\"x\":number,\"y\":number,\"label\":\"positive\"|\"negative\"}]}]}";

#[derive(Deserialize)]
struct LayersDraft {
    #[serde(default)]
    layers: Vec<LayerDraft>,
}
#[derive(Deserialize)]
struct LayerDraft {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    speed: f64,
    #[serde(default)]
    points: Vec<PointDraft>,
}
#[derive(Deserialize)]
struct PointDraft {
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    label: String,
}

/// Ask gpt-4o vision for a depth-band plan. `count` is the desired layer total (2..=6);
/// `hint` carries scene context (the asset's subject / game brief).
pub async fn propose_layers(
    api_key: &str,
    source_png: &[u8],
    count: u32,
    hint: &str,
) -> Result<Vec<LayerProposal>, String> {
    let count = count.clamp(2, 6);
    let user_text = if hint.trim().is_empty() {
        format!("Split this game background into exactly {count} parallax depth layers.")
    } else {
        format!(
            "Context: {hint}\nSplit this game background into exactly {count} parallax \
depth layers."
        )
    };
    let content = vision_json(api_key, LAYERS_PROPOSE_SYSTEM, &user_text, source_png).await?;
    let draft: LayersDraft = serde_json::from_str(&content)
        .map_err(|e| format!("could not parse layer plan: {e}"))?;
    let out = clamp_layers(draft, count as usize);
    if out.len() < 2 {
        return Err("the model proposed fewer than 2 usable layers".into());
    }
    Ok(out)
}

/// Validate/clamp a raw model draft: slug ids, deduped, clamped points, monotone speeds.
fn clamp_layers(draft: LayersDraft, count: usize) -> Vec<LayerProposal> {
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<LayerProposal> = Vec::new();
    for (i, l) in draft.layers.into_iter().enumerate() {
        let slug = slugify(if l.id.trim().is_empty() { &l.name } else { &l.id });
        if slug.is_empty() {
            continue;
        }
        let mut id = slug.clone();
        let mut n = 2;
        while seen.contains(&id) {
            id = format!("{slug}_{n}");
            n += 1;
        }
        let points: Vec<SamPrompt> = l
            .points
            .iter()
            .take(8)
            .map(|pt| SamPrompt {
                x: pt.x.clamp(0.0, 1.0),
                y: pt.y.clamp(0.0, 1.0),
                label: if pt.label == "negative" { SamLabel::Negative } else { SamLabel::Positive },
            })
            .collect();
        // Every layer except the farthest needs at least one positive seed.
        if i > 0 && !points.iter().any(|p| p.label == SamLabel::Positive) {
            continue;
        }
        let name = if l.name.trim().is_empty() { id.clone() } else { l.name.trim().to_string() };
        // Speeds must rise strictly with nearness; repair drafts that don't.
        let min_speed = out.last().map(|p: &LayerProposal| p.speed + 0.05).unwrap_or(0.02);
        let speed = l.speed.clamp(0.0, 1.25).max(min_speed);
        seen.push(id.clone());
        out.push(LayerProposal {
            id,
            name,
            points: if out.is_empty() { Vec::new() } else { points },
            speed,
        });
        if out.len() >= count {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_repairs_speeds_and_strips_catch_all_points() {
        let draft = LayersDraft {
            layers: vec![
                LayerDraft {
                    id: "sky".into(),
                    name: "Sky".into(),
                    speed: 0.5, // too fast for the farthest…
                    points: vec![PointDraft { x: 0.5, y: 0.1, label: "positive".into() }],
                },
                LayerDraft {
                    id: "mid".into(),
                    name: "Mid".into(),
                    speed: 0.1, // …and non-monotone here
                    points: vec![PointDraft { x: 0.5, y: 0.6, label: "positive".into() }],
                },
                LayerDraft {
                    id: "front".into(),
                    name: "Front".into(),
                    speed: 1.0,
                    points: vec![PointDraft { x: 0.5, y: 0.9, label: "positive".into() }],
                },
            ],
        };
        let out = clamp_layers(draft, 3);
        assert_eq!(out.len(), 3);
        assert!(out[0].points.is_empty(), "catch-all keeps no points");
        assert!(out.windows(2).all(|w| w[0].speed < w[1].speed), "speeds monotone");
    }

    #[test]
    fn layer_without_positive_points_is_dropped() {
        let draft = LayersDraft {
            layers: vec![
                LayerDraft { id: "sky".into(), name: "Sky".into(), speed: 0.05, points: vec![] },
                LayerDraft {
                    id: "mid".into(),
                    name: "Mid".into(),
                    speed: 0.5,
                    points: vec![PointDraft { x: 0.5, y: 0.5, label: "negative".into() }],
                },
                LayerDraft {
                    id: "front".into(),
                    name: "Front".into(),
                    speed: 1.0,
                    points: vec![PointDraft { x: 0.5, y: 0.9, label: "positive".into() }],
                },
            ],
        };
        let out = clamp_layers(draft, 3);
        assert_eq!(out.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), vec!["sky", "front"]);
    }
}
