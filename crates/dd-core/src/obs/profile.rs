//! What OBS loads: the profile "Digital Drivers" (`basic/profiles/DigitalDrivers/basic.ini` and
//! `recordEncoder.json`), the scene collection "Digital Drivers" (`basic/scenes/DigitalDrivers.json`, one empty
//! scene) and the three sources the shell creates in it through obs-websocket: the game's image, the game's sound
//! only, and the DD mark. The files are the ones `03-obs-setup.ps1` wrote and OBS 33.0.1 loaded on Joe's PC on
//! 2026-11-09 (only for OBS 32 and newer: decision 1 of the plan); the encoder keys of AMD and Intel are read from
//! OBS 33.0.1's source (`texture-amf.cpp`, `obs-qsv11.c`) and not yet run on such a card.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::obs::tiers::{Canvas, Codec, Gpu, Hardware, Preset, Tier};

/// The names OBS shows and the app asks for with `--profile` / `--collection`.
pub const PROFILE_NAME: &str = "Digital Drivers";
pub const COLLECTION_NAME: &str = "Digital Drivers";
/// The profile's folder below `basic/profiles` and the collection's file below `basic/scenes`.
pub const PROFILE_DIR: &str = "DigitalDrivers";
pub const COLLECTION_FILE: &str = "DigitalDrivers.json";
/// The collection's one scene.
pub const SCENE_NAME: &str = "Digital Drivers";
pub const GAME_SOURCE: &str = "EVO Game Capture";
pub const AUDIO_SOURCE: &str = "EVO Game Audio";
pub const MARK_SOURCE: &str = "DD Watermark";
/// The game's executable, which the capture and the sound are matched by.
pub const GAME_EXE: &str = "AssettoCorsaEVO.exe";
/// The window string when OBS lists no window of the game: `title:class:exe`, matched by the exe (priority 2).
pub const GAME_WINDOW_GUESS: &str = "AssettoCorsaEVO:AssettoCorsaEVO:AssettoCorsaEVO.exe";
/// Widths of the DD mark PNGs the app ships (9 % of 1920 and of 2560 px); the closest is scaled to 9 %.
pub const MARK_WIDTHS: [u32; 2] = [173, 230];
/// The mark's width in per cent of the canvas, and its margin from the edges (A-obs-quality.md section 6).
const MARK_SHARE: f64 = 0.09;
const MARK_MARGIN: f64 = 0.05;
/// OBS's fixed id of the main canvas, as in Joe's 33.0.1 collection.
const MAIN_CANVAS: &str = "6c69626f-6273-4c00-9d88-c5136d61696e";
/// Our scene's id; it only has to be unique within the collection.
const SCENE_UUID: &str = "d0d0d0d0-6464-4d00-9d00-000000000001";

/// OBS's id of the encoder.
pub fn encoder_id(hardware: Hardware, codec: Codec) -> &'static str {
    match (hardware, codec) {
        (Hardware::Nvenc, Codec::Hevc) => "obs_nvenc_hevc_tex",
        (Hardware::Nvenc, Codec::H264) => "obs_nvenc_h264_tex",
        (Hardware::Nvenc, Codec::Av1) => "obs_nvenc_av1_tex",
        (Hardware::Amf, Codec::Hevc) => "h265_texture_amf",
        (Hardware::Amf, Codec::H264) => "h264_texture_amf",
        (Hardware::Amf, Codec::Av1) => "av1_texture_amf",
        (Hardware::Qsv, Codec::Hevc) => "obs_qsv11_hevc",
        (Hardware::Qsv, Codec::H264) => "obs_qsv11_v2",
        (Hardware::Qsv, Codec::Av1) => "obs_qsv11_av1",
    }
}

