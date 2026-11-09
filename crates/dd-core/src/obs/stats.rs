//! The numbers of a recording from obs-websocket, sampled once a second: `GetStats` (OBS's rendering, cumulative
//! since OBS started: the app takes deltas against a baseline before `StartRecord`), `GetRecordStatus` (duration and
//! bytes) and `GetOutputStatus("adv_file_output")` (the record output's own frame counters). From them: Mb/s, GB per
//! hour, minutes left on the drive, and the quality test's verdict (A-obs-quality.md 4.2, 4.3).
//!
//! What the 2026-11-09 runs on Joe's PC showed (results/03-obs-capture-*.json):
//! - `GetStats`' output counters start again with each recording, so a baseline taken before `StartRecord` of the
//!   second recording in one OBS session gives negative deltas. The record output's own counters are right every
//!   time; `GetStats` only stands in when they are missing and its delta makes sense, else the counter is unknown,
//!   which is no failure (the record script failed such a run though its recording was fine).
//! - `GetRecordStatus.outputBytes` keeps counting over the recordings of one session (721 MB 0.7 s into the second
//!   recording), so the rate comes from the bytes between the first and the last sample.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::obs::tiers::{Canvas, Tier};

/// The test's limits: at most 0.1 % frames skipped in rendering and in the encoder, at least 59.5 fps on average,
/// at most 8 ms to render a frame, a capture brighter than black.
pub const MAX_SKIPPED: f64 = 0.001;
pub const MIN_FPS: f64 = 59.5;
pub const MAX_RENDER_MS: f64 = 8.0;
/// Mean luma (0-255) above which the game's image is not black; the record script's threshold.
pub const MIN_LUMA: f64 = 8.0;
/// Kept free on the drive besides the recording, and the free space at which a recording stops by itself.
pub const RESERVE_BYTES: u64 = 5_000_000_000;
pub const STOP_BELOW_BYTES: u64 = 2_000_000_000;
/// What a recording is planned with before the quality test measured this PC, at 1080p60 HEVC CQP 20: the ladder
/// re-encode gave 50.5 Mb/s at QP 20, and OBS wrote ~15 % more than that re-encode at 16 (95 against 82 Mb/s on
/// 2026-11-09), so 60 with some margin. Change with the CQP constants of `tiers`.
pub const ESTIMATED_MBPS_1080P: f64 = 60.0;

/// `GetStats`, the fields the app uses.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub active_fps: f64,
    /// ms
    pub average_frame_render_time: f64,
    pub render_skipped_frames: u64,
    pub render_total_frames: u64,
    pub output_skipped_frames: u64,
    pub output_total_frames: u64,
    /// MiB free on the recording's drive
    pub available_disk_space: f64,
}

/// `GetRecordStatus`
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RecordStatus {
    pub output_active: bool,
    /// ms
    pub output_duration: u64,
    pub output_bytes: u64,
}

/// `GetOutputStatus` of `adv_file_output`
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OutputStatus {
    pub output_skipped_frames: u64,
    pub output_total_frames: u64,
}

/// One second of a recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub stats: Stats,
    pub record: RecordStatus,
    /// None when OBS did not answer for the output
    pub output: Option<OutputStatus>,
}

/// A recording so far.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Recorded seconds
    pub seconds: f64,
    /// Bytes written (estimated from the rate until the file is final)
    pub bytes: u64,
    /// None until two samples are a moment apart
    pub mbps: Option<f64>,
    pub fps_avg: f64,
    pub fps_min: f64,
    /// Mean of OBS's average render time, ms
    pub render_ms: f64,
    pub render_skipped: u64,
    pub render_total: u64,
    /// None when the encoder's counters are unknown
    pub output_skipped: Option<u64>,
    pub output_total: Option<u64>,
    /// The least free space seen on the drive
    pub free_bytes: u64,
}

