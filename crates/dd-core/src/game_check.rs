//! The game check before recording a replay: how Assetto Corsa EVO renders, judged for a video. The game prints its
//! video settings as JSON into its log at every start, and again when the driver changes them
//! (`[rendering] [info] Video Settings` and the lines after it), with the internal render resolution
//! (`Render Resolution: (960 x 540)`), the graphics card (`Using graphics adapter ...`) and its replay settings
//! (`[replay] [info] Replay Setting`). This reads the last of them; it changes nothing. The rules follow
//! research/2026-11-08-replay-video/B-evo-graphics-replay.md ("Recommended recording preset" and section 3); the
//! page has the words for each check by its id.

use serde::Serialize;
use serde_json::Value;

/// The maker of the graphics card, which decides the upscaler to recommend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

fn vendor_of(gpu: &str) -> Vendor {
    let gpu = gpu.to_lowercase();
    if gpu.contains("nvidia") || gpu.contains("geforce") {
        Vendor::Nvidia
    } else if gpu.contains("amd") || gpu.contains("radeon") {
        Vendor::Amd
    } else if gpu.contains("intel") {
        Vendor::Intel
    } else {
        Vendor::Other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Upscaler {
    None,
    Dlss,
    Fsr,
}

/// The values the checks judge, as the game's log has them. Quality levels are the game's names without their
/// prefix: `Off`, `Low`, `Medium`, `High`, `Ultra`, `Custom`; upscaler modes `DLAA`, `Quality`, `Balanced`,
/// `Performance`, `UltraQuality`, `Custom` (DLSS) and `NativeAA`, `Quality`, ... (FSR).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVideo {
    /// When the game wrote the settings, as its log has it: the PC's local time, `2026-11-08 15:47:11`
    pub logged_at: String,
    /// `0.9.1+release.6`
    pub game_version: Option<String>,
    /// `NVIDIA GeForce RTX 3070`
    pub gpu: Option<String>,
    pub vendor: Vendor,
    pub vram_mb: Option<u32>,
    /// The graphics driver as Windows numbers it (`32.0.16.2101` is NVIDIA's 621.01)
    pub driver: Option<String>,
    pub window_width: u32,
    pub window_height: u32,
    /// Exclusive fullscreen; else a borderless window
    pub fullscreen: bool,
    /// Frames per second while driving and in replays, 0 for none
    pub frame_limit: u32,
    /// Frames per second while another window covers the game, 0 for none
    pub background_frame_limit: u32,
    /// `Off`, `On` or `Adaptive`
    pub v_sync: String,
    pub upscaler: Upscaler,
    /// None without an upscaler
    pub upscaler_mode: Option<String>,
    /// The resolution the game renders at before the upscaler, from its log line; None when the log has none
    pub render_width: Option<u32>,
    pub render_height: Option<u32>,
    /// Per cent of the window's resolution, 100 when off
    pub supersampling: u32,
    /// FidelityFX CAS, 0 to 2
    pub sharpening: f64,
    pub motion_blur: String,
    /// 0 to 3
    pub motion_blur_strength: f64,
    pub depth_of_field: String,
    pub level_of_detail: String,
    pub vehicle_level_of_detail: String,
    /// `Off`, `Single` or `Multi`
    pub mirrors: String,
    /// "Applied also in spectating cameras": hides cars from the TV cameras
    pub spectator_car_filter: bool,
    /// The replay quality, `VeryLow` to `Insane`; None when the log has no replay settings
    pub replay_quality: Option<String>,
    /// The replay length of the settings, in seconds (the menu offers 5 to 45 minutes)
    pub replay_length_s: Option<f64>,
}

/// How a value suits a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Good,
    Warn,
    Bad,
}

/// One line of the check: its id (the page's text), how the value suits a recording and the value itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub level: Level,
    pub value: String,
}

/// The message of a log line of `category` (`[2026-11-08 15:47:11.576] [rendering] [info] <message>`).
fn message<'a>(line: &'a str, category: &str) -> Option<&'a str> {
    let rest = line.strip_prefix('[')?;
    let (_, rest) = rest.split_once("] ")?;
    rest.strip_prefix('[')?.strip_prefix(category)?.strip_prefix("] [info] ")
}