/// `basic.ini` of our profile (UTF-8 with BOM and `\n`, as OBS keeps it). The streaming encoder is the card's
/// H.264, so OBS 33's check for missing encoders finds nothing missing. Paths with `/`: OBS 33 escapes `\` in its
/// ini files.
pub fn basic_ini(tier: Tier, canvas: Canvas, record_dir: &str) -> String {
    let output = tier.output(canvas);
    format!(
        "\u{feff}[General]
Name={PROFILE_NAME}

[Output]
Mode=Advanced
FilenameFormatting=DD %CCYY-%MM-%DD %hh-%mm-%ss

[AdvOut]
Encoder={stream}
RecType=Standard
RecEncoder={record}
RecFormat2=hybrid_mp4
RecFilePath={path}
RecTracks=1
RecAudioEncoder=ffmpeg_aac
Track1Bitrate=320
RecUseRescale=false
RecSplitFile=false
RecRB=false

[Video]
BaseCX={bw}
BaseCY={bh}
OutputCX={ow}
OutputCY={oh}
FPSType=0
FPSCommon=60
ScaleType=lanczos
ColorFormat=NV12
ColorSpace=709
ColorRange=Partial

[Audio]
SampleRate=48000
ChannelSetup=Stereo
",
        stream = encoder_id(tier.hardware, Codec::H264),
        record = encoder_id(tier.hardware, tier.codec),
        path = record_dir.replace('\\', "/"),
        bw = canvas.width,
        bh = canvas.height,
        ow = output.width,
        oh = output.height,
    )
}

/// `recordEncoder.json`: constant quality, 2 s keyframes, and per encoder the values of A-obs-quality.md 2.6
/// (NVENC: tune hq, quarter-resolution two-pass, AQ on, no lookahead, the OBS 31+ keys `preset` and
/// `adaptive_quantization`).
pub fn record_encoder(tier: Tier, gpu: Gpu) -> String {
    let best = tier.preset == Preset::Best;
    let value = match tier.hardware {
        Hardware::Nvenc => json!({
            "rate_control": "CQP",
            "cqp": tier.cqp(),
            "preset": if best { "p7" } else { "p5" },
            "tune": "hq",
            "multipass": "qres",
            "profile": if tier.codec == Codec::H264 { "high" } else { "main" },
            "bf": if gpu.b_frames { 2 } else { 0 },
            "adaptive_quantization": true,
            "lookahead": false,
            "keyint_sec": 2,
        }),
        Hardware::Amf => json!({
            "rate_control": "CQP",
            "cqp": tier.cqp(),
            "preset": match (tier.codec, best) {
                (Codec::Av1, true) => "highQuality",
                (Codec::Av1, false) | (_, true) => "quality",
                (_, false) => "balanced",
            },
            "bf": if tier.codec == Codec::Hevc { 0 } else { 2 },
            "keyint_sec": 2,
        }),
        Hardware::Qsv if tier.codec == Codec::Av1 => json!({
            "rate_control": "CQP",
            "cqp": tier.cqp(),
            "target_usage": if best { "TU1" } else { "TU4" },
            "bframes": 3,
            "keyint_sec": 2,
        }),
        Hardware::Qsv => json!({
            "rate_control": "ICQ",
            "icq_quality": tier.cqp(),
            "target_usage": if best { "TU1" } else { "TU4" },
            "bframes": 3,
            "keyint_sec": 2,
        }),
    };
    value.to_string()
}

/// The scene collection: one empty scene and no desktop or microphone sound (no `DesktopAudioDevice1` /
/// `AuxAudioDevice1`). The sources come through obs-websocket, which places them for any canvas.
pub fn scene_collection() -> String {
    let collection = json!({
        "name": COLLECTION_NAME,
        "current_scene": SCENE_NAME,
        "current_program_scene": SCENE_NAME,
        "scene_order": [{ "name": SCENE_NAME }],
        "sources": [{
            "name": SCENE_NAME,
            "uuid": SCENE_UUID,
            "id": "scene",
            "versioned_id": "scene",
            "settings": { "id_counter": 0, "custom_size": false, "items": [] },
            "mixers": 0,
            "sync": 0,
            "flags": 0,
            "volume": 1.0,
            "balance": 0.5,
            "enabled": true,
            "muted": false,
            "hotkeys": {},
            "private_settings": {},
            "canvas_uuid": MAIN_CANVAS,
        }],
        "groups": [],
        "transitions": [],
        "quick_transitions": [],
        "saved_projectors": [],
        "preview_locked": false,
        "scaling_enabled": false,
        "modules": {},
    });
    serde_json::to_string_pretty(&collection).expect("a JSON value")
}

/// The game's window among the items OBS lists for the game capture's `window` property
/// (`GetInputPropertiesListPropertyItems`): the first of the game's exe, as on Joe's PC.
pub fn game_window(items: &Value) -> Option<String> {
    items["propertyItems"].as_array()?.iter().filter_map(|item| item["itemValue"].as_str()).find(|value| value.ends_with(GAME_EXE)).map(str::to_string)
}