/// The test's criteria, each None when unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checks {
    pub render_skipped: Option<bool>,
    pub output_skipped: Option<bool>,
    pub fps: bool,
    pub render_time: bool,
    pub capture_not_black: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Pass,
    /// The encoder dropped frames: a lower tier helps.
    EncodingLag,
    /// OBS's rendering dropped frames: the game takes the card; no tier helps.
    RenderingLag,
    /// The game capture shows nothing: no replay running, or the hook failed.
    CaptureBlack,
    /// Too few numbers to say.
    Unknown,
}

const MIB: f64 = 1024.0 * 1024.0;

pub fn mbps(bytes: u64, seconds: f64) -> Option<f64> {
    (seconds > 0.0).then(|| bytes as f64 * 8.0 / seconds / 1e6)
}

pub fn gb_per_hour(mbps: f64) -> f64 {
    mbps * 3600.0 / 8.0 / 1000.0
}

/// Minutes until the recording stops by itself at `mbps` on a drive with `free_bytes`.
pub fn minutes_left(free_bytes: u64, mbps: f64) -> Option<u64> {
    (mbps > 0.0).then(|| (free_bytes.saturating_sub(STOP_BELOW_BYTES) as f64 * 8.0 / (mbps * 1e6) / 60.0) as u64)
}

/// The space a recording of `seconds` at `mbps` needs: 20 % over the rate, plus the reserve (A-obs-quality.md 4.3).
pub fn bytes_needed(mbps: f64, seconds: f64) -> u64 {
    (mbps * 1e6 / 8.0 * seconds * 1.2) as u64 + RESERVE_BYTES
}

/// The rate to plan with before the test measured one: the 1080p estimate scaled by the output's pixels.
pub fn estimated_mbps(tier: Tier, canvas: Canvas) -> f64 {
    let output = tier.output(canvas);
    ESTIMATED_MBPS_1080P * f64::from(output.width) * f64::from(output.height) / (1920.0 * 1080.0)
}

/// The recording so far from the baseline before `StartRecord` and the samples since; None without samples.
pub fn summarize(baseline: &Stats, samples: &[Sample]) -> Option<Summary> {
    let (first, last) = (samples.first()?, samples.last()?);
    let seconds = last.record.output_duration as f64 / 1000.0;
    let span_bytes = last.record.output_bytes.saturating_sub(first.record.output_bytes);
    let rate = mbps(span_bytes, last.record.output_duration.saturating_sub(first.record.output_duration) as f64 / 1000.0);
    let bytes = span_bytes + rate.map_or(0, |r| (r * 1e6 / 8.0 * first.record.output_duration as f64 / 1000.0) as u64);
    let fps: Vec<f64> = samples.iter().map(|s| s.stats.active_fps).collect();
    let (output_skipped, output_total) = match last.output.filter(|o| o.output_total_frames > 0) {
        Some(o) => (Some(o.output_skipped_frames), Some(o.output_total_frames)),
        None => {
            let total = last.stats.output_total_frames.checked_sub(baseline.output_total_frames).filter(|&t| t > 0);
            let skipped = last.stats.output_skipped_frames.checked_sub(baseline.output_skipped_frames);
            match (skipped, total) {
                (Some(s), Some(t)) => (Some(s), Some(t)),
                _ => (None, None),
            }
        }
    };
    Some(Summary {
        seconds,
        bytes,
        mbps: rate,
        fps_avg: fps.iter().sum::<f64>() / fps.len() as f64,
        fps_min: fps.iter().copied().fold(f64::INFINITY, f64::min),
        render_ms: samples.iter().map(|s| s.stats.average_frame_render_time).sum::<f64>() / samples.len() as f64,
        render_skipped: last.stats.render_skipped_frames.saturating_sub(baseline.render_skipped_frames),
        render_total: last.stats.render_total_frames.saturating_sub(baseline.render_total_frames),
        output_skipped,
        output_total,
        free_bytes: (samples.iter().map(|s| s.stats.available_disk_space).fold(f64::INFINITY, f64::min) * MIB) as u64,
    })
}

