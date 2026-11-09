//! The settings ladder of a recording. A tier is the encoder, its preset, the constant quality and the output size;
//! the canvas is always the game's image. The quality test starts at the top tier for the graphics card at 1080p
//! (or the game's size below it), steps down when OBS drops frames in the encoder ("P7 -> P5 -> 1440p -> 1080p ->
//! H.264", plan "Automatischer Qualitätstest") and up when a tier holds: a larger output while the game renders
//! larger (a 1440p or 4K monitor, DSR), then AV1 on cards that encode it well. Dropped frames in OBS's rendering
//! are the game's load: no encoder step helps there (A-obs-quality.md 4.2).

use serde::{Deserialize, Serialize, Serializer};

use crate::game_check::Vendor;
use crate::obs::stats::Verdict;

/// The constant quality (CQP; for Intel's HEVC and H.264 ICQ) per codec, the same for every preset and size: a
/// step down keeps it. 20, approved by Joe on 2026-11-09 from the quality ladder of a 30 s lossless reference
/// (research/2026-11-09-testabend/replay/results/04b-ladder-lossless-ref.json: HEVC QP 20 = PSNR 45.2 dB, SSIM
/// 0.988, 50 Mb/s = ~23 GB/h at 1080p; OBS's CQP 16 had given 92-98 Mb/s = ~43 GB/h, too much).
pub const HEVC_CQP: u8 = 20;
pub const H264_CQP: u8 = 20;
/// On OBS's AV1 scale (1-63; NVENC multiplies it by 4). Whether 20 matches HEVC's 20 is not measured.
pub const AV1_CQP: u8 = 20;

/// The output heights the ladder steps through. A game below 1080p records at its own size.
const HEIGHTS: [u32; 3] = [1080, 1440, 2160];

/// The graphics card's encoder block, as OBS names its encoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Hardware {
    /// NVIDIA NVENC
    Nvenc,
    /// AMD AMF
    Amf,
    /// Intel Quick Sync
    Qsv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Codec {
    Hevc,
    H264,
    Av1,
}

/// The encoder's speed: `Best` is NVENC P7, AMF quality (AV1: highQuality), Quick Sync TU1; `Balanced` is P5,
/// balanced (AV1: quality), TU4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Preset {
    Best,
    Balanced,
}

/// One rung of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tier {
    pub hardware: Hardware,
    pub codec: Codec,
    pub preset: Preset,
    /// The output height; the width follows the canvas.
    pub height: u32,
}

/// The size of OBS's canvas: the game's image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
}

/// What a card's encoder can do, from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gpu {
    pub hardware: Hardware,
    /// AV1 is worth testing: NVIDIA RTX 40/50, AMD RX 9000, Intel Arc. AMD's RX 7000 encodes AV1 but pads 1080p to
    /// 1082 lines (A-obs-quality.md 2.6), so it stays on HEVC.
    pub av1: bool,
    /// NVENC takes B-frames (RTX); older NVIDIA cards get none rather than an encoder that does not start. AMD and
    /// Intel take the values of A-obs-quality.md 2.6 whatever this says.
    pub b_frames: bool,
}

/// Why a test failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lag {
    /// The encoder did not keep up (`outputSkippedFrames`).
    Encoding,
    /// OBS's rendering did not keep up (`renderSkippedFrames`, fps, render time): the game takes the card.
    Rendering,
}

impl Tier {
    pub fn cqp(&self) -> u8 {
        match self.codec {
            Codec::Hevc => HEVC_CQP,
            Codec::H264 => H264_CQP,
            Codec::Av1 => AV1_CQP,
        }
    }

    /// The output size for `canvas`: the canvas itself at its height or above, else scaled to `height` (even width).
    pub fn output(&self, canvas: Canvas) -> Canvas {
        if self.height >= canvas.height {
            return canvas;
        }
        let width = (u64::from(canvas.width) * u64::from(self.height) / u64::from(canvas.height)) as u32;
        Canvas { width: width & !1, height: self.height }
    }
}

/// The page shows the constant quality and OBS's encoder id with the tier; reading a tier back ignores both.
impl Serialize for Tier {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct View {
            hardware: Hardware,
            codec: Codec,
            preset: Preset,
            height: u32,
            cqp: u8,
            encoder: &'static str,
        }
        View { hardware: self.hardware, codec: self.codec, preset: self.preset, height: self.height, cqp: self.cqp(), encoder: crate::obs::profile::encoder_id(self.hardware, self.codec) }.serialize(serializer)
    }
}