/// The game capture: the game's window by its exe, no cursor, no overlays, the anti-cheat hook.
pub fn game_capture_settings(window: &str) -> Value {
    json!({ "capture_mode": "window", "window": window, "priority": 2, "capture_cursor": false, "limit_framerate": true, "capture_overlays": false, "anti_cheat_hook": true })
}

/// The game's sound only: no Discord, no music.
pub fn game_audio_settings(window: &str) -> Value {
    json!({ "window": window, "priority": 2 })
}

pub fn mark_settings(file: &str) -> Value {
    json!({ "file": file, "unload": false })
}

/// The game's image fills the canvas.
pub fn game_transform(canvas: Canvas) -> Value {
    json!({ "positionX": 0, "positionY": 0, "alignment": 5, "boundsType": "OBS_BOUNDS_SCALE_INNER", "boundsAlignment": 0, "boundsWidth": canvas.width, "boundsHeight": canvas.height })
}

/// Where the DD mark sits; the plan's default is top right (timing towers sit left, YouTube's controls below).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    #[default]
    TopRight,
    TopLeft,
    BottomLeft,
    BottomRight,
}

/// Which of the shipped PNGs to use for `canvas`: the one closest to 9 % of its width.
pub fn mark_width(canvas: Canvas) -> u32 {
    let target = MARK_SHARE * f64::from(canvas.width);
    *MARK_WIDTHS.iter().min_by(|a, b| (f64::from(**a) - target).abs().total_cmp(&(f64::from(**b) - target).abs())).expect("two widths")
}