fn share_ok(skipped: u64, total: u64) -> Option<bool> {
    (total > 0).then(|| skipped as f64 / total as f64 <= MAX_SKIPPED)
}

/// The test's verdict on a recording and the screenshot check.
pub fn judge(summary: &Summary, capture_not_black: Option<bool>) -> (Checks, Verdict) {
    let checks = Checks {
        render_skipped: share_ok(summary.render_skipped, summary.render_total),
        output_skipped: summary.output_skipped.zip(summary.output_total).and_then(|(s, t)| share_ok(s, t)),
        fps: summary.fps_avg >= MIN_FPS,
        render_time: summary.render_ms <= MAX_RENDER_MS,
        capture_not_black,
    };
    let verdict = if capture_not_black == Some(false) {
        Verdict::CaptureBlack
    } else if checks.output_skipped == Some(false) {
        Verdict::EncodingLag
    } else if checks.render_skipped == Some(false) || !checks.fps || !checks.render_time {
        Verdict::RenderingLag
    } else if checks.render_skipped.is_none() {
        Verdict::Unknown
    } else {
        Verdict::Pass
    };
    (checks, verdict)
}

/// The bytes of a `data:image/...;base64,` address (`GetSourceScreenshot`'s `imageData`).
pub fn data_uri_bytes(uri: &str) -> Option<Vec<u8>> {
    STANDARD.decode(uri.split_once(";base64,")?.1).ok()
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The mean luma (Rec. 709 weights, 0-255) of an uncompressed 24 or 32 bit BMP, the screenshot format the app asks
/// OBS for: it needs no decoder. None for anything else.
pub fn bmp_mean_luma(bmp: &[u8]) -> Option<f64> {
    if bmp.get(..2)? != b"BM" {
        return None;
    }
    let pixels = u32_at(bmp, 10)? as usize;
    let width = u32_at(bmp, 18)? as i32;
    let height = u32_at(bmp, 22)? as i32;
    let bpp = u16::from_le_bytes(bmp.get(28..30)?.try_into().ok()?);
    let compression = u32_at(bmp, 30)?;
    let bgr_masks = compression == 3 && bpp == 32 && (u32_at(bmp, 54)?, u32_at(bmp, 58)?, u32_at(bmp, 62)?) == (0x00FF_0000, 0x0000_FF00, 0x0000_00FF);
    if width <= 0 || height == 0 || !(bpp == 24 || bpp == 32) || !(compression == 0 || bgr_masks) {
        return None;
    }
    let (width, rows) = (width as usize, height.unsigned_abs() as usize);
    let step = usize::from(bpp / 8);
    let stride = (width * step).div_ceil(4) * 4;
    let mut sum = 0.0;
    for row in 0..rows {
        let line = bmp.get(pixels + row * stride..pixels + row * stride + width * step)?;
        for px in line.chunks_exact(step) {
            sum += 0.0722 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.2126 * f64::from(px[2]);
        }
    }
    Some(sum / (width * rows) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Samples as results/03-obs-capture-*.json keeps them: deltas against the baseline before `StartRecord`.
    fn samples(baseline: &Stats, json: &str) -> Vec<Sample> {
        let rows: Vec<Value> = serde_json::from_str(json).unwrap();
        rows.iter()
            .map(|r| {
                let n = |k: &str| r[k].as_f64().unwrap();
                Sample {
                    stats: Stats {
                        active_fps: n("activeFps"),
                        average_frame_render_time: n("avgRenderMs"),
                        render_skipped_frames: (baseline.render_skipped_frames as f64 + n("renderSkipped")) as u64,
                        render_total_frames: (baseline.render_total_frames as f64 + n("renderTotal")) as u64,
                        output_skipped_frames: (baseline.output_skipped_frames as f64 + n("outputSkipped")) as u64,
                        output_total_frames: (baseline.output_total_frames as f64 + n("outputTotal")) as u64,
                        available_disk_space: n("diskMB"),
                    },
                    record: RecordStatus { output_active: r["recActive"].as_bool().unwrap(), output_duration: n("recDurationMs") as u64, output_bytes: n("recBytes") as u64 },
                    output: r.get("fileOutTotal").map(|_| OutputStatus { output_skipped_frames: n("fileOutSkipped") as u64, output_total_frames: n("fileOutTotal") as u64 }),
                }
            })
            .collect()
    }

    /// results/03-obs-capture-dlaa-1080p-20261109-205758.json, the second recording of the OBS session: its first
    /// three and last two samples. The record script failed it on the encoder counters; the file had 3594 frames.
    const DLAA: &str = r#"[
        {"t":1.02,"activeFps":60.0,"renderSkipped":0,"renderTotal":61,"outputSkipped":0,"outputTotal":-3556,"avgRenderMs":0.384,"diskMB":37502,"recBytes":721211497,"recDurationMs":666,"recActive":true,"fileOutSkipped":0,"fileOutTotal":40},
        {"t":2.01,"activeFps":60.0,"renderSkipped":0,"renderTotal":120,"outputSkipped":0,"outputTotal":-3496,"avgRenderMs":0.488,"diskMB":37502,"recBytes":735804368,"recDurationMs":1649,"recActive":true,"fileOutSkipped":0,"fileOutTotal":100},
        {"t":3.01,"activeFps":60.0,"renderSkipped":0,"renderTotal":180,"outputSkipped":0,"outputTotal":-3436,"avgRenderMs":0.515,"diskMB":37472,"recBytes":750807326,"recDurationMs":2649,"recActive":true,"fileOutSkipped":0,"fileOutTotal":160},
        {"t":59.02,"activeFps":60.0,"renderSkipped":0,"renderTotal":3541,"outputSkipped":0,"outputTotal":-75,"avgRenderMs":0.471,"diskMB":36824,"recBytes":1433103593,"recDurationMs":58666,"recActive":true,"fileOutSkipped":0,"fileOutTotal":3520},
        {"t":60.01,"activeFps":60.0,"renderSkipped":0,"renderTotal":3600,"outputSkipped":0,"outputTotal":-16,"avgRenderMs":0.474,"diskMB":36824,"recBytes":1444740560,"recDurationMs":59649,"recActive":true,"fileOutSkipped":0,"fileOutTotal":3579}
    ]"#;

    /// results/03-obs-capture-first-check-20261109-204414.json, the first recording of the session: first and last.
    const FIRST_CHECK: &str = r#"[
        {"t":1.03,"activeFps":60.0,"renderSkipped":0,"renderTotal":62,"outputSkipped":0,"outputTotal":51,"avgRenderMs":0.591,"diskMB":38231,"recBytes":6999525,"recDurationMs":683,"recActive":true,"fileOutSkipped":0,"fileOutTotal":41},
        {"t":60.02,"activeFps":60.0,"renderSkipped":0,"renderTotal":3602,"outputSkipped":0,"outputTotal":3591,"avgRenderMs":0.644,"diskMB":37570,"recBytes":709310930,"recDurationMs":59666,"recActive":true,"fileOutSkipped":0,"fileOutTotal":3580}
    ]"#;

    /// A baseline from before the second recording: OBS's counters after the first one (3606 frames output).
    const BASELINE: Stats = Stats { active_fps: 60.0, average_frame_render_time: 0.5, render_skipped_frames: 0, render_total_frames: 50_000, output_skipped_frames: 0, output_total_frames: 3606, available_disk_space: 37_600.0 };

    #[test]
    fn passes_the_second_recording_the_script_failed() {
        let s = samples(&BASELINE, DLAA);
        let summary = summarize(&BASELINE, &s).unwrap();
        assert_eq!((summary.render_skipped, summary.render_total), (0, 3600));
        assert_eq!((summary.output_skipped, summary.output_total), (Some(0), Some(3579)), "the record output's own counters");
        // the file: 734526678 bytes in 59.65 s = 98.5 Mb/s
        let rate = summary.mbps.unwrap();
        assert!((rate - 98.5).abs() < 1.0, "{rate}");
        assert!((summary.bytes as f64 - 734_526_678.0).abs() < 0.01 * 734_526_678.0, "{}", summary.bytes);
        assert!((summary.seconds - 59.649).abs() < 1e-9);
        assert_eq!(summary.free_bytes, (36_824.0 * MIB) as u64);
        let (checks, verdict) = judge(&summary, Some(true));
        assert_eq!(verdict, Verdict::Pass);
        assert_eq!(checks, Checks { render_skipped: Some(true), output_skipped: Some(true), fps: true, render_time: true, capture_not_black: Some(true) });
    }

    #[test]
    fn counts_unknown_encoder_counters_as_unknown_not_as_a_failure() {
        let mut s = samples(&BASELINE, DLAA);
        s.iter_mut().for_each(|sample| sample.output = None);
        let summary = summarize(&BASELINE, &s).unwrap();
        assert_eq!((summary.output_skipped, summary.output_total), (None, None), "GetStats' delta is negative here");
        let (checks, verdict) = judge(&summary, Some(true));
        assert_eq!((checks.output_skipped, verdict), (None, Verdict::Pass));
        // in the first recording of a session GetStats' delta stands in
        let fresh = Stats { output_total_frames: 0, render_total_frames: 0, ..BASELINE };
        let mut s = samples(&fresh, FIRST_CHECK);
        s.iter_mut().for_each(|sample| sample.output = None);
        let summary = summarize(&fresh, &s).unwrap();
        assert_eq!((summary.output_skipped, summary.output_total), (Some(0), Some(3591)));
        assert!((summary.mbps.unwrap() - 95.5).abs() < 1.0, "the file: 95.5 Mb/s");
        assert_eq!(judge(&summary, None).1, Verdict::Pass, "a screenshot OBS could not give is no failure");
    }

    #[test]
    fn tells_the_encoders_lag_from_the_games() {
        let fresh = Stats::default();
        let base = summarize(&fresh, &samples(&fresh, FIRST_CHECK)).unwrap();
        let encoder = Summary { output_skipped: Some(30), output_total: Some(900), ..base.clone() };
        assert_eq!(judge(&encoder, Some(true)).1, Verdict::EncodingLag);
        assert_eq!(judge(&Summary { output_skipped: Some(1), ..encoder.clone() }, Some(true)).1, Verdict::EncodingLag, "1 of 900 is 0.11 %");
        assert_eq!(judge(&Summary { output_skipped: Some(1), output_total: Some(3580), ..encoder.clone() }, Some(true)).1, Verdict::Pass);
        let rendering = Summary { render_skipped: 10, ..base.clone() };
        assert_eq!(judge(&rendering, Some(true)).1, Verdict::RenderingLag);
        assert_eq!(judge(&Summary { fps_avg: 57.0, ..base.clone() }, Some(true)).1, Verdict::RenderingLag);
        assert_eq!(judge(&Summary { render_ms: 9.5, ..base.clone() }, Some(true)).1, Verdict::RenderingLag);
        assert_eq!(judge(&encoder, Some(false)).1, Verdict::CaptureBlack, "black first: no tier helps");
        assert_eq!(judge(&Summary { render_total: 0, ..base }, Some(true)).1, Verdict::Unknown);
        assert_eq!(summarize(&fresh, &[]), None);
    }

    #[test]
    fn plans_the_drive() {
        // first-check: 95.5 Mb/s = 43 GB/h; 37570 MiB free: the script said 52 min (MiB as MB, no 2 GB kept)
        assert!((gb_per_hour(95.5) - 42.98).abs() < 0.01);
        let free = (37_570.0 * MIB) as u64;
        assert_eq!(minutes_left(free, 95.5), Some(52));
        assert_eq!(minutes_left(STOP_BELOW_BYTES, 95.5), Some(0));
        assert_eq!(minutes_left(free, 0.0), None);
        // a 21-minute replay at 95.5 Mb/s: 18 GB with the 20 % and the reserve
        assert_eq!(bytes_needed(95.5, 21.0 * 60.0), 23_049_500_000);
        assert_eq!(mbps(734_526_678, 59.65).map(|r| (r * 10.0).round() / 10.0), Some(98.5));
        let tier = crate::obs::tiers::start(crate::obs::tiers::gpu(crate::game_check::Vendor::Nvidia, "RTX 3070").unwrap(), Canvas { width: 2560, height: 1440 });
        assert_eq!(estimated_mbps(tier, Canvas { width: 2560, height: 1440 }), 60.0);
        assert!((estimated_mbps(crate::obs::tiers::Tier { height: 1440, ..tier }, Canvas { width: 2560, height: 1440 }) - 106.7).abs() < 0.1);
    }

    /// A BMP as Qt writes one: 24 bit, rows bottom-up, padded to 4 bytes; or 32 bit with BGRA masks.
    fn bmp(width: u32, height: u32, bpp: u16, rgb: [u8; 3]) -> Vec<u8> {
        let step = u32::from(bpp / 8);
        let stride = (width * step).div_ceil(4) * 4;
        let header = if bpp == 32 { 14 + 40 + 12 } else { 14 + 40 };
        let mut out = b"BM".to_vec();
        out.extend((header + stride * height).to_le_bytes());
        out.extend([0u8; 4]);
        out.extend(header.to_le_bytes());
        out.extend(40u32.to_le_bytes());
        out.extend(width.to_le_bytes());
        out.extend(height.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(bpp.to_le_bytes());
        out.extend((if bpp == 32 { 3u32 } else { 0 }).to_le_bytes());
        out.extend([0u8; 20]);
        if bpp == 32 {
            out.extend(0x00FF_0000u32.to_le_bytes());
            out.extend(0x0000_FF00u32.to_le_bytes());
            out.extend(0x0000_00FFu32.to_le_bytes());
        }
        for _ in 0..height {
            let mut row: Vec<u8> = (0..width).flat_map(|_| if bpp == 32 { vec![rgb[2], rgb[1], rgb[0], 255] } else { vec![rgb[2], rgb[1], rgb[0]] }).collect();
            row.resize(stride as usize, 0);
            out.extend(row);
        }
        out
    }

    #[test]
    fn sees_a_black_capture() {
        assert_eq!(bmp_mean_luma(&bmp(5, 3, 24, [0, 0, 0])), Some(0.0));
        let grey = bmp_mean_luma(&bmp(5, 3, 24, [77, 77, 77])).unwrap();
        assert!((grey - 77.0).abs() < 1e-9);
        let red = bmp_mean_luma(&bmp(4, 2, 32, [255, 0, 0])).unwrap();
        assert!((red - 0.2126 * 255.0).abs() < 1e-9);
        assert!(grey > MIN_LUMA);
        assert_eq!(bmp_mean_luma(b"\x89PNG\r\n"), None);
        assert_eq!(bmp_mean_luma(&bmp(5, 3, 24, [9, 9, 9])[..60]), None, "cut short");
        let uri = format!("data:image/bmp;base64,{}", STANDARD.encode(bmp(2, 2, 24, [200, 200, 200])));
        assert!((bmp_mean_luma(&data_uri_bytes(&uri).unwrap()).unwrap() - 200.0).abs() < 1e-9);
        assert_eq!(data_uri_bytes("no data"), None);
    }
}
