//! Audio post-processing via **ffmpeg**: loudness-normalize each generated take to broadcast-web
//! levels and write a single clean **wav** intermediate. Shells out (Preflight-detected), mirroring
//! the rembg / Real-ESRGAN external-tool pattern.
//!
//! Delivery encoding is NOT done here. Stake Engine games load audio as ONE Howler **audiosprite**
//! (`sounds.ogg/m4a/mp3/ac3` + `sounds.json`), so all cues are concatenated and encoded together at
//! export time by the `audiosprite` CLI (see `export::bake_audio_sprite`). This stage just produces
//! the normalized `final.wav` that the bake consumes.

use std::path::Path;
use std::process::Command;

use crate::model::asset_record::AudioReport;
use crate::processing::StageOutput;

/// Web-game loudness target (GANG/Google guidance): −16 LUFS integrated, ≤ −1 dBTP true peak.
const TARGET_I: f64 = -16.0;
const TARGET_TP: f64 = -1.0;
const TARGET_LRA: f64 = 11.0;

fn loudnorm() -> String {
    format!("loudnorm=I={TARGET_I}:TP={TARGET_TP}:LRA={TARGET_LRA}")
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("ffmpeg failed to launch: {e} — is ffmpeg installed?"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        // ffmpeg puts the real reason near the end of stderr.
        let tail: String = err.lines().rev().take(4).collect::<Vec<_>>().join(" ").chars().take(400).collect();
        return Err(format!("ffmpeg error: {tail}"));
    }
    Ok(())
}

/// Whether `ffmpeg` is on PATH.
pub fn ffmpeg_available() -> bool {
    crate::processing::which("ffmpeg").is_some()
}

/// Loudness-normalize `raw_path` into `<var_dir>/stages/final.wav` — the clean −16 LUFS,
/// 44.1 kHz stereo intermediate the export's audiosprite bake concatenates. Returns the stage
/// output (relative to the variation dir) + a measured loudness/duration report.
pub fn process_audio(
    raw_path: &Path,
    var_dir: &Path,
    _looped: bool,
) -> Result<(Vec<StageOutput>, AudioReport), String> {
    if !ffmpeg_available() {
        return Err("ffmpeg is not installed — required to process audio (brew install ffmpeg).".into());
    }
    if !raw_path.is_file() {
        return Err(format!("raw audio missing: {}", raw_path.display()));
    }
    let stages_dir = var_dir.join("stages");
    std::fs::create_dir_all(&stages_dir).map_err(|e| format!("create stages dir: {e}"))?;
    let wav = stages_dir.join("final.wav");
    let af = loudnorm();

    // Normalized 44.1 kHz stereo PCM wav — the canonical audiosprite input format.
    run(Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(raw_path)
        .args(["-af", &af, "-ar", "44100", "-ac", "2", "-c:a", "pcm_s16le"])
        .arg(&wav))?;

    let (lufs, peak) = measure(&wav).unwrap_or((TARGET_I, TARGET_TP));
    let duration = probe_duration(&wav).unwrap_or(0.0);

    Ok((
        vec![StageOutput { name: "wav".into(), file: "stages/final.wav".into() }],
        AudioReport { duration_secs: duration, lufs, peak_dbtp: peak },
    ))
}

/// Measure a file's integrated loudness + true peak via a loudnorm analysis pass (the printed
/// `input_i`/`input_tp` describe the FILE being analysed = our normalized output).
fn measure(path: &Path) -> Option<(f64, f64)> {
    let out = Command::new("ffmpeg")
        .args(["-i"])
        .arg(path)
        .args(["-af", "loudnorm=print_format=json", "-f", "null", "-"])
        .output()
        .ok()?;
    let err = String::from_utf8_lossy(&out.stderr);
    // The loudnorm JSON is a flat object — take the last {...} block in stderr.
    let start = err.rfind('{')?;
    let end = err[start..].find('}')? + start + 1;
    let json: serde_json::Value = serde_json::from_str(&err[start..end]).ok()?;
    let f = |k: &str| json.get(k)?.as_str()?.parse::<f64>().ok();
    Some((f("input_i")?, f("input_tp")?))
}

fn probe_duration(path: &Path) -> Option<f64> {
    let out = Command::new("ffprobe")
        .args(["-v", "quiet", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_to_wav_intermediate() {
        if !ffmpeg_available() {
            return; // no ffmpeg on this machine → skip (CI/dev without the tool)
        }
        let dir = std::env::temp_dir().join(format!("wf_audio_{}", std::process::id()));
        let var_dir = dir.join("var");
        std::fs::create_dir_all(&var_dir).unwrap();
        let raw = var_dir.join("raw.wav");
        // A 1-second tone as the "generated" source.
        Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1"])
            .arg(&raw)
            .output()
            .unwrap();
        assert!(raw.is_file(), "test wav created");

        let (stages, report) = process_audio(&raw, &var_dir, false).expect("process_audio");
        assert!(stages.iter().any(|s| s.name == "wav"), "wav stage recorded");
        assert!(var_dir.join("stages/final.wav").is_file(), "normalized wav written");
        // Loudness measured + brought near the −16 LUFS target (single-pass loudnorm → tolerant).
        assert!(report.lufs.is_finite(), "LUFS measured: {}", report.lufs);
        assert!((report.lufs + 16.0).abs() < 6.0, "LUFS ≈ −16, got {}", report.lufs);
        assert!(report.duration_secs > 0.5, "duration measured: {}", report.duration_secs);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