/// The mark's transform: 9 % of the canvas width, 5 % from the two edges of `corner`.
pub fn mark_transform(corner: Corner, canvas: Canvas, png_width: u32) -> Value {
    // OBS's alignment flags: left 1, right 2, top 4, bottom 8
    let (right, bottom) = match corner {
        Corner::TopRight => (true, false),
        Corner::TopLeft => (false, false),
        Corner::BottomLeft => (false, true),
        Corner::BottomRight => (true, true),
    };
    let (w, h) = (f64::from(canvas.width), f64::from(canvas.height));
    let x = if right { w * (1.0 - MARK_MARGIN) } else { w * MARK_MARGIN };
    let y = if bottom { h * (1.0 - MARK_MARGIN) } else { h * MARK_MARGIN };
    let alignment = (if right { 2 } else { 1 }) + (if bottom { 8 } else { 4 });
    let scale = MARK_SHARE * w / f64::from(png_width);
    json!({ "alignment": alignment, "positionX": x.round(), "positionY": y.round(), "scaleX": scale, "scaleY": scale, "boundsType": "OBS_BOUNDS_NONE" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_check::Vendor;
    use crate::obs::tiers;

    const FULL_HD: Canvas = Canvas { width: 1920, height: 1080 };

    fn joe() -> (Tier, Gpu) {
        let gpu = tiers::gpu(Vendor::Nvidia, "NVIDIA GeForce RTX 3070").unwrap();
        (tiers::start(gpu, FULL_HD), gpu)
    }

    /// The profile `03-obs-setup.ps1` wrote for "DD Test" (1920x1080) on 2026-11-09, with our name and file name
    /// pattern; OBS 33.0.1 loaded it.
    const SCRIPT_BASIC_INI: &str = "[General]
Name=Digital Drivers

[Output]
Mode=Advanced
FilenameFormatting=DD %CCYY-%MM-%DD %hh-%mm-%ss

[AdvOut]
Encoder=obs_nvenc_h264_tex
RecType=Standard
RecEncoder=obs_nvenc_hevc_tex
RecFormat2=hybrid_mp4
RecFilePath=C:/projekte/digitaldrivers/testabend/video
RecTracks=1
RecAudioEncoder=ffmpeg_aac
Track1Bitrate=320
RecUseRescale=false
RecSplitFile=false
RecRB=false

[Video]
BaseCX=1920
BaseCY=1080
OutputCX=1920
OutputCY=1080
FPSType=0
FPSCommon=60
ScaleType=lanczos
ColorFormat=NV12
ColorSpace=709
ColorRange=Partial

[Audio]
SampleRate=48000
ChannelSetup=Stereo
";

    fn ini(text: &str, section: &str, key: &str) -> Option<String> {
        crate::obs::config::ini_value(text, section, key)
    }

    #[test]
    fn writes_the_profile_the_script_wrote_and_obs_loaded() {
        let (tier, _) = joe();
        let text = basic_ini(tier, FULL_HD, r"C:\projekte\digitaldrivers\testabend\video");
        assert_eq!(text.strip_prefix('\u{feff}'), Some(SCRIPT_BASIC_INI));
        // `profileParams` of results/03-obs-capture-dlaa-1080p-20261109-205758.json: what OBS read back
        for (key, loaded) in [
            ("Output/Mode", "Advanced"),
            ("AdvOut/RecEncoder", "obs_nvenc_hevc_tex"),
            ("AdvOut/RecFormat2", "hybrid_mp4"),
            ("AdvOut/RecFilePath", "C:/projekte/digitaldrivers/testabend/video"),
            ("AdvOut/RecTracks", "1"),
            ("Video/BaseCX", "1920"),
            ("Video/BaseCY", "1080"),
            ("Video/OutputCX", "1920"),
            ("Video/OutputCY", "1080"),
            ("Video/FPSCommon", "60"),
            ("Video/ColorFormat", "NV12"),
        ] {
            let (section, name) = key.split_once('/').unwrap();
            assert_eq!(ini(&text, section, name).as_deref(), Some(loaded), "{key}");
        }
    }

    #[test]
    fn scales_down_in_the_profile_and_streams_with_the_cards_h264() {
        let gpu = tiers::gpu(Vendor::Amd, "AMD Radeon RX 6800 XT").unwrap();
        let tier = Tier { height: 1080, ..tiers::start(gpu, Canvas { width: 2560, height: 1440 }) };
        let text = basic_ini(tier, Canvas { width: 2560, height: 1440 }, "D:/Videos/Digital Drivers");
        assert_eq!(ini(&text, "Video", "BaseCX").as_deref(), Some("2560"));
        assert_eq!(ini(&text, "Video", "OutputCY").as_deref(), Some("1080"));
        assert_eq!(ini(&text, "AdvOut", "Encoder").as_deref(), Some("h264_texture_amf"));
        assert_eq!(ini(&text, "AdvOut", "RecEncoder").as_deref(), Some("h265_texture_amf"));
    }

    #[test]
    fn writes_the_nvenc_settings_obs_used_on_joes_pc() {
        let (tier, gpu) = joe();
        let written: Value = serde_json::from_str(&record_encoder(tier, gpu)).unwrap();
        // the "DD Test" encoder of 03-obs-setup.ps1
        let script = json!({ "rate_control": "CQP", "cqp": 16, "preset": "p7", "tune": "hq", "multipass": "qres", "profile": "main", "bf": 2, "adaptive_quantization": true, "lookahead": false, "keyint_sec": 2 });
        assert_eq!(written, script);
        let p5 = serde_json::from_str::<Value>(&record_encoder(Tier { preset: Preset::Balanced, codec: Codec::H264, ..tier }, gpu)).unwrap();
        assert_eq!((p5["preset"].as_str(), p5["profile"].as_str()), (Some("p5"), Some("high")));
        let gtx = tiers::gpu(Vendor::Nvidia, "NVIDIA GeForce GTX 1660").unwrap();
        assert_eq!(serde_json::from_str::<Value>(&record_encoder(tier, gtx)).unwrap()["bf"], 0);
    }

    #[test]
    fn writes_amd_and_intel_settings_by_obs_source() {
        let amd = tiers::gpu(Vendor::Amd, "AMD Radeon RX 9070 XT").unwrap();
        let tier = tiers::start(amd, FULL_HD);
        let hevc: Value = serde_json::from_str(&record_encoder(tier, amd)).unwrap();
        assert_eq!(hevc, json!({ "rate_control": "CQP", "cqp": 16, "preset": "quality", "bf": 0, "keyint_sec": 2 }));
        let av1: Value = serde_json::from_str(&record_encoder(Tier { codec: Codec::Av1, ..tier }, amd)).unwrap();
        assert_eq!((av1["preset"].as_str(), av1["bf"].as_u64()), (Some("highQuality"), Some(2)));
        let intel = tiers::gpu(Vendor::Intel, "Intel(R) Arc(TM) A770 Graphics").unwrap();
        let tier = tiers::start(intel, FULL_HD);
        let hevc: Value = serde_json::from_str(&record_encoder(tier, intel)).unwrap();
        assert_eq!(hevc, json!({ "rate_control": "ICQ", "icq_quality": 16, "target_usage": "TU1", "bframes": 3, "keyint_sec": 2 }));
        let av1: Value = serde_json::from_str(&record_encoder(Tier { codec: Codec::Av1, preset: Preset::Balanced, ..tier }, intel)).unwrap();
        assert_eq!((av1["rate_control"].as_str(), av1["cqp"].as_u64(), av1["target_usage"].as_str()), (Some("CQP"), Some(16), Some("TU4")));
    }

    #[test]
    fn writes_the_collection_the_script_wrote() {
        let written: Value = serde_json::from_str(&scene_collection()).unwrap();
        // 03-obs-setup.ps1's collection, named ours (its uuid was random)
        let script = json!({
            "name": "Digital Drivers", "current_scene": "Digital Drivers", "current_program_scene": "Digital Drivers",
            "scene_order": [{ "name": "Digital Drivers" }],
            "sources": [{ "name": "Digital Drivers", "uuid": SCENE_UUID, "id": "scene", "versioned_id": "scene",
                "settings": { "id_counter": 0, "custom_size": false, "items": [] },
                "mixers": 0, "sync": 0, "flags": 0, "volume": 1.0, "balance": 0.5, "enabled": true, "muted": false,
                "hotkeys": {}, "private_settings": {}, "canvas_uuid": "6c69626f-6273-4c00-9d88-c5136d61696e" }],
            "groups": [], "transitions": [], "quick_transitions": [], "saved_projectors": [],
            "preview_locked": false, "scaling_enabled": false, "modules": {}
        });
        assert_eq!(written, script);
        assert!(written.get("DesktopAudioDevice1").is_none() && written.get("AuxAudioDevice1").is_none());
    }

    #[test]
    fn picks_the_games_window_as_on_joes_pc() {
        // `windowCandidates` of the 2026-11-09 runs, after OBS's own window
        let items = json!({ "propertyItems": [
            { "itemName": "[obs64.exe]: OBS", "itemValue": "OBS:Qt:obs64.exe", "itemEnabled": true },
            { "itemName": "[AssettoCorsaEVO.exe]: AssettoCorsaEVO", "itemValue": "AssettoCorsaEVO:AssettoCorsaEVO:AssettoCorsaEVO.exe", "itemEnabled": true },
            { "itemName": "[AssettoCorsaEVO.exe]: Assetto Corsa EVO", "itemValue": "Assetto Corsa EVO:acevo:AssettoCorsaEVO.exe", "itemEnabled": true }
        ]});
        assert_eq!(game_window(&items).as_deref(), Some("AssettoCorsaEVO:AssettoCorsaEVO:AssettoCorsaEVO.exe"));
        assert_eq!(game_window(&json!({ "propertyItems": [] })), None);
        assert_eq!(game_capture_settings(GAME_WINDOW_GUESS)["capture_cursor"], false);
    }

    #[test]
    fn places_the_mark_as_the_script_did() {
        // top right at 1080p: alignment 6, (1824, 54), the 173 px PNG at about 1:1 (scale 0.999 in the result)
        assert_eq!(mark_width(FULL_HD), 173);
        assert_eq!(mark_width(Canvas { width: 2560, height: 1440 }), 230);
        let t = mark_transform(Corner::TopRight, FULL_HD, 173);
        assert_eq!((t["alignment"].as_u64(), t["positionX"].as_f64(), t["positionY"].as_f64()), (Some(6), Some(1824.0), Some(54.0)));
        assert!((t["scaleX"].as_f64().unwrap() - 0.999).abs() < 0.001);
        let t = mark_transform(Corner::BottomLeft, FULL_HD, 173);
        assert_eq!((t["alignment"].as_u64(), t["positionX"].as_f64(), t["positionY"].as_f64()), (Some(9), Some(96.0), Some(1026.0)));
        assert_eq!(serde_json::from_value::<Corner>(json!("bottom-right")).unwrap(), Corner::BottomRight);
    }
}
