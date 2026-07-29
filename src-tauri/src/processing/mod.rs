//! Post-processing pipeline: raw generated image → shippable finals.
//!
//! Rust-native stages (resize/convert, 9-slice json) always run. External-tool stages
//! (rembg for background removal, Real-ESRGAN for upscale) run only when the tool is
//! detected on the system, and are skipped gracefully otherwise.

pub mod audio;
pub mod chromakey;
pub mod convert;
pub mod normalize;
pub mod tone;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::model::asset::Insets;
use crate::providers::{BgMode, CHROMA_RGB};

const UPSCALE_BIN: &str = "realesrgan-ncnn-vulkan";

/// Which external tools are available.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    /// `rembg` CLI present (background removal).
    pub rembg: bool,
    /// Real-ESRGAN upscaler present.
    pub upscale: bool,
    /// Resolved path of the upscaler binary, if found.
    pub upscale_bin: Option<String>,
    /// `ffmpeg` present (required for the audio loudness normalize pass).
    #[serde(default)]
    pub ffmpeg: bool,
    /// `audiosprite` CLI present (bakes all cues into the game's Howler audiosprite at export).
    #[serde(default)]
    pub audiosprite: bool,
    /// Whether this ffmpeg can encode Ogg/Vorbis. When false, the exported audiosprite ships
    /// m4a+mp3+ac3 (still covers every browser) but skips the `.ogg` twin.
    #[serde(default)]
    pub audio_ogg: bool,
}

/// One produced stage file (relative to the variation dir).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StageOutput {
    pub name: String,
    pub file: String,
}

/// Detect available external tools.
pub fn detect() -> Preflight {
    let upscale_bin = which(UPSCALE_BIN);
    Preflight {
        rembg: which("rembg").is_some(),
        upscale: upscale_bin.is_some(),
        upscale_bin,
        ffmpeg: which("ffmpeg").is_some(),
        audiosprite: which("audiosprite").is_some(),
        audio_ogg: ffmpeg_has_libvorbis(),
    }
}

/// Whether the ffmpeg on PATH was built with libvorbis (needed for the audiosprite `.ogg` twin).
/// `audiosprite` aborts the whole bake if a requested encoder is missing, so the export gates the
/// ogg format on this.
pub fn ffmpeg_has_libvorbis() -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("libvorbis"))
}