/// The JSON the game prints after the line at `header`, one line of `category` per line of JSON.
fn json_after(lines: &[&str], header: usize, category: &str) -> Option<(Value, usize)> {
    let mut text = String::new();
    let mut depth = 0i32;
    for (i, line) in lines.iter().enumerate().skip(header + 1).take(1000) {
        // another thread's line between them
        let Some(part) = message(line, category) else { continue };
        depth += part.matches('{').count() as i32 - part.matches('}').count() as i32;
        text.push_str(part);
        text.push('\n');
        if depth <= 0 {
            return serde_json::from_str(&text).ok().map(|v| (v, i));
        }
    }
    None
}

/// The part of the game's enum name after its prefix (`DLSS_Performance` is `Performance`).
fn level_name(value: &Value) -> String {
    value.as_str().map(|s| s.rsplit('_').next().unwrap_or(s).to_string()).unwrap_or_default()
}

fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

fn whole(value: &Value) -> u32 {
    value.as_u64().and_then(|v| u32::try_from(v).ok()).unwrap_or(0)
}

/// `(960 x 540)` in a message.
fn resolution_in(message: &str) -> Option<(u32, u32)> {
    let inside = message.split_once('(')?.1.split_once(')')?.0;
    let (w, h) = inside.split_once(" x ")?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// The video settings the game logged last in `log` (one of its `Logs\log-*.txt`); None when it logged none (a run
/// that ended before the renderer started).
pub fn parse(log: &str) -> Option<GameVideo> {
    let lines: Vec<&str> = log.lines().collect();
    let header = lines.iter().rposition(|line| message(line, "rendering").is_some_and(|m| m.trim_end() == "Video Settings"))?;
    let (settings, end) = json_after(&lines, header, "rendering")?;
    let logged_at = lines[header].get(1..20)?.to_string();
    let display = &settings["display"];
    let graphics = &settings["graphics"];

    let before = &lines[..header];
    let last_before = |category: &str, prefix: &str| before.iter().rev().find_map(|l| message(l, category)?.strip_prefix(prefix)).map(str::trim);
    let adapter = last_before("rendering", "Using graphics adapter ");
    let (gpu, vram_mb) = match adapter.and_then(|a| a.rsplit_once(" (video memory ")) {
        Some((name, memory)) => (Some(name.to_string()), memory.split_whitespace().next().and_then(|mb| mb.parse().ok())),
        None => (adapter.map(str::to_string), None),
    };
    let game_version = lines.iter().find_map(|l| message(l, "others")?.strip_prefix("Build release ")?.split_once(", version ")?.1.split(',').next()).map(str::to_string);
    let render = lines[end..].iter().filter_map(|l| message(l, "rendering")).find_map(|m| m.trim_start().strip_prefix("Render Resolution:")).and_then(resolution_in);

    let replay = lines.iter().rposition(|l| message(l, "replay").is_some_and(|m| m.trim_end() == "Replay Setting")).and_then(|at| json_after(&lines, at, "replay")).map(|(v, _)| v);

    let upscaling = &graphics["upscaling"];
    let upscaler = match upscaling["mode"].as_str() {
        Some("Upscaling_DLSS") => Upscaler::Dlss,
        Some("Upscaling_FSR3") => Upscaler::Fsr,
        _ => Upscaler::None,
    };
    let upscaler_mode = match upscaler {
        Upscaler::Dlss => Some(level_name(&upscaling["dlss_preset"])),
        Upscaler::Fsr => Some(level_name(&upscaling["fsr3_preset"])),
        Upscaler::None => None,
    };
    // `Supersampling_150`; `Supersampling_Off` is 100 %
    let supersampling = level_name(&display["supersampling"]).parse::<u32>().unwrap_or(100);
    let frame_limit = &display["frame_rate_limit"];
    Some(GameVideo {
        logged_at,
        game_version,
        vendor: gpu.as_deref().map_or(Vendor::Other, vendor_of),
        gpu,
        vram_mb,
        driver: last_before("rendering", "Driver version:").map(str::to_string),
        window_width: whole(&display["window_width"]),
        window_height: whole(&display["window_height"]),
        fullscreen: display["is_fullscreen"].as_bool().unwrap_or(false),
        frame_limit: whole(&frame_limit["gameplay_frame_rate_limit"]),
        background_frame_limit: whole(&frame_limit["occluded_frame_rate_limit"]),
        v_sync: display["v_sync"].as_str().unwrap_or("Adaptive").to_string(),
        upscaler,
        upscaler_mode,
        render_width: render.map(|r| r.0),
        render_height: render.map(|r| r.1),
        supersampling,
        sharpening: number(&graphics["sharpening"]["strength"]),
        motion_blur: level_name(&graphics["motionBlur"]["quality"]),
        motion_blur_strength: number(&graphics["motionBlur"]["strength"]),
        depth_of_field: level_name(&graphics["depthOfField"]),
        level_of_detail: level_name(&graphics["levelOfDetail"]["levelOfDetail"]),
        vehicle_level_of_detail: level_name(&graphics["vehicleLevelOfDetail"]),
        mirrors: level_name(&graphics["mirror"]["mirror_mode"]),
        spectator_car_filter: graphics["car_visibility"]["filter_when_spectating"].as_bool().unwrap_or(false),
        replay_quality: replay.as_ref().and_then(|r| r["quality"].as_str()).map(str::to_string),
        replay_length_s: replay.as_ref().and_then(|r| r["max_duration"].as_f64()),
    })
}

/// The height the game renders at against the window's: Quality modes render two thirds of it.
fn render_scale(video: &GameVideo) -> Option<f64> {
    let height = f64::from(video.render_height?);
    (video.window_height > 0).then(|| height / f64::from(video.window_height))
}

fn by_scale(scale: f64) -> Level {
    if scale >= 0.66 {
        Level::Good
    } else if scale >= 0.57 {
        Level::Warn
    } else {
        Level::Bad
    }
}

/// An upscaler's mode by how sharp it renders: the native modes and the Quality modes record well, Balanced is
/// soft, Performance (half the height) is visibly soft after YouTube's encoding.
fn by_mode(mode: &str, video: &GameVideo) -> Level {
    match mode {
        "DLAA" | "NativeAA" | "Quality" | "UltraQuality" => Level::Good,
        "Balanced" => Level::Warn,
        "Performance" => Level::Bad,
        _ => render_scale(video).map_or(Level::Warn, by_scale),
    }
}

fn upscaler_level(video: &GameVideo) -> Level {
    let mode = video.upscaler_mode.as_deref().unwrap_or("");
    let worst = |a: Level, b: Level| if a == Level::Bad || b == Level::Bad { Level::Bad } else if a == Level::Warn || b == Level::Warn { Level::Warn } else { Level::Good };
    match (video.vendor, video.upscaler) {
        // NVIDIA: DLAA, else DLSS Quality; FSR and plain Filmic AA look worse on the same card
        (Vendor::Nvidia, Upscaler::Dlss) => by_mode(mode, video),
        (Vendor::Nvidia, Upscaler::Fsr) => worst(Level::Warn, by_mode(mode, video)),
        (Vendor::Nvidia, Upscaler::None) => Level::Warn,
        // AMD: FSR as anti-aliasing (NativeAA) or Quality
        (Vendor::Amd, Upscaler::Fsr) => by_mode(mode, video),
        (Vendor::Amd, _) => Level::Warn,
        // Intel (EVO has no XeSS): native with Filmic AA, or FSR NativeAA
        (Vendor::Intel, Upscaler::None) => Level::Good,
        (Vendor::Intel, Upscaler::Fsr) if mode == "NativeAA" => Level::Good,
        (Vendor::Intel, _) => worst(Level::Warn, by_mode(mode, video)),
        (Vendor::Other, Upscaler::None) => Level::Good,
        (Vendor::Other, _) => by_mode(mode, video),
    }
}

/// A number as text without trailing zeros: `1`, `0.3`.
fn text(value: f64) -> String {
    format!("{}", (value * 100.0).round() / 100.0)
}

fn fps(limit: u32) -> String {
    if limit == 0 { "off".to_string() } else { limit.to_string() }
}

/// The video settings judged for a recording at 60 fps, in the order the page shows them.
pub fn judge(video: &GameVideo) -> Vec<Check> {
    let check = |id, level, value: String| Check { id, level, value };
    let good_if = |good: bool| if good { Level::Good } else { Level::Warn };
    let mut checks = vec![
        check("window-mode", good_if(!video.fullscreen), if video.fullscreen { "fullscreen" } else { "borderless" }.to_string()),
        check("resolution", good_if(video.window_height >= 1080), format!("{}x{}", video.window_width, video.window_height)),
        check(
            "frame-limit",
            match video.frame_limit {
                60 => Level::Good,
                0 => Level::Bad,
                _ => Level::Warn,
            },
            fps(video.frame_limit),
        ),
        check(
            "background-frame-limit",
            good_if(video.background_frame_limit == 0 || (video.frame_limit > 0 && video.background_frame_limit >= video.frame_limit)),
            fps(video.background_frame_limit),
        ),
        check("v-sync", good_if(video.v_sync == "Off"), video.v_sync.clone()),
        check(
            "upscaler",
            upscaler_level(video),
            match (video.upscaler, &video.upscaler_mode) {
                (Upscaler::Dlss, Some(mode)) => format!("DLSS {mode}"),
                (Upscaler::Fsr, Some(mode)) => format!("FSR {mode}"),
                _ => "off".to_string(),
            },
        ),
    ];
    if let (Some(scale), Some(width), Some(height)) = (render_scale(video), video.render_width, video.render_height) {
        checks.push(check("render-resolution", by_scale(scale), format!("{width}x{height}")));
    }
    checks.extend([
        check(
            "supersampling",
            good_if(video.supersampling <= 150),
            if video.supersampling <= 100 { "off".to_string() } else { video.supersampling.to_string() },
        ),
        check(
            "sharpening",
            if video.sharpening <= 0.5 {
                Level::Good
            } else if video.sharpening <= 1.0 {
                Level::Warn
            } else {
                Level::Bad
            },
            text(video.sharpening),
        ),
        check(
            "motion-blur",
            good_if(video.motion_blur == "Off" || video.motion_blur_strength == 0.0 || (video.motion_blur == "Low" && video.motion_blur_strength <= 0.5)),
            if video.motion_blur == "Off" { "Off".to_string() } else { format!("{} {}", video.motion_blur, text(video.motion_blur_strength)) },
        ),
        check("depth-of-field", good_if(matches!(video.depth_of_field.as_str(), "High" | "Ultra")), video.depth_of_field.clone()),
        check(
            "level-of-detail",
            good_if(video.level_of_detail == "Ultra" && video.vehicle_level_of_detail == "Ultra"),
            format!("{}/{}", video.level_of_detail, video.vehicle_level_of_detail),
        ),
        check("mirrors", good_if(video.mirrors == "Off"), video.mirrors.clone()),
        check("spectator-car-filter", good_if(!video.spectator_car_filter), if video.spectator_car_filter { "on" } else { "off" }.to_string()),
    ]);
    if let Some(quality) = &video.replay_quality {
        let level = match quality.as_str() {
            "Insane" => Level::Good,
            "High" => Level::Warn,
            _ => Level::Bad,
        };
        checks.push(check("replay-quality", level, quality.clone()));
    }
    if let Some(length) = video.replay_length_s {
        checks.push(check("replay-length", good_if(length >= 45.0 * 60.0), text(length)));
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Joe's log of 2026-11-08 15:46:57, only the lines this reads (no paths, no names).
    const JOE: &str = include_str!("../tests/fixtures/evo-log-video-settings.txt");

    fn joe() -> GameVideo {
        parse(JOE).unwrap()
    }

    fn level(checks: &[Check], id: &str) -> (Level, String) {
        let c = checks.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no check {id}"));
        (c.level, c.value.clone())
    }

    #[test]
    fn reads_joe_s_video_settings_from_the_log() {
        let v = joe();
        assert_eq!(v.logged_at, "2026-11-08 15:47:11");
        assert_eq!(v.game_version.as_deref(), Some("0.9.1+release.6"));
        assert_eq!((v.gpu.as_deref(), v.vendor, v.vram_mb), (Some("NVIDIA GeForce RTX 3070"), Vendor::Nvidia, Some(8018)));
        assert_eq!(v.driver.as_deref(), Some("32.0.16.2101"));
        assert_eq!((v.window_width, v.window_height, v.fullscreen), (1920, 1080, false));
        assert_eq!((v.frame_limit, v.background_frame_limit, v.v_sync.as_str()), (60, 60, "Off"));
        assert_eq!((v.upscaler, v.upscaler_mode.as_deref()), (Upscaler::Dlss, Some("Performance")));
        // the JSON says 1920 x 1080 for the render size; the line after it has what the game renders
        assert_eq!((v.render_width, v.render_height), (Some(960), Some(540)));
        assert_eq!((v.supersampling, v.sharpening), (100, 1.0));
        assert_eq!((v.motion_blur.as_str(), v.motion_blur_strength), ("Off", 0.0));
        assert_eq!((v.depth_of_field.as_str(), v.level_of_detail.as_str(), v.vehicle_level_of_detail.as_str()), ("Medium", "High", "High"));
        assert_eq!((v.mirrors.as_str(), v.spectator_car_filter), ("Multi", false));
        assert_eq!((v.replay_quality.as_deref(), v.replay_length_s), (Some("Insane"), Some(1000.0)));
    }

    #[test]
    fn judges_joe_s_settings_dlss_performance_needs_quality() {
        let checks = judge(&joe());
        assert_eq!(level(&checks, "upscaler"), (Level::Bad, "DLSS Performance".into()));
        assert_eq!(level(&checks, "render-resolution"), (Level::Bad, "960x540".into()));
        assert_eq!(level(&checks, "sharpening"), (Level::Warn, "1".into()));
        assert_eq!(level(&checks, "frame-limit"), (Level::Good, "60".into()));
        assert_eq!(level(&checks, "window-mode"), (Level::Good, "borderless".into()));
        let ids: Vec<&str> = checks.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "window-mode", "resolution", "frame-limit", "background-frame-limit", "v-sync", "upscaler", "render-resolution", "supersampling",
                "sharpening", "motion-blur", "depth-of-field", "level-of-detail", "mirrors", "spectator-car-filter", "replay-quality", "replay-length"
            ]
        );
        let bad: Vec<&str> = checks.iter().filter(|c| c.level == Level::Bad).map(|c| c.id).collect();
        assert_eq!(bad, ["upscaler", "render-resolution"]);
        let warn: Vec<&str> = checks.iter().filter(|c| c.level == Level::Warn).map(|c| c.id).collect();
        assert_eq!(warn, ["sharpening", "depth-of-field", "level-of-detail", "mirrors", "replay-length"]);
    }

    #[test]
    fn the_fixture_holds_no_user_name_or_path() {
        assert!(!JOE.contains("Users") && !JOE.contains(":\\") && !JOE.contains("revision"));
    }

    /// Joe's log with the given replacements in the JSON and the render line.
    fn joe_with(replacements: &[(&str, &str)]) -> GameVideo {
        let mut log = JOE.to_string();
        for (from, to) in replacements {
            assert!(log.contains(from), "{from}");
            log = log.replace(from, to);
        }
        parse(&log).unwrap()
    }

    #[test]
    fn recommends_the_upscaler_by_the_card_s_maker() {
        let nvidia = |mode: &str| upscaler_level(&joe_with(&[("DLSS_Performance", mode)]));
        assert_eq!([nvidia("DLSS_DLAA"), nvidia("DLSS_Quality"), nvidia("DLSS_UltraQuality"), nvidia("DLSS_Balanced")], [Level::Good, Level::Good, Level::Good, Level::Warn]);
        let fsr_on = |gpu: &str, mode: &str| {
            upscaler_level(&joe_with(&[("\"Upscaling_DLSS\"", "\"Upscaling_FSR3\""), ("FSR3_NativeAA", mode), ("NVIDIA GeForce RTX 3070 (video", gpu)]))
        };
        assert_eq!(fsr_on("NVIDIA GeForce RTX 3070 (video", "FSR3_NativeAA"), Level::Warn);
        assert_eq!(fsr_on("AMD Radeon RX 7800 XT (video", "FSR3_NativeAA"), Level::Good);
        assert_eq!(fsr_on("AMD Radeon RX 7800 XT (video", "FSR3_Quality"), Level::Good);
        assert_eq!(fsr_on("AMD Radeon RX 7800 XT (video", "FSR3_Balanced"), Level::Warn);
        assert_eq!(fsr_on("AMD Radeon RX 7800 XT (video", "FSR3_Performance"), Level::Bad);
        assert_eq!(fsr_on("Intel(R) Arc(TM) A770 Graphics (video", "FSR3_NativeAA"), Level::Good);
        assert_eq!(fsr_on("Intel(R) Arc(TM) A770 Graphics (video", "FSR3_Quality"), Level::Warn);
        let native_on = |gpu: &str| upscaler_level(&joe_with(&[("\"Upscaling_DLSS\"", "\"Upscaling_None\""), ("NVIDIA GeForce RTX 3070 (video", gpu)]));
        assert_eq!(native_on("NVIDIA GeForce RTX 3070 (video"), Level::Warn);
        assert_eq!(native_on("AMD Radeon RX 6600 (video"), Level::Warn);
        assert_eq!(native_on("Intel(R) Arc(TM) B580 Graphics (video"), Level::Good);
        let native = judge(&joe_with(&[("\"Upscaling_DLSS\"", "\"Upscaling_None\"")]));
        assert_eq!(level(&native, "upscaler").1, "off");
    }

    #[test]
    fn judges_a_custom_ratio_by_the_render_resolution() {
        let custom = |height: &str| upscaler_level(&joe_with(&[("DLSS_Performance", "DLSS_Custom"), ("(960 x 540)", height)]));
        assert_eq!([custom("(1280 x 720)"), custom("(1114 x 626)"), custom("(960 x 540)")], [Level::Good, Level::Warn, Level::Bad]);
        // supersampling 150 % with DLSS Quality renders at the window's height: good
        let v = joe_with(&[("Supersampling_Off", "Supersampling_150"), ("DLSS_Performance", "DLSS_Quality"), ("(960 x 540)", "(1920 x 1080)")]);
        let checks = judge(&v);
        assert_eq!(level(&checks, "supersampling"), (Level::Good, "150".into()));
        assert_eq!(level(&checks, "render-resolution"), (Level::Good, "1920x1080".into()));
        let v = joe_with(&[("Supersampling_Off", "Supersampling_200")]);
        assert_eq!(level(&judge(&v), "supersampling"), (Level::Warn, "200".into()));
    }

    #[test]
    fn judges_the_frame_limits_v_sync_and_window() {
        let checks = judge(&joe_with(&[("\"gameplay_frame_rate_limit\": 60", "\"gameplay_frame_rate_limit\": 0"), ("\"v_sync\": \"Off\"", "\"v_sync\": \"On\"")]));
        assert_eq!(level(&checks, "frame-limit"), (Level::Bad, "off".into()));
        assert_eq!(level(&checks, "background-frame-limit").0, Level::Warn);
        assert_eq!(level(&checks, "v-sync"), (Level::Warn, "On".into()));
        let checks = judge(&joe_with(&[("\"gameplay_frame_rate_limit\": 60", "\"gameplay_frame_rate_limit\": 144"), ("\"occluded_frame_rate_limit\": 60", "\"occluded_frame_rate_limit\": 30")]));
        assert_eq!(level(&checks, "frame-limit"), (Level::Warn, "144".into()));
        assert_eq!(level(&checks, "background-frame-limit"), (Level::Warn, "30".into()));
        let checks = judge(&joe_with(&[("\"occluded_frame_rate_limit\": 60", "\"occluded_frame_rate_limit\": 0")]));
        assert_eq!(level(&checks, "background-frame-limit"), (Level::Good, "off".into()));
        let checks = judge(&joe_with(&[("\"is_fullscreen\": false", "\"is_fullscreen\": true"), ("\"window_height\": 1080", "\"window_height\": 720"), ("\"window_width\": 1920", "\"window_width\": 1280")]));
        assert_eq!(level(&checks, "window-mode"), (Level::Warn, "fullscreen".into()));
        assert_eq!(level(&checks, "resolution"), (Level::Warn, "1280x720".into()));
    }

    #[test]
    fn judges_sharpening_motion_blur_and_the_replay_settings() {
        let sharp = |s: &str| level(&judge(&joe_with(&[("\"strength\": 1\n", &format!("\"strength\": {s}\n"))])), "sharpening");
        assert_eq!(sharp("0.3"), (Level::Good, "0.3".into()));
        assert_eq!(sharp("1.5"), (Level::Bad, "1.5".into()));
        let blur = |quality: &str, strength: &str| {
            level(&judge(&joe_with(&[("MotionBlurQuality_Off", quality), ("\"strength\": 0,", &format!("\"strength\": {strength},"))])), "motion-blur")
        };
        assert_eq!(blur("MotionBlurQuality_Low", "0.5"), (Level::Good, "Low 0.5".into()));
        assert_eq!(blur("MotionBlurQuality_High", "2"), (Level::Warn, "High 2".into()));
        let replay = |quality: &str, length: &str| {
            let checks = judge(&joe_with(&[("\"Insane\"", quality), ("\"max_duration\": 1000", length)]));
            (level(&checks, "replay-quality").0, level(&checks, "replay-length"))
        };
        assert_eq!(replay("\"Insane\"", "\"max_duration\": 2700"), (Level::Good, (Level::Good, "2700".into())));
        assert_eq!(replay("\"High\"", "\"max_duration\": 1000"), (Level::Warn, (Level::Warn, "1000".into())));
        assert_eq!(replay("\"Medium\"", "\"max_duration\": 1000").0, Level::Bad);
    }

    #[test]
    fn takes_the_last_settings_also_with_windows_line_ends_and_lines_between() {
        // the game prints the settings again when the driver changes them
        let changed = JOE.replace("DLSS_Performance", "DLSS_DLAA").replace("15:47:11", "16:02:30");
        let mut log = format!("{JOE}{changed}");
        // another thread's line in the JSON
        log = log.replacen("\"msaa\": \"MSAA_Off\",", "\"msaa\": \"MSAA_Off\",\n[2026-11-08 16:02:30.001] [audio] [info] FMOD {", 2);
        let v = parse(&log.replace('\n', "\r\n")).unwrap();
        assert_eq!((v.logged_at.as_str(), v.upscaler_mode.as_deref()), ("2026-11-08 16:02:30", Some("DLAA")));
    }

    #[test]
    fn a_log_without_video_settings_has_none() {
        assert_eq!(parse(""), None);
        let start: String = JOE.lines().take_while(|l| !l.ends_with("] Video Settings")).map(|l| format!("{l}\n")).collect();
        assert_eq!(parse(&start), None);
        // cut off inside the JSON: the game crashed while printing
        let cut: String = JOE.lines().take(60).map(|l| format!("{l}\n")).collect();
        assert_eq!(parse(&cut), None);
        // without the render line, the adapter and the replay settings it still reads
        let bare: String = JOE.lines().filter(|l| !l.contains("Render Resolution") && !l.contains("adapter") && !l.contains("[replay]")).map(|l| format!("{l}\n")).collect();
        let v = parse(&bare).unwrap();
        assert_eq!((v.render_height, v.vendor, v.replay_quality.as_deref()), (None, Vendor::Other, None));
        let ids: Vec<&str> = judge(&v).iter().map(|c| c.id).collect();
        assert!(!ids.contains(&"render-resolution") && !ids.contains(&"replay-length"));
    }

    #[test]
    fn hands_the_page_camel_case() {
        let json = serde_json::to_string(&(judge(&joe()), joe())).unwrap();
        for key in ["\"level\":\"bad\"", "\"id\":\"upscaler\"", "\"loggedAt\"", "\"renderHeight\":540", "\"vendor\":\"nvidia\"", "\"upscaler\":\"dlss\"", "\"replayLengthS\":1000.0"] {
            assert!(json.contains(key), "{key} in {json}");
        }
    }
}