/// The number after `marker` in `name` ("RTX 3070" -> 3070).
fn model_number(name: &str, marker: &str) -> Option<u32> {
    let at = name.find(marker)? + marker.len();
    let digits: String = name[at..].trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The card's encoder from the game's log (`Using graphics adapter NVIDIA GeForce RTX 3070`); None for a card
/// without a hardware encoder OBS knows.
pub fn gpu(vendor: Vendor, name: &str) -> Option<Gpu> {
    let upper = name.to_uppercase();
    match vendor {
        Vendor::Nvidia => {
            let rtx = upper.contains("RTX");
            let series = model_number(&upper, "RTX").map(|n| n / 1000);
            let av1 = matches!(series, Some(4 | 5)) || upper.contains("ADA") || upper.contains("BLACKWELL");
            Some(Gpu { hardware: Hardware::Nvenc, av1, b_frames: rtx })
        }
        Vendor::Amd => Some(Gpu { hardware: Hardware::Amf, av1: model_number(&upper, "RX").is_some_and(|n| n / 1000 == 9), b_frames: false }),
        Vendor::Intel => Some(Gpu { hardware: Hardware::Qsv, av1: upper.contains("ARC"), b_frames: false }),
        Vendor::Other => None,
    }
}

/// Where the test starts: HEVC at the best preset, 1080p (the game's size below it).
pub fn start(gpu: Gpu, canvas: Canvas) -> Tier {
    Tier { hardware: gpu.hardware, codec: Codec::Hevc, preset: Preset::Best, height: canvas.height.min(HEIGHTS[0]) }
}

/// The next tier down after `lag`, None when there is none (or no encoder step can help).
pub fn step_down(tier: Tier, lag: Lag) -> Option<Tier> {
    if lag == Lag::Rendering {
        return None;
    }
    if tier.preset == Preset::Best {
        return Some(Tier { preset: Preset::Balanced, ..tier });
    }
    if tier.codec == Codec::Av1 {
        return Some(Tier { codec: Codec::Hevc, ..tier });
    }
    if let Some(&lower) = HEIGHTS.iter().rev().find(|&&h| h < tier.height) {
        return Some(Tier { height: lower, ..tier });
    }
    (tier.codec != Codec::H264).then_some(Tier { codec: Codec::H264, ..tier })
}

/// The next tier up after a test that held, None at the top: a larger output while the canvas is larger (at the
/// balanced preset from 1440p, A-obs-quality.md 2.6), then AV1 where it is worth it.
pub fn step_up(tier: Tier, gpu: Gpu, canvas: Canvas) -> Option<Tier> {
    if let Some(&higher) = HEIGHTS.iter().find(|&&h| h > tier.height && h <= canvas.height) {
        return Some(Tier { height: higher, preset: Preset::Balanced, ..tier });
    }
    (tier.codec == Codec::Hevc && gpu.av1).then_some(Tier { codec: Codec::Av1, ..tier })
}

/// The quality test's walk over the ladder: down from a tier that fails until one passes, up from one that passes
/// until one fails (the last that passed stays), never up after it went down.
#[derive(Debug, Clone, PartialEq)]
pub struct Ladder {
    /// The tier to test next.
    pub tier: Tier,
    /// The best tier that passed and its measured rate.
    pub chosen: Option<(Tier, Option<f64>)>,
    /// Why the walk ended without a passing tier, or why it did not go further down.
    pub reason: Option<Verdict>,
    going_up: bool,
    went_down: bool,
}

impl Ladder {
    pub fn new(tier: Tier) -> Ladder {
        Ladder { tier, chosen: None, reason: None, going_up: false, went_down: false }
    }

    /// After testing `self.tier` with `verdict` and the rate it measured: the next tier to test, None when done.
    pub fn after(&mut self, verdict: Verdict, mbps: Option<f64>, gpu: Gpu, canvas: Canvas) -> Option<Tier> {
        let next = match verdict {
            Verdict::Pass => {
                (self.chosen, self.reason) = (Some((self.tier, mbps)), None);
                let up = step_up(self.tier, gpu, canvas).filter(|_| !self.went_down);
                self.going_up |= up.is_some();
                up
            }
            Verdict::CaptureBlack | Verdict::Unknown => {
                self.reason = Some(verdict);
                None
            }
            // A failing step up keeps the tier below it.
            Verdict::NoRecording | Verdict::EncodingLag | Verdict::RenderingLag if self.going_up => None,
            Verdict::NoRecording | Verdict::EncodingLag | Verdict::RenderingLag => {
                self.reason = Some(verdict);
                let down = step_down(self.tier, if verdict == Verdict::RenderingLag { Lag::Rendering } else { Lag::Encoding });
                self.went_down |= down.is_some();
                down
            }
        };
        if let Some(tier) = next {
            self.tier = tier;
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_HD: Canvas = Canvas { width: 1920, height: 1080 };
    const QHD: Canvas = Canvas { width: 2560, height: 1440 };
    const UHD: Canvas = Canvas { width: 3840, height: 2160 };

    fn nvidia(name: &str) -> Gpu {
        gpu(Vendor::Nvidia, name).unwrap()
    }

    #[test]
    fn knows_the_cards_encoders_from_their_names() {
        assert_eq!(nvidia("NVIDIA GeForce RTX 3070"), Gpu { hardware: Hardware::Nvenc, av1: false, b_frames: true });
        assert!(nvidia("NVIDIA GeForce RTX 4090 Laptop GPU").av1);
        assert!(nvidia("NVIDIA GeForce RTX 5070 Ti").av1);
        assert!(nvidia("NVIDIA RTX 4000 Ada Generation").av1);
        assert!(!nvidia("NVIDIA RTX A4000").av1);
        assert!(!nvidia("NVIDIA GeForce RTX 2060 SUPER").av1);
        assert_eq!(nvidia("NVIDIA GeForce GTX 1660 SUPER"), Gpu { hardware: Hardware::Nvenc, av1: false, b_frames: false });
        assert_eq!(gpu(Vendor::Amd, "AMD Radeon RX 9070 XT").unwrap(), Gpu { hardware: Hardware::Amf, av1: true, b_frames: false });
        assert!(!gpu(Vendor::Amd, "AMD Radeon RX 7900 XTX").unwrap().av1, "RX 7000 pads AV1 at 1080p");
        assert!(!gpu(Vendor::Amd, "AMD Radeon(TM) Graphics").unwrap().av1);
        assert!(gpu(Vendor::Intel, "Intel(R) Arc(TM) B580 Graphics").unwrap().av1);
        assert_eq!(gpu(Vendor::Intel, "Intel(R) UHD Graphics 770").unwrap().hardware, Hardware::Qsv);
        assert_eq!(gpu(Vendor::Other, "Microsoft Basic Render Driver"), None);
    }

    #[test]
    fn starts_at_1080p_hevc_best_or_at_the_games_size_below() {
        let rtx = nvidia("NVIDIA GeForce RTX 3070");
        let top = Tier { hardware: Hardware::Nvenc, codec: Codec::Hevc, preset: Preset::Best, height: 1080 };
        assert_eq!(start(rtx, FULL_HD), top);
        assert_eq!(start(rtx, UHD), top);
        assert_eq!(start(rtx, Canvas { width: 1600, height: 900 }).height, 900);
        assert_eq!(top.cqp(), HEVC_CQP);
    }

    #[test]
    fn steps_down_p7_p5_then_the_size_then_h264() {
        let mut tier = Tier { hardware: Hardware::Nvenc, codec: Codec::Hevc, preset: Preset::Balanced, height: 2160 };
        let mut seen = vec![];
        while let Some(next) = step_down(tier, Lag::Encoding) {
            seen.push((next.codec, next.preset, next.height));
            tier = next;
        }
        assert_eq!(seen, [(Codec::Hevc, Preset::Balanced, 1440), (Codec::Hevc, Preset::Balanced, 1080), (Codec::H264, Preset::Balanced, 1080)]);
        let top = start(nvidia("NVIDIA GeForce RTX 3070"), FULL_HD);
        assert_eq!(step_down(top, Lag::Encoding), Some(Tier { preset: Preset::Balanced, ..top }));
        assert_eq!(step_down(top, Lag::Rendering), None, "the game's load is not the encoder's");
        let av1 = Tier { codec: Codec::Av1, preset: Preset::Balanced, ..top };
        assert_eq!(step_down(av1, Lag::Encoding), Some(Tier { codec: Codec::Hevc, ..av1 }));
        let small = Tier { height: 900, preset: Preset::Balanced, ..top };
        assert_eq!(step_down(small, Lag::Encoding), Some(Tier { codec: Codec::H264, ..small }));
    }

    #[test]
    fn steps_up_to_the_canvas_then_av1_where_it_is_worth_it() {
        let rtx30 = nvidia("NVIDIA GeForce RTX 3070");
        let rtx40 = nvidia("NVIDIA GeForce RTX 4080");
        let top = start(rtx30, FULL_HD);
        assert_eq!(step_up(top, rtx30, FULL_HD), None, "Joe's PC: 1080p HEVC P7 is the top");
        assert_eq!(step_up(top, rtx40, FULL_HD), Some(Tier { codec: Codec::Av1, ..top }));
        let q = step_up(top, rtx30, QHD).unwrap();
        assert_eq!((q.height, q.preset, q.codec), (1440, Preset::Balanced, Codec::Hevc));
        assert_eq!(step_up(q, rtx30, QHD), None);
        let u = step_up(step_up(top, rtx40, UHD).unwrap(), rtx40, UHD).unwrap();
        assert_eq!(u.height, 2160);
        assert_eq!(step_up(u, rtx40, UHD).unwrap().codec, Codec::Av1);
        assert_eq!(step_up(Tier { codec: Codec::Av1, ..u }, rtx40, UHD), None);
        // 2560x1600 records 1440p, never upscaled
        assert_eq!(step_up(top, rtx30, Canvas { width: 2560, height: 1600 }).unwrap().height, 1440);
    }

    #[test]
    fn walks_the_ladder_down_until_a_tier_holds_and_up_until_one_fails() {
        let rtx40 = nvidia("NVIDIA GeForce RTX 4080");
        let top = start(rtx40, QHD);
        // up: 1080p passes, 1440p passes, AV1 does not start (no recording): 1440p HEVC stays
        let mut ladder = Ladder::new(top);
        let q = ladder.after(Verdict::Pass, Some(50.0), rtx40, QHD).unwrap();
        assert_eq!(q.height, 1440);
        let av1 = ladder.after(Verdict::Pass, Some(80.0), rtx40, QHD).unwrap();
        assert_eq!(av1.codec, Codec::Av1);
        assert_eq!(ladder.after(Verdict::NoRecording, None, rtx40, QHD), None);
        assert_eq!((ladder.chosen, ladder.reason), (Some((q, Some(80.0))), None));
        // down: the start tier records nothing (encoder not there), P5 drops frames, 1080p... H.264 holds; no way up after
        let rtx30 = nvidia("NVIDIA GeForce RTX 3070");
        let mut ladder = Ladder::new(start(rtx30, FULL_HD));
        assert_eq!(ladder.after(Verdict::NoRecording, None, rtx30, FULL_HD).map(|t| t.preset), Some(Preset::Balanced));
        assert_eq!(ladder.after(Verdict::EncodingLag, Some(50.0), rtx30, FULL_HD).map(|t| t.codec), Some(Codec::H264));
        assert_eq!(ladder.after(Verdict::Pass, Some(45.0), rtx30, QHD), None, "not up again after going down");
        assert_eq!(ladder.chosen.map(|(t, _)| t.codec), Some(Codec::H264));
        assert_eq!(ladder.reason, None);
        // the game's load ends the walk at once, as does a black capture
        let mut ladder = Ladder::new(start(rtx30, FULL_HD));
        assert_eq!(ladder.after(Verdict::RenderingLag, Some(50.0), rtx30, FULL_HD), None);
        assert_eq!((ladder.chosen, ladder.reason), (None, Some(Verdict::RenderingLag)));
        let mut ladder = Ladder::new(start(rtx30, FULL_HD));
        assert_eq!(ladder.after(Verdict::CaptureBlack, None, rtx30, FULL_HD), None);
        assert_eq!(ladder.reason, Some(Verdict::CaptureBlack));
        // nothing below H.264 at 1080p: the last reason stays
        let mut ladder = Ladder::new(Tier { codec: Codec::H264, preset: Preset::Balanced, ..start(rtx30, FULL_HD) });
        assert_eq!(ladder.after(Verdict::EncodingLag, None, rtx30, FULL_HD), None);
        assert_eq!(ladder.reason, Some(Verdict::EncodingLag));
    }

    #[test]
    fn scales_the_output_to_the_tiers_height() {
        let tier = Tier { hardware: Hardware::Nvenc, codec: Codec::Hevc, preset: Preset::Best, height: 1080 };
        assert_eq!(tier.output(FULL_HD), FULL_HD);
        assert_eq!(tier.output(QHD), FULL_HD);
        assert_eq!(tier.output(Canvas { width: 3440, height: 1440 }), Canvas { width: 2580, height: 1080 });
        assert_eq!(tier.output(Canvas { width: 1600, height: 900 }), Canvas { width: 1600, height: 900 });
    }

    #[test]
    fn shows_the_quality_and_the_encoder_and_reads_back_without_them() {
        let tier = Tier { hardware: Hardware::Nvenc, codec: Codec::Hevc, preset: Preset::Best, height: 1080 };
        let json = serde_json::to_value(tier).unwrap();
        assert_eq!(json, serde_json::json!({ "hardware": "nvenc", "codec": "hevc", "preset": "best", "height": 1080, "cqp": 20, "encoder": "obs_nvenc_hevc_tex" }));
        assert_eq!(serde_json::from_value::<Tier>(json).unwrap(), tier);
    }
}