pub(crate) fn which(bin: &str) -> Option<String> {
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

fn run(cmd: &mut Command, label: &str) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("{label} failed to launch: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{label} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Run the full pipeline for one variation. `raw_path` is the source image; stage files
/// are written under `stages_dir`. Returns the ordered stage outputs (paths relative to
/// the variation dir) plus the visual-mass report when the normalize stage measured one.
#[allow(clippy::too_many_arguments)]
pub fn run_pipeline(
    raw_path: &Path,
    stages_dir: &Path,
    target_w: u32,
    target_h: u32,
    opaque: bool,
    bg: BgMode,
    nine_slice: Option<Insets>,
    // Normalize the subject's VISUAL MASS to the tier's target (symbols only).
    mass: Option<normalize::MassSpec>,
    // Tone-normalise the asset into its value-budget band (assets with a band).
    tone_spec: Option<tone::ToneSpec>,
    // WebP encode quality (10–100); the studio default is 85.
    webp_quality: f32,
    // Interior cut-outs: key chroma fills ANYWHERE in the image (not only a chroma
    // background) — scene elements with see-through openings.
    chroma_sweep: bool,
    pre: &Preflight,
) -> Result<
    (
        Vec<StageOutput>,
        Option<crate::model::asset_record::MassReport>,
        Option<crate::model::asset_record::ToneReport>,
    ),
    String,
> {
    fs::create_dir_all(stages_dir).map_err(|e| format!("create stages dir failed: {e}"))?;

    let mut stages: Vec<StageOutput> = Vec::new();
    let mut mass_report = None;
    let mut tone_report = None;
    let mut cur: PathBuf = raw_path.to_path_buf();

    // (Upscale is now a deliberate, separate step — see the "Upscale" command — so Process
    // keeps the source resolution and only removes the background + converts.)

    // 1. Background removal (alpha assets only) — strategy depends on how the image was
    //    generated: chroma-key (Rust-native, no tool) for chroma backgrounds, rembg for a
    //    plain opaque background, nothing when the model already produced alpha.
    if !opaque {
        match bg {
            BgMode::Chroma => {
                let bytes = fs::read(&cur).map_err(|e| format!("read for chromakey failed: {e}"))?;
                let keyed = chromakey::chroma_key(&bytes, CHROMA_RGB, 0.18)?;
                let out = stages_dir.join("nobg.png");
                fs::write(&out, &keyed).map_err(|e| format!("write nobg failed: {e}"))?;
                stages.push(stage("chromakey", "nobg.png"));
                cur = out;
            }
            BgMode::Opaque => {
                if pre.rembg {
                    let out = stages_dir.join("nobg.png");
                    run(
                        Command::new("rembg").args([
                            "i",
                            &cur.to_string_lossy(),
                            &out.to_string_lossy(),
                        ]),
                        "rembg",
                    )?;
                    stages.push(stage("rembg", "nobg.png"));
                    cur = out;
                }
            }
            BgMode::Native => {}
        }
    }

    // 1b. Interior cut-out keying: openings are generated as flat chroma fills wherever
    //     they sit in the element; key them to real transparency. Runs even after a
    //     native-alpha generation (the chroma stage above only runs for chroma
    //     BACKGROUNDS) — double keying is harmless.
    if chroma_sweep && !opaque {
        let bytes = fs::read(&cur).map_err(|e| format!("read for cutout failed: {e}"))?;
        let keyed = chromakey::chroma_key(&bytes, CHROMA_RGB, 0.18)?;
        let out = stages_dir.join("cutout.png");
        fs::write(&out, &keyed).map_err(|e| format!("write cutout failed: {e}"))?;
        stages.push(stage("cutout", "cutout.png"));
        cur = out;
    }

    // 1c. Tone pass: the value budget's enforcement — solve a gamma to the asset's
    //     declared band (median HSV value over opaque pixels) and bake it. Runs after
    //     keying (fringe must not drag the median) and before the fit pass + lossy
    //     encode. In-band assets pass through untouched.
    if let Some(spec) = tone_spec {
        let bytes = fs::read(&cur).map_err(|e| format!("read for tone failed: {e}"))?;
        let (corrected, report) = tone::tone_pass(&bytes, spec)?;
        tone_report = Some(report);
        if let Some(corrected) = corrected {
            let out = stages_dir.join("tone.png");
            fs::write(&out, &corrected).map_err(|e| format!("write tone failed: {e}"))?;
            stages.push(stage("tone", "tone.png"));
            cur = out;
        }
    }

    // 2. Symbol fit pass: blend height- and ink-based scales (the eye reads ink, not
    //    height), anchor centroid-biased, clamp to the safe box — every symbol in a
    //    class carries comparable visual weight and nothing overflows its cell.
    if let Some(spec) = mass {
        if !opaque {
            let bytes = fs::read(&cur).map_err(|e| format!("read for normalize failed: {e}"))?;
            let (normalized, report) = normalize::fit_symbol(&bytes, spec)?;
            mass_report = report;
            if let Some(normalized) = normalized {
                let out = stages_dir.join("fill.png");
                fs::write(&out, &normalized).map_err(|e| format!("write fill failed: {e}"))?;
                stages.push(stage("normalize", "fill.png"));
                cur = out;
            }
        }
    }

    // 3. Resize + convert (always).
    let bytes = fs::read(&cur).map_err(|e| format!("read stage input failed: {e}"))?;
    let outputs = convert::resize_convert(&bytes, target_w, target_h, opaque, webp_quality)?;
    fs::write(stages_dir.join("final.webp"), &outputs.webp)
        .map_err(|e| format!("write webp failed: {e}"))?;
    stages.push(stage("webp", "final.webp"));
    let sec_name = format!("final.{}", outputs.secondary_ext);
    fs::write(stages_dir.join(&sec_name), &outputs.secondary)
        .map_err(|e| format!("write secondary failed: {e}"))?;
    stages.push(stage(outputs.secondary_ext, &sec_name));

    // 4. 9-slice descriptor (when applicable).
    if let Some(insets) = nine_slice {
        let json = serde_json::json!({
            "sourceSize": { "w": target_w, "h": target_h },
            "borderTop": insets.top,
            "borderRight": insets.right,
            "borderBottom": insets.bottom,
            "borderLeft": insets.left,
        });
        let text = serde_json::to_vec_pretty(&json).map_err(|e| format!("json failed: {e}"))?;
        fs::write(stages_dir.join("final.9.json"), text)
            .map_err(|e| format!("write 9.json failed: {e}"))?;
        stages.push(stage("nineSlice", "final.9.json"));
    }

    Ok((stages, mass_report, tone_report))
}

fn stage(name: &str, filename: &str) -> StageOutput {
    StageOutput {
        name: name.to_string(),
        file: format!("stages/{filename}"),
    }
}
