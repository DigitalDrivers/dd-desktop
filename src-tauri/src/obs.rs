//! The replay page's OBS helper: records a replay through the driver's OBS Studio (32 or newer) in our own profile
//! and scene collection "Digital Drivers", with the best settings this PC holds without dropped frames, and puts
//! the driver's OBS back as it was afterwards. The pure parts (files, ladder, numbers, protocol) are
//! `dd_core::obs`; this side finds OBS (`HKLM\SOFTWARE\OBS Studio`), writes its files only while it is closed,
//! starts it with our profile and a websocket password of its own, controls it through obs-websocket 5, and closes
//! it gracefully (WM_CLOSE to its window after the recording stopped: obs-websocket has no quit request, and OBS 32+
//! offers Safe Mode, without the websocket, after an unclean exit). The mechanics are those of
//! research/2026-11-09-testabend/replay/03-obs-*.ps1, proven against OBS 33.0.1 on Joe's PC.
//!
//! What it changes in the driver's OBS, all put back by `obs_restore`: our profile folder
//! `basic/profiles/DigitalDrivers` and collection `basic/scenes/DigitalDrivers.json` stay; `server_enabled` in the
//! websocket's `config.json` is switched on while the app uses OBS (port and password come as start options, which
//! OBS does not save); OBS remembers the profile it was started with in `user.ini`, so the app switches back to the
//! driver's before closing and resets those four keys after. What it changed is remembered in `obs-restore.json` in
//! the app's config folder until it is put back, also over a restart of the app. The tier the quality test chose and
//! the rate it measured are kept in `obs-tested.json` and tested again when the graphics card, its driver, OBS's
//! version or the game's resolution change.
//!
//! # Commands (all `Result<T, String>` with kebab-case error codes; `failed: <text>` for the unexpected)
//!
//! `obs_status()` -> Status. Reads only; quick enough to poll.
//! ```text
//! { installed: bool,             // obs64.exe found
//!   version: "33.0.1" | null,    // OBS's version from its uninstall entry, null when unknown
//!   supported: bool | null,      // version >= 32 (null when unknown: checked again once OBS runs)
//!   running: bool,               // an obs64.exe runs (the app's or the driver's own)
//!   ours: bool,                  // the app started it and controls it (obs_prepare)
//!   busy: bool,                  // a quality test or a prepare is under way
//!   recording: bool,
//!   restorePending: bool,        // OBS is not back as it was yet (e.g. after the app was closed): call obs_restore
//!   gpu: "NVIDIA GeForce RTX 3070" | null,   // the card the game uses, from its log
//!   hardware: "nvenc" | "amf" | "qsv" | null,// null: no hardware encoder, no recording
//!   canvas: { width, height } | null,        // the game's resolution = OBS's canvas
//!   tier: Tier | null,           // what a recording uses: OBS's current, else the tested one, else the start tier
//!   tested: Tested | null,       // the last quality test on this PC, null when none or no longer valid
//!   folder: "C:\\Users\\<u>\\Videos\\Digital Drivers" | null,
//!   freeBytes: number | null,    // free on the folder's drive
//!   minutesLeft: number | null } // at the tested rate, else at an estimate (60 Mb/s per 1080p)
//! Tier   = { hardware, codec: "hevc" | "h264" | "av1", preset: "best" | "balanced" (P7 | P5 on NVENC),
//!            height: 1080 | 1440 | 2160 | <the game's below 1080>, cqp: 20, encoder: "obs_nvenc_hevc_tex" }
//! Tested = { tier: Tier, mbps: 95.5, gbPerHour: 43.0, testedAt: <unix s>,
//!            fingerprint: { gpu, driver, obs, canvas } }
//! ```
//! Errors: none (an unknown value is null).
//!
//! `obs_prepare()` -> Status. OBS must be closed. Writes the profile (the tested tier, else the start tier) and the
//! collection, switches the websocket on, starts OBS minimised to the tray, checks that our profile and collection
//! are the current ones (OBS falls back to the driver's own silently) and sets up the sources `EVO Game Capture`,
//! `EVO Game Audio` and `DD Watermark`. Returns at once when the app's OBS already runs.
//! Errors: `obs-not-found`, `obs-too-old`, `obs-running` (the driver's own OBS runs: close it first),
//! `ac-evo-not-found`, `no-video-settings` (start the game once), `no-hardware-encoder`, `obs-config-unreadable`
//! (the websocket's config.json is no JSON), `obs-not-reachable` (no websocket within 30 s, e.g. Safe Mode),
//! `obs-exited`, `obs-auth-failed`, `obs-profile-not-loaded`. On an error after OBS started, it is closed and put
//! back.
//!
//! `obs_quality_test()` -> QualityTest. Needs `obs_prepare` and a replay playing in the game. Records 15 s per
//! tier (the clip is deleted), judges the numbers, steps down on encoding lag (P7 -> P5 -> smaller output -> H.264)
//! and up while a tier holds (larger output up to the game's resolution, then AV1 where the card is good at it);
//! each step restarts OBS with the new profile (~10 s). Leaves OBS on the chosen tier and keeps it for this PC.
//! ```text
//! { passed: bool,
//!   tier: Tier,                  // the chosen tier; when none passed, the one OBS is left on
//!   reason: null | "capture-black" | "rendering-lag" | "encoding-lag" | "unknown",
//!                                // why it stopped without a passing tier, or why it did not go further down:
//!                                // capture-black = no game image (no replay running, hook failed);
//!                                // rendering-lag = the game takes the card: cap its fps, lower its settings;
//!                                // encoding-lag = even H.264 at 1080p dropped frames; unknown = no numbers
//!   mbps: 95.5 | null, gbPerHour: 43.0 | null, minutesLeft: 51 | null,   // of the chosen tier
//!   capture: { width, height } | null,       // the game image's real size (canvas should match)
//!   attempts: [ { tier: Tier,
//!                 verdict: "pass" | "encoding-lag" | "rendering-lag" | "capture-black" | "unknown",
//!                 checks: { renderSkipped: bool|null, outputSkipped: bool|null, fps: bool, renderTime: bool,
//!                           captureNotBlack: bool|null } | null,
//!                 summary: Summary | null } ] }
//! Summary = { seconds, bytes, mbps: number|null, fpsAvg, fpsMin, renderMs, renderSkipped, renderTotal,
//!             outputSkipped: number|null, outputTotal: number|null, freeBytes }
//! ```
//! Event `obs-test` while it runs: `{ attempt: 1, tier: Tier, second: 0..15, seconds: 15 }` (second 0: OBS
//! (re)starts or the capture is checked).
//! Errors: `obs-not-prepared`, `already-recording`, and those of `obs_prepare` for a restart. After an error OBS
//! is closed and put back.
//!
//! `obs_record_start({ watermark?: "top-right" | "top-left" | "bottom-left" | "bottom-right" | "off",
//! seconds?: number })` -> null. `watermark` defaults to top right; `seconds` is the replay's length for the disk
//! check (free >= rate x seconds x 1.2 + 5 GB; without it only the 5 GB). Refuses a black capture.
//! Errors: `obs-not-prepared`, `already-recording`, `disk-full`, `capture-black`, `bad-watermark`.
//! Event `obs-recording` every second:
//! ```text
//! { seconds, bytes, mbps: number|null, fps, renderSkipped, outputSkipped: number|null, freeBytes,
//!   minutesLeft: number|null, done: false }
//! ```
//! and once at the end the same with `done: true, stopped: "driver" | "disk-low" | "outside", path: string|null`
//! (disk-low: stopped by itself below 2 GB free; outside: stopped in OBS).
//!
//! `obs_record_stop()` -> Recorded, also after the recording stopped by itself:
//! ```text
//! { path: string | null, bytes, seconds, mbps: number|null, gbPerHour: number|null, fpsAvg, renderSkipped,
//!   renderTotal, outputSkipped: number|null, outputTotal: number|null, stopped: "driver" | "disk-low" | "outside" }
//! ```
//! Errors: `obs-not-prepared`, `not-recording`.
//!
//! `obs_restore()` -> null. Stops a recording, switches OBS back to the driver's profile and collection, closes it,
//! switches the websocket back off (if it was) and resets `user.ini`. Nothing to do is no error.
//! Errors: `obs-running` (the driver's own OBS runs: close it, then again), `obs-not-closing` (OBS did not close
//! within 20 s: close it by hand, then again).
//!
//! `obs_open_video({ path })` -> null. Opens a recording (an .mp4 in the recording folder) with the default player.
//! Errors: `not-a-recording`.

use std::collections::VecDeque;
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dd_core::obs::config::{self, Paths, UserChoice};
use dd_core::obs::profile::{self, Corner};
use dd_core::obs::stats::{self, Checks, OutputStatus, RecordStatus, Sample, Stats, Summary, Verdict};
use dd_core::obs::tiers::{self, Canvas, Gpu, Hardware, Lag, Tier};
use dd_core::obs::websocket::{self, Message};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

use crate::{log, unix_now};

/// OBS versions before 32 write other encoder ids and keys (plan, decision 1).
const MIN_MAJOR: u32 = 32;
const TEST_SECONDS: u64 = 15;
/// Tiers one quality test tries at most (down from the top is 4, up at most 3).
const MAX_ATTEMPTS: usize = 8;
const START_TIMEOUT: Duration = Duration::from_secs(30);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the game capture may take to show the game.
const HOOK_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_TIMEOUT: Duration = Duration::from_secs(30);
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
/// The folder below the driver's Videos the recordings go to.
const RECORD_FOLDER: &str = "Digital Drivers";
const RESTORE_FILE: &str = "obs-restore.json";
const TESTED_FILE: &str = "obs-tested.json";
/// The DD mark PNGs (research/2026-11-09-testabend/replay/assets/make-watermark.cjs: 75 % opacity and a soft shadow
/// baked in), written into the app's config folder for OBS to load.
const MARKS: [(u32, &[u8]); 2] = [(173, include_bytes!("../assets/obs/dd-mark-173.png")), (230, include_bytes!("../assets/obs/dd-mark-230.png"))];

/// The app's OBS, between `obs_prepare` and `obs_restore`.
#[derive(Default)]
pub struct Obs(Mutex<Option<Session>>);

struct Session {
    install: Install,
    paths: Paths,
    pid: u32,
    client: Arc<Mutex<Client>>,
    gpu: Gpu,
    canvas: Canvas,
    tier: Tier,
    fingerprint: Fingerprint,
    folder: PathBuf,
    config: PathBuf,
    recording: Option<Recording>,
}

struct Recording {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Result<Recorded, String>>,
}

#[derive(Debug, Clone)]
struct Install {
    exe: PathBuf,
    version: Option<String>,
}

/// What a quality test result is valid for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fingerprint {
    gpu: Option<String>,
    driver: Option<String>,
    obs: Option<String>,
    canvas: Canvas,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tested {
    tier: Tier,
    mbps: f64,
    gb_per_hour: f64,
    tested_at: u64,
    fingerprint: Fingerprint,
}

/// What the app changed in the driver's OBS, and how to reach the OBS it started.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Before {
    choice: UserChoice,
    server_enabled: bool,
    pid: Option<u32>,
    port: Option<u16>,
    password: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    installed: bool,
    version: Option<String>,
    supported: Option<bool>,
    running: bool,
    ours: bool,
    busy: bool,
    recording: bool,
    restore_pending: bool,
    gpu: Option<String>,
    hardware: Option<Hardware>,
    canvas: Option<Canvas>,
    tier: Option<Tier>,
    tested: Option<Tested>,
    folder: Option<String>,
    free_bytes: Option<u64>,
    minutes_left: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityTest {
    passed: bool,
    tier: Tier,
    reason: Option<&'static str>,
    mbps: Option<f64>,
    gb_per_hour: Option<f64>,
    minutes_left: Option<u64>,
    capture: Option<Canvas>,
    attempts: Vec<Attempt>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Attempt {
    tier: Tier,
    verdict: Verdict,
    checks: Option<Checks>,
    summary: Option<Summary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestProgress {
    attempt: usize,
    tier: Tier,
    second: u64,
    seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recorded {
    path: Option<String>,
    bytes: u64,
    seconds: f64,
    mbps: Option<f64>,
    gb_per_hour: Option<f64>,
    fps_avg: f64,
    render_skipped: u64,
    render_total: u64,
    output_skipped: Option<u64>,
    output_total: Option<u64>,
    stopped: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordingTick {
    seconds: f64,
    bytes: u64,
    mbps: Option<f64>,
    fps: f64,
    render_skipped: u64,
    output_skipped: Option<u64>,
    free_bytes: u64,
    minutes_left: Option<u64>,
    done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<Option<String>>,
}

fn failed(what: &str, error: impl std::fmt::Display) -> String {
    format!("failed: {what}: {error}")
}

// ---------------------------------------------------------------------------------------------------------------
// obs-websocket over a plain local TCP connection

struct Client {
    ws: tungstenite::WebSocket<TcpStream>,
    next: u64,
    events: VecDeque<(String, Value)>,
}

impl Client {
    fn connect(port: u16, password: &str) -> Result<Client, String> {
        let stream = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2)).map_err(|_| "obs-not-reachable".to_string())?;
        stream.set_read_timeout(Some(REPLY_TIMEOUT)).map_err(|e| failed("socket", e))?;
        stream.set_write_timeout(Some(REPLY_TIMEOUT)).map_err(|e| failed("socket", e))?;
        let (ws, _) = tungstenite::client::client(format!("ws://127.0.0.1:{port}"), stream).map_err(|_| "obs-not-reachable".to_string())?;
        let mut client = Client { ws, next: 0, events: VecDeque::new() };
        let Message::Hello(hello) = client.read()? else { return Err(failed("obs-websocket", "no Hello")) };
        let identify = websocket::identify(&hello, Some(password), websocket::EVENTS_OUTPUTS).map_err(|e| failed("obs-websocket", format!("{e:?}")))?;
        client.send(identify)?;
        match client.read()? {
            Message::Identified => Ok(client),
            other => Err(failed("obs-websocket", format!("{other:?} instead of Identified"))),
        }
    }

    fn send(&mut self, text: String) -> Result<(), String> {
        self.ws.send(tungstenite::Message::text(text)).map_err(|e| failed("obs-websocket", e))
    }

    fn read(&mut self) -> Result<Message, String> {
        loop {
            match self.ws.read() {
                Ok(tungstenite::Message::Text(text)) => return websocket::parse(text.as_str()).map_err(|e| failed("obs-websocket", format!("{e:?}"))),
                Ok(tungstenite::Message::Close(frame)) => {
                    let code = frame.map(|f| u16::from(f.code));
                    return Err(if code == Some(websocket::CLOSE_AUTH_FAILED) { "obs-auth-failed".to_string() } else { failed("obs-websocket closed", format!("{code:?}")) });
                }
                Ok(_) => continue,
                Err(error) => return Err(failed("obs-websocket", error)),
            }
        }
    }

    /// One request; OBS's refusal is an error that names the request.
    fn call(&mut self, kind: &str, data: Option<Value>) -> Result<Value, String> {
        self.next += 1;
        let id = format!("dd-{}", self.next);
        self.send(websocket::request(kind, &id, data.as_ref()))?;
        loop {
            match self.read()? {
                Message::Event { kind, data } => self.events.push_back((kind, data)),
                Message::Response { id: answered, result } if answered == id => {
                    return result.map_err(|refused| failed(kind, format!("{} {}", refused.code, refused.comment)));
                }
                _ => {}
            }
        }
    }

    /// Waits for an event, asking OBS something cheap now and then so events arrive.
    fn wait_event(&mut self, kind: &str, accept: impl Fn(&Value) -> bool, timeout: Duration) -> Option<Value> {
        let until = Instant::now() + timeout;
        loop {
            if let Some(at) = self.events.iter().position(|(k, d)| k == kind && accept(d)) {
                return self.events.remove(at).map(|(_, data)| data);
            }
            if Instant::now() > until {
                return None;
            }
            std::thread::sleep(Duration::from_millis(200));
            self.call("GetRecordStatus", None).ok()?;
        }
    }
}

fn lock(client: &Mutex<Client>) -> std::sync::MutexGuard<'_, Client> {
    client.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------------------------------------------
// Windows: OBS's install, its process and window, the drive

#[cfg(windows)]
fn find_install() -> Option<Install> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    let machine = RegKey::predef(HKEY_LOCAL_MACHINE);
    let dir: String = machine.open_subkey(r"SOFTWARE\OBS Studio").ok()?.get_value("").ok()?;
    let exe = PathBuf::from(dir).join("bin").join("64bit").join("obs64.exe");
    if !exe.is_file() {
        return None;
    }
    let version = machine.open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\OBS Studio").ok().and_then(|key| key.get_value::<String, _>("DisplayVersion").ok());
    Some(Install { exe, version })
}

#[cfg(not(windows))]
fn find_install() -> Option<Install> {
    None
}

/// Whether `tasklist` lists obs64.exe under `filter`.
#[cfg(windows)]
fn obs_listed(filter: &str) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("tasklist").args(["/FI", filter, "/FI", "IMAGENAME eq obs64.exe", "/NH"]).creation_flags(CREATE_NO_WINDOW).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).contains("obs64.exe")).unwrap_or(false)
}

#[cfg(not(windows))]
fn obs_listed(_filter: &str) -> bool {
    false
}

fn obs_running() -> bool {
    obs_listed("STATUS ne UNKNOWN")
}

fn obs_alive(pid: u32) -> bool {
    obs_listed(&format!("PID eq {pid}"))
}

/// Asks OBS's main window to close, as the driver's click on its X would.
#[cfg(windows)]
fn post_close(pid: u32) -> bool {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};

    struct Search {
        pid: u32,
        posted: bool,
    }
    unsafe extern "system" fn each(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `Search` below, alive for the whole EnumWindows call.
        let search = unsafe { &mut *(data.0 as *mut Search) };
        let mut pid = 0u32;
        // SAFETY: plain Win32 calls on a window handle EnumWindows passed in.
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid as *mut u32)) };
        if pid == search.pid {
            let mut title = [0u16; 256];
            let len = unsafe { GetWindowTextW(window, &mut title) };
            // The main window is "OBS 33.0.1 - Profil: ... - Szenen: ..."; Qt's helper windows have other titles.
            if String::from_utf16_lossy(&title[..len.max(0) as usize]).starts_with("OBS ") && unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) }.is_ok() {
                search.posted = true;
            }
        }
        BOOL(1)
    }
    let mut search = Search { pid, posted: false };
    // SAFETY: the callback only touches `search` through the pointer, during this call.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut search as *mut Search as isize)) };
    search.posted
}

#[cfg(not(windows))]
fn post_close(_pid: u32) -> bool {
    false
}

/// Bytes free for the user on the drive of `path` (or of its nearest existing parent).
#[cfg(windows)]
fn disk_free(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(directory: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let dir = path.ancestors().find(|p| p.exists())?;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY: `wide` is a zero-terminated path that outlives the call; the two totals may be null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(available)
}

#[cfg(not(windows))]
fn disk_free(_path: &Path) -> Option<u64> {
    None
}

#[cfg(windows)]
fn open_with_default_app(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("explorer").arg(path).spawn().map(|_| ())
}

#[cfg(not(windows))]
fn open_with_default_app(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("only on Windows"))
}

fn major(version: &str) -> Option<u32> {
    version.split('.').next()?.trim().parse().ok()
}

// ---------------------------------------------------------------------------------------------------------------
// Files

/// OBS's files: `%APPDATA%\obs-studio`, or `<install>\config\obs-studio` in portable mode, and what
/// `global.ini [Locations]` moved.
fn obs_paths(install: &Install) -> Option<Paths> {
    let root = config::install_root(&install.exe)?;
    let portable = config::portable(root);
    let base = if portable { root.join("config") } else { PathBuf::from(std::env::var_os("APPDATA")?) };
    let global = fs::read_to_string(base.join("obs-studio").join("global.ini")).ok();
    Some(config::paths(&base, global.as_deref(), portable, |dir| dir.exists()))
}

fn record_folder(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().video_dir().map(|dir| dir.join(RECORD_FOLDER)).map_err(|e| failed("Videos folder", e))
}

fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| failed("config folder", e))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    fs::create_dir_all(path.parent().expect("a file in a folder")).map_err(|e| failed("config folder", e))?;
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).map_err(|e| failed(&path.display().to_string(), e))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(path.parent().expect("a file in a folder")).map_err(|e| failed(&path.display().to_string(), e))?;
    fs::write(path, bytes).map_err(|e| failed(&path.display().to_string(), e))
}

/// Remembers the driver's profile, collection and websocket switch, unless an earlier run that was not put back
/// remembered them already (those are the driver's own).
fn remember_before(config: &Path, paths: &Paths) -> Result<(), String> {
    let file = config.join(RESTORE_FILE);
    if file.exists() {
        return Ok(());
    }
    let mut choice = config::user_choice(&fs::read_to_string(&paths.user_ini).unwrap_or_default());
    if choice.profile.as_deref() == Some(profile::PROFILE_NAME) {
        (choice.profile, choice.profile_dir) = (None, None);
    }
    if choice.collection.as_deref() == Some(profile::COLLECTION_NAME) {
        (choice.collection, choice.collection_file) = (None, None);
    }
    let server_enabled = config::server_enabled(fs::read_to_string(&paths.websocket).ok().as_deref());
    write_json(&file, &Before { choice, server_enabled, pid: None, port: None, password: None })
}

/// Puts the driver's files back while OBS is closed: the websocket switch and the four keys of `user.ini`.
fn restore_files(paths: &Paths, before: &Before) -> Result<(), String> {
    if let Ok(text) = fs::read_to_string(&paths.websocket) {
        if config::server_enabled(Some(&text)) != before.server_enabled {
            let back = config::with_server_enabled(Some(&text), before.server_enabled).ok_or("obs-config-unreadable")?;
            write_file(&paths.websocket, back.as_bytes())?;
        }
    }
    if let Ok(text) = fs::read_to_string(&paths.user_ini) {
        if let Some(back) = config::with_user_choice(&text, &before.choice) {
            write_file(&paths.user_ini, back.as_bytes())?;
        }
    }
    Ok(())
}

/// Our profile for `tier`, the collection (once: OBS keeps our sources in it) and the websocket switched on.
fn write_obs_files(paths: &Paths, tier: Tier, gpu: Gpu, canvas: Canvas, folder: &Path) -> Result<(), String> {
    let dir = paths.profiles.join(profile::PROFILE_DIR);
    write_file(&dir.join("basic.ini"), profile::basic_ini(tier, canvas, &folder.display().to_string()).as_bytes())?;
    write_file(&dir.join("recordEncoder.json"), profile::record_encoder(tier, gpu).as_bytes())?;
    let collection = paths.scenes.join(profile::COLLECTION_FILE);
    if !collection.exists() {
        write_file(&collection, profile::scene_collection().as_bytes())?;
    }
    let current = fs::read_to_string(&paths.websocket).ok();
    if !config::server_enabled(current.as_deref()) {
        let on = config::with_server_enabled(current.as_deref(), true).ok_or("obs-config-unreadable")?;
        write_file(&paths.websocket, on.as_bytes())?;
    }
    fs::create_dir_all(folder).map_err(|e| failed(&folder.display().to_string(), e))
}

/// The mark PNG for `canvas` in the app's config folder.
fn mark_file(config: &Path, canvas: Canvas) -> Result<PathBuf, String> {
    let width = profile::mark_width(canvas);
    let bytes = MARKS.iter().find(|(w, _)| *w == width).expect("a shipped width").1;
    let path = config.join("obs").join(format!("dd-mark-{width}.png"));
    if fs::read(&path).ok().as_deref() != Some(bytes) {
        write_file(&path, bytes)?;
    }
    Ok(path)
}

fn tested_for(config: &Path, fingerprint: &Fingerprint) -> Option<Tested> {
    read_json::<Tested>(&config.join(TESTED_FILE)).filter(|tested| &tested.fingerprint == fingerprint)
}

/// Whether `path` is a file inside `folder`.
fn inside(path: &Path, folder: &Path) -> bool {
    match (fs::canonicalize(path), fs::canonicalize(folder)) {
        (Ok(path), Ok(folder)) => path.starts_with(folder) && path.is_file(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Starting, checking and closing OBS

fn free_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0").and_then(|listener| listener.local_addr()).map(|addr| addr.port()).map_err(|e| failed("free port", e))
}

/// A password for this start of OBS only: OS randomness (std's hasher keys) and the time, hashed.
fn new_password() -> String {
    use sha2::{Digest, Sha256};
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    let digest = Sha256::digest(format!("{}{}", hasher.finish(), std::process::id()));
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// Starts OBS with our profile and collection, a websocket port and password of its own, minimised to the tray.
fn launch(install: &Install, port: u16, password: &str) -> Result<u32, String> {
    let bin = install.exe.parent().ok_or("obs-not-found")?;
    let port = port.to_string();
    let child = std::process::Command::new(&install.exe)
        .current_dir(bin)
        .args(["--profile", profile::PROFILE_NAME, "--collection", profile::COLLECTION_NAME, "--websocket_port", &port, "--websocket_password", password, "--disable-updater", "--disable-missing-files-check", "--minimize-to-tray"])
        .spawn()
        .map_err(|e| failed("starting OBS", e))?;
    Ok(child.id())
}

fn connect_when_up(port: u16, password: &str, pid: u32) -> Result<Client, String> {
    let until = Instant::now() + START_TIMEOUT;
    loop {
        match Client::connect(port, password) {
            Ok(client) => return Ok(client),
            Err(error) if error == "obs-auth-failed" => return Err(error),
            Err(_) if Instant::now() < until => {
                if !obs_alive(pid) {
                    return Err("obs-exited".to_string());
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(_) => return Err("obs-not-reachable".to_string()),
        }
    }
}

/// The name of the current program scene.
fn scene(client: &mut Client) -> Result<String, String> {
    Ok(client.call("GetCurrentProgramScene", None)?["currentProgramSceneName"].as_str().unwrap_or(profile::SCENE_NAME).to_string())
}

fn item_id(client: &mut Client, scene: &str, source: &str) -> Result<u64, String> {
    client.call("GetSceneItemId", Some(json!({ "sceneName": scene, "sourceName": source })))?["sceneItemId"].as_u64().ok_or_else(|| failed("GetSceneItemId", source))
}

/// Points the capture and the sound at the game's window, when OBS lists it (the game runs).
fn pick_window(client: &mut Client) -> String {
    let listed = client.call("GetInputPropertiesListPropertyItems", Some(json!({ "inputName": profile::GAME_SOURCE, "propertyName": "window" }))).ok();
    match listed.as_ref().and_then(profile::game_window) {
        Some(window) => {
            let _ = client.call("SetInputSettings", Some(json!({ "inputName": profile::GAME_SOURCE, "inputSettings": { "window": window } })));
            let _ = client.call("SetInputSettings", Some(json!({ "inputName": profile::AUDIO_SOURCE, "inputSettings": profile::game_audio_settings(&window) })));
            window
        }
        None => profile::GAME_WINDOW_GUESS.to_string(),
    }
}

/// Creates what is missing of the three sources in our scene, fits the game to the canvas and puts the mark on top.
fn set_up_sources(client: &mut Client, canvas: Canvas, mark: &Path) -> Result<(), String> {
    let scene = scene(client)?;
    let items = client.call("GetSceneItemList", Some(json!({ "sceneName": scene })))?;
    let has = |name: &str| items["sceneItems"].as_array().is_some_and(|items| items.iter().any(|item| item["sourceName"] == name));
    let create = |client: &mut Client, name: &str, kind: &str, settings: Value| client.call("CreateInput", Some(json!({ "sceneName": scene, "inputName": name, "inputKind": kind, "inputSettings": settings, "sceneItemEnabled": true }))).map(|_| ());
    if !has(profile::GAME_SOURCE) {
        create(client, profile::GAME_SOURCE, "game_capture", profile::game_capture_settings(profile::GAME_WINDOW_GUESS))?;
    }
    if !has(profile::AUDIO_SOURCE) {
        create(client, profile::AUDIO_SOURCE, "wasapi_process_output_capture", profile::game_audio_settings(profile::GAME_WINDOW_GUESS))?;
    }
    let file = mark.display().to_string();
    if has(profile::MARK_SOURCE) {
        client.call("SetInputSettings", Some(json!({ "inputName": profile::MARK_SOURCE, "inputSettings": profile::mark_settings(&file) })))?;
    } else {
        create(client, profile::MARK_SOURCE, "image_source", profile::mark_settings(&file))?;
    }
    pick_window(client);
    let game = item_id(client, &scene, profile::GAME_SOURCE)?;
    client.call("SetSceneItemTransform", Some(json!({ "sceneName": scene, "sceneItemId": game, "sceneItemTransform": profile::game_transform(canvas) })))?;
    client.call("SetSceneItemEnabled", Some(json!({ "sceneName": scene, "sceneItemId": game, "sceneItemEnabled": true })))?;
    place_mark(client, canvas, Some(Corner::default()))
}

/// The mark in `corner` on top of the game, or hidden.
fn place_mark(client: &mut Client, canvas: Canvas, corner: Option<Corner>) -> Result<(), String> {
    let scene = scene(client)?;
    let mark = item_id(client, &scene, profile::MARK_SOURCE)?;
    if let Some(corner) = corner {
        client.call("SetSceneItemTransform", Some(json!({ "sceneName": scene, "sceneItemId": mark, "sceneItemTransform": profile::mark_transform(corner, canvas, profile::mark_width(canvas)) })))?;
        let count = client.call("GetSceneItemList", Some(json!({ "sceneName": scene })))?["sceneItems"].as_array().map_or(1, Vec::len);
        client.call("SetSceneItemIndex", Some(json!({ "sceneName": scene, "sceneItemId": mark, "sceneItemIndex": count.saturating_sub(1) })))?;
    }
    client.call("SetSceneItemEnabled", Some(json!({ "sceneName": scene, "sceneItemId": mark, "sceneItemEnabled": corner.is_some() })))?;
    Ok(())
}

/// Whether the game capture shows the game (None when OBS gives no BMP screenshot), waiting for the hook, and the
/// size of the game's image.
fn check_capture(client: &mut Client) -> Result<(Option<bool>, Option<Canvas>), String> {
    pick_window(client);
    let bmp = client.call("GetVersion", None)?["supportedImageFormats"].as_array().is_some_and(|formats| formats.iter().any(|f| f == "bmp"));
    let mut seen = None;
    let until = Instant::now() + HOOK_TIMEOUT;
    while bmp && seen != Some(true) && Instant::now() <= until {
        let shot = client.call("GetSourceScreenshot", Some(json!({ "sourceName": profile::GAME_SOURCE, "imageFormat": "bmp", "imageWidth": 320 }))).ok();
        if let Some(luma) = shot.and_then(|s| s["imageData"].as_str().and_then(stats::data_uri_bytes)).and_then(|bytes| stats::bmp_mean_luma(&bytes)) {
            seen = Some(luma > stats::MIN_LUMA);
        }
        if seen != Some(true) {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let scene = scene(client)?;
    let game = item_id(client, &scene, profile::GAME_SOURCE)?;
    let transform = client.call("GetSceneItemTransform", Some(json!({ "sceneName": scene, "sceneItemId": game })))?;
    let size = |key: &str| transform["sceneItemTransform"][key].as_f64().map(|v| v.round() as u32).filter(|&v| v > 0);
    Ok((seen, size("sourceWidth").zip(size("sourceHeight")).map(|(width, height)| Canvas { width, height })))
}

/// Starts OBS for `session`'s tier and connects: OBS must be ours, 32 or newer, with our profile and collection.
fn start_obs(config: &Path, install: &Install, canvas: Canvas) -> Result<(u32, Client), String> {
    let (port, password) = (free_port()?, new_password());
    let pid = launch(install, port, &password)?;
    log(&format!("obs: started OBS {} (pid {pid}, websocket port {port})", install.version.as_deref().unwrap_or("?")));
    if let Some(mut before) = read_json::<Before>(&config.join(RESTORE_FILE)) {
        (before.pid, before.port, before.password) = (Some(pid), Some(port), Some(password.clone()));
        write_json(&config.join(RESTORE_FILE), &before)?;
    }
    let checked = (|| {
        let mut client = connect_when_up(port, &password, pid)?;
        let version = client.call("GetVersion", None)?;
        if version["obsVersion"].as_str().and_then(major).is_some_and(|m| m < MIN_MAJOR) {
            return Err("obs-too-old".to_string());
        }
        let profile = client.call("GetProfileList", None)?["currentProfileName"].as_str().map(str::to_string);
        let collection = client.call("GetSceneCollectionList", None)?["currentSceneCollectionName"].as_str().map(str::to_string);
        if profile.as_deref() != Some(profile::PROFILE_NAME) || collection.as_deref() != Some(profile::COLLECTION_NAME) {
            log(&format!("obs: OBS loaded profile {profile:?} and collection {collection:?}, not ours"));
            return Err("obs-profile-not-loaded".to_string());
        }
        set_up_sources(&mut client, canvas, &mark_file(config, canvas)?)?;
        Ok(client)
    })();
    match checked {
        Ok(client) => Ok((pid, client)),
        Err(error) => {
            log(&format!("obs: start failed: {error}"));
            // Ours, but not usable: close it, so the driver's files can be put back.
            let _ = close_obs(None, pid, config);
            Err(error)
        }
    }
}

/// Switches OBS back to the driver's profile and collection (when connected) and closes it gracefully.
fn close_obs(client: Option<&mut Client>, pid: u32, config: &Path) -> Result<(), String> {
    if let Some(client) = client {
        if client.call("GetRecordStatus", None).ok().is_some_and(|status| status["outputActive"] == true) {
            let _ = stop_record(client);
        }
        if let Some(before) = read_json::<Before>(&config.join(RESTORE_FILE)) {
            let listed = |client: &mut Client, request: &str, list: &str, name: &str| client.call(request, None).ok().is_some_and(|v| v[list].as_array().is_some_and(|names| names.iter().any(|n| n == name)));
            if let Some(collection) = before.choice.collection.as_deref().filter(|c| listed(client, "GetSceneCollectionList", "sceneCollections", c)) {
                let _ = client.call("SetCurrentSceneCollection", Some(json!({ "sceneCollectionName": collection })));
            }
            if let Some(name) = before.choice.profile.as_deref().filter(|p| listed(client, "GetProfileList", "profiles", p)) {
                let _ = client.call("SetCurrentProfile", Some(json!({ "profileName": name })));
            }
            // OBS loads the collection and profile before it answers; the record script waited 2 s, too.
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    if !obs_alive(pid) {
        return Ok(());
    }
    if !post_close(pid) {
        return Err("obs-not-closing".to_string());
    }
    let until = Instant::now() + CLOSE_TIMEOUT;
    while obs_alive(pid) {
        if Instant::now() > until {
            log(&format!("obs: OBS (pid {pid}) did not close"));
            return Err("obs-not-closing".to_string());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    log(&format!("obs: OBS (pid {pid}) closed"));
    Ok(())
}

/// Closes the session's OBS, rewrites the profile for `tier` and starts it again.
fn relaunch(session: &mut Session, tier: Tier) -> Result<(), String> {
    close_obs(Some(&mut *lock(&session.client)), session.pid, &session.config)?;
    write_obs_files(&session.paths, tier, session.gpu, session.canvas, &session.folder)?;
    let (pid, client) = start_obs(&session.config, &session.install, session.canvas)?;
    (session.pid, session.client, session.tier) = (pid, Arc::new(Mutex::new(client)), tier);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Recording

fn sample(client: &mut Client) -> Result<Sample, String> {
    let stats: Stats = serde_json::from_value(client.call("GetStats", None)?).map_err(|e| failed("GetStats", e))?;
    let record: RecordStatus = serde_json::from_value(client.call("GetRecordStatus", None)?).map_err(|e| failed("GetRecordStatus", e))?;
    let output = client.call("GetOutputStatus", Some(json!({ "outputName": "adv_file_output" }))).ok().and_then(|v| serde_json::from_value::<OutputStatus>(v).ok());
    Ok(Sample { stats, record, output })
}

/// Starts recording; answers the statistics before, the baseline of the deltas.
fn start_record(client: &mut Client) -> Result<Stats, String> {
    if client.call("GetRecordStatus", None)?["outputActive"] == true {
        return Err("already-recording".to_string());
    }
    let baseline = serde_json::from_value(client.call("GetStats", None)?).map_err(|e| failed("GetStats", e))?;
    client.events.clear();
    client.call("StartRecord", None)?;
    Ok(baseline)
}

/// Stops recording and waits until the file is complete (Hybrid MP4 writes its index on stop); answers its path.
fn stop_record(client: &mut Client) -> Option<PathBuf> {
    let answer = client.call("StopRecord", None).ok();
    let stopped = |data: &Value| websocket::record_state(data).is_some_and(|(state, _)| state == websocket::OUTPUT_STOPPED);
    let event = client.wait_event("RecordStateChanged", stopped, STOP_TIMEOUT);
    let path = answer.as_ref().and_then(|a| a["outputPath"].as_str()).or_else(|| event.as_ref().and_then(|e| e["outputPath"].as_str())).map(PathBuf::from)?;
    let mut last = None;
    for _ in 0..20 {
        let size = fs::metadata(&path).ok().map(|m| m.len());
        if size.is_some() && size == last {
            break;
        }
        last = size;
        std::thread::sleep(Duration::from_millis(500));
    }
    Some(path)
}

/// A summary with the final file's size instead of the estimate.
fn with_file(mut summary: Summary, path: Option<&Path>) -> Summary {
    if let Some(bytes) = path.and_then(|p| fs::metadata(p).ok()).map(|m| m.len()) {
        summary.bytes = bytes;
        summary.mbps = stats::mbps(bytes, summary.seconds);
    }
    summary
}

/// One rung of the quality test: the capture check and 15 s of recording, the clip deleted.
fn test_once(app: &tauri::AppHandle, session: &Session, attempt: usize) -> Result<(Attempt, Option<Canvas>), String> {
    let mut client = lock(&session.client);
    let progress = |second| {
        let _ = app.emit("obs-test", TestProgress { attempt, tier: session.tier, second, seconds: TEST_SECONDS });
    };
    progress(0);
    let (seen, capture) = check_capture(&mut client)?;
    if seen == Some(false) {
        return Ok((Attempt { tier: session.tier, verdict: Verdict::CaptureBlack, checks: None, summary: None }, capture));
    }
    place_mark(&mut client, session.canvas, Some(Corner::default()))?;
    let baseline = start_record(&mut client)?;
    let started = Instant::now();
    let mut samples = Vec::new();
    for second in 1..=TEST_SECONDS {
        std::thread::sleep((started + Duration::from_secs(second)).saturating_duration_since(Instant::now()));
        samples.push(sample(&mut client)?);
        progress(second);
    }
    let path = stop_record(&mut client);
    let summary = stats::summarize(&baseline, &samples).map(|s| with_file(s, path.as_deref()));
    if let Some(path) = path.filter(|p| inside(p, &session.folder)) {
        let _ = fs::remove_file(path);
    }
    let summary = summary.ok_or_else(|| failed("quality test", "no samples"))?;
    let (checks, verdict) = stats::judge(&summary, seen);
    log(&format!("obs: test {attempt} {:?} -> {verdict:?}, {:?} Mb/s, render {}/{}, output {:?}/{:?}, {:.1} fps", session.tier, summary.mbps, summary.render_skipped, summary.render_total, summary.output_skipped, summary.output_total, summary.fps_avg));
    Ok((Attempt { tier: session.tier, verdict, checks: Some(checks), summary: Some(summary) }, capture))
}

fn verdict_reason(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "pass",
        Verdict::EncodingLag => "encoding-lag",
        Verdict::RenderingLag => "rendering-lag",
        Verdict::CaptureBlack => "capture-black",
        Verdict::Unknown => "unknown",
    }
}

fn run_ladder(app: &tauri::AppHandle, session: &mut Session) -> Result<QualityTest, String> {
    let mut attempts = Vec::new();
    let mut chosen: Option<(Tier, f64)> = None;
    let (mut going_up, mut went_down, mut reason, mut capture) = (false, false, None, None);
    let mut tier = session.tier;
    for attempt in 1..=MAX_ATTEMPTS {
        if tier != session.tier {
            let _ = app.emit("obs-test", TestProgress { attempt, tier, second: 0, seconds: TEST_SECONDS });
            relaunch(session, tier)?;
        }
        let (result, seen) = test_once(app, session, attempt)?;
        capture = seen.or(capture);
        let verdict = result.verdict;
        let rate = result.summary.as_ref().and_then(|s| s.mbps);
        attempts.push(result);
        match verdict {
            Verdict::Pass => {
                chosen = Some((tier, rate.unwrap_or(0.0)));
                reason = None;
                match tiers::step_up(tier, session.gpu, session.canvas).filter(|_| !went_down) {
                    Some(up) => (going_up, tier) = (true, up),
                    None => break,
                }
            }
            Verdict::CaptureBlack | Verdict::Unknown => {
                reason = Some(verdict_reason(verdict));
                break;
            }
            Verdict::EncodingLag | Verdict::RenderingLag => {
                if going_up {
                    break;
                }
                let lag = if verdict == Verdict::EncodingLag { Lag::Encoding } else { Lag::Rendering };
                reason = Some(verdict_reason(verdict));
                match tiers::step_down(tier, lag) {
                    Some(down) => (went_down, tier) = (true, down),
                    None => break,
                }
            }
        }
    }
    let left_on = chosen.map_or(session.tier, |(tier, _)| tier);
    if left_on != session.tier {
        relaunch(session, left_on)?;
    }
    let free = disk_free(&session.folder);
    let mbps = chosen.map(|(_, rate)| rate).filter(|&r| r > 0.0);
    if let (Some((tier, _)), Some(mbps)) = (chosen, mbps) {
        let tested = Tested { tier, mbps, gb_per_hour: stats::gb_per_hour(mbps), tested_at: unix_now(), fingerprint: session.fingerprint.clone() };
        write_json(&session.config.join(TESTED_FILE), &tested)?;
    }
    Ok(QualityTest {
        passed: chosen.is_some(),
        tier: left_on,
        reason,
        mbps,
        gb_per_hour: mbps.map(stats::gb_per_hour),
        minutes_left: free.zip(mbps).and_then(|(free, mbps)| stats::minutes_left(free, mbps)),
        capture,
        attempts,
    })
}

/// The recording's loop: a tick every second, until the driver stops it, the drive is nearly full or OBS stopped.
fn record_loop(app: tauri::AppHandle, client: Arc<Mutex<Client>>, baseline: Stats, stop: Arc<AtomicBool>) -> Result<Recorded, String> {
    let started = Instant::now();
    let mut samples: Vec<Sample> = Vec::new();
    let mut second = 0;
    let stopped = loop {
        second += 1;
        while Instant::now() < started + Duration::from_secs(second) {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if stop.load(Ordering::Relaxed) {
            break "driver";
        }
        let now = sample(&mut lock(&client))?;
        samples.push(now);
        if let Some(summary) = stats::summarize(&baseline, &samples) {
            let _ = app.emit("obs-recording", tick(&summary, now.stats.active_fps, false, None, None));
            if summary.free_bytes < stats::STOP_BELOW_BYTES {
                break "disk-low";
            }
        }
        if !now.record.output_active {
            break "outside";
        }
    };
    let path = {
        let mut client = lock(&client);
        if stopped == "outside" {
            let done = |data: &Value| websocket::record_state(data).is_some_and(|(state, _)| state == websocket::OUTPUT_STOPPED);
            client.wait_event("RecordStateChanged", done, STOP_TIMEOUT).and_then(|data| data["outputPath"].as_str().map(PathBuf::from))
        } else {
            stop_record(&mut client)
        }
    };
    let summary = stats::summarize(&baseline, &samples).map(|s| with_file(s, path.as_deref()));
    let path = path.map(|p| p.display().to_string());
    log(&format!("obs: recording stopped ({stopped}) after {second} s: {path:?}"));
    let fps = samples.last().map_or(0.0, |s| s.stats.active_fps);
    let recorded = match &summary {
        Some(s) => Recorded {
            path: path.clone(),
            bytes: s.bytes,
            seconds: s.seconds,
            mbps: s.mbps,
            gb_per_hour: s.mbps.map(stats::gb_per_hour),
            fps_avg: s.fps_avg,
            render_skipped: s.render_skipped,
            render_total: s.render_total,
            output_skipped: s.output_skipped,
            output_total: s.output_total,
            stopped,
        },
        None => Recorded { path: path.clone(), bytes: 0, seconds: 0.0, mbps: None, gb_per_hour: None, fps_avg: 0.0, render_skipped: 0, render_total: 0, output_skipped: None, output_total: None, stopped },
    };
    if let Some(summary) = &summary {
        let _ = app.emit("obs-recording", tick(summary, fps, true, Some(stopped), Some(path)));
    }
    Ok(recorded)
}

fn tick(summary: &Summary, fps: f64, done: bool, stopped: Option<&'static str>, path: Option<Option<String>>) -> RecordingTick {
    RecordingTick {
        seconds: summary.seconds,
        bytes: summary.bytes,
        mbps: summary.mbps,
        fps,
        render_skipped: summary.render_skipped,
        output_skipped: summary.output_skipped,
        free_bytes: summary.free_bytes,
        minutes_left: summary.mbps.and_then(|mbps| stats::minutes_left(summary.free_bytes, mbps)),
        done,
        stopped,
        path,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The commands

fn state(app: &tauri::AppHandle) -> &Obs {
    app.state::<Obs>().inner()
}

fn fingerprint(video: &dd_core::game_check::GameVideo, install: Option<&Install>) -> Fingerprint {
    Fingerprint { gpu: video.gpu.clone(), driver: video.driver.clone(), obs: install.and_then(|i| i.version.clone()), canvas: Canvas { width: video.window_width, height: video.window_height } }
}

pub fn status(app: &tauri::AppHandle) -> Status {
    let install = find_install();
    let version = install.as_ref().and_then(|i| i.version.clone());
    let config = config_dir(app).ok();
    let folder = record_folder(app).ok();
    let free_bytes = folder.as_deref().and_then(disk_free);
    let mut status = Status {
        installed: install.is_some(),
        supported: version.as_deref().and_then(major).map(|m| m >= MIN_MAJOR),
        version,
        running: obs_running(),
        ours: false,
        busy: false,
        recording: false,
        restore_pending: config.as_ref().is_some_and(|c| c.join(RESTORE_FILE).exists()),
        gpu: None,
        hardware: None,
        canvas: None,
        tier: None,
        tested: None,
        folder: folder.as_ref().map(|f| f.display().to_string()),
        free_bytes,
        minutes_left: None,
    };
    let current = match state(app).0.try_lock() {
        Ok(session) => session.as_ref().map(|s| (s.tier, s.recording.is_some())),
        Err(TryLockError::WouldBlock) => {
            status.busy = true;
            None
        }
        Err(TryLockError::Poisoned(_)) => None,
    };
    status.ours = status.busy || current.is_some();
    status.recording = current.is_some_and(|(_, recording)| recording);
    if let Ok(video) = crate::replays::video(app) {
        let canvas = Canvas { width: video.window_width, height: video.window_height };
        let gpu = video.gpu.as_deref().and_then(|name| tiers::gpu(video.vendor, name));
        let tested = config.as_deref().and_then(|c| tested_for(c, &fingerprint(&video, install.as_ref())));
        let tier = current.map(|(tier, _)| tier).or(tested.as_ref().map(|t| t.tier)).or(gpu.map(|g| tiers::start(g, canvas)));
        let mbps = tested.as_ref().filter(|t| Some(t.tier) == tier).map(|t| t.mbps).or(tier.map(|t| stats::estimated_mbps(t, canvas)));
        status.minutes_left = free_bytes.zip(mbps).and_then(|(free, mbps)| stats::minutes_left(free, mbps));
        (status.gpu, status.hardware, status.canvas, status.tier, status.tested) = (video.gpu.clone(), gpu.map(|g| g.hardware), Some(canvas), tier, tested);
    }
    status
}

pub fn prepare(app: &tauri::AppHandle) -> Result<Status, String> {
    {
        let mut slot = state(app).0.lock().map_err(|_| failed("state", "poisoned"))?;
        if slot.is_none() {
            *slot = Some(open_session(app)?);
        }
    }
    Ok(status(app))
}

fn open_session(app: &tauri::AppHandle) -> Result<Session, String> {
    let install = find_install().ok_or("obs-not-found")?;
    if install.version.as_deref().and_then(major).is_some_and(|m| m < MIN_MAJOR) {
        return Err("obs-too-old".to_string());
    }
    if obs_running() {
        return Err("obs-running".to_string());
    }
    let video = crate::replays::video(app)?;
    let gpu = video.gpu.as_deref().and_then(|name| tiers::gpu(video.vendor, name)).ok_or("no-hardware-encoder")?;
    let canvas = Canvas { width: video.window_width, height: video.window_height };
    if canvas.width == 0 || canvas.height == 0 {
        return Err("no-video-settings".to_string());
    }
    let config = config_dir(app)?;
    let fingerprint = fingerprint(&video, Some(&install));
    let tier = tested_for(&config, &fingerprint).map_or_else(|| tiers::start(gpu, canvas), |tested| tested.tier);
    let paths = obs_paths(&install).ok_or_else(|| failed("OBS config", "no APPDATA"))?;
    let folder = record_folder(app)?;
    remember_before(&config, &paths)?;
    let started = write_obs_files(&paths, tier, gpu, canvas, &folder).and_then(|()| start_obs(&config, &install, canvas));
    let (pid, client) = match started {
        Ok(started) => started,
        Err(error) => {
            // OBS is closed again (start_obs closes what it started): put the driver's files back.
            if !obs_running() {
                if let Some(before) = read_json::<Before>(&config.join(RESTORE_FILE)) {
                    if restore_files(&paths, &before).is_ok() {
                        let _ = fs::remove_file(config.join(RESTORE_FILE));
                    }
                }
            }
            return Err(error);
        }
    };
    log(&format!("obs: prepared {tier:?} on {canvas:?} for {:?}", video.gpu));
    Ok(Session { install, paths, pid, client: Arc::new(Mutex::new(client)), gpu, canvas, tier, fingerprint, folder, config, recording: None })
}

pub fn quality_test(app: &tauri::AppHandle) -> Result<QualityTest, String> {
    let mut slot = state(app).0.lock().map_err(|_| failed("state", "poisoned"))?;
    let session = slot.as_mut().ok_or("obs-not-prepared")?;
    if session.recording.is_some() {
        return Err("already-recording".to_string());
    }
    match run_ladder(app, session) {
        Ok(result) => Ok(result),
        Err(error) => {
            log(&format!("obs: quality test failed: {error}"));
            let session = slot.take();
            drop(slot);
            if let Err(also) = restore_session(app, session) {
                log(&format!("obs: putting OBS back failed too: {also}"));
            }
            Err(error)
        }
    }
}

pub fn record_start(app: &tauri::AppHandle, watermark: Option<String>, seconds: Option<f64>) -> Result<(), String> {
    let corner = match watermark.as_deref() {
        None => Some(Corner::default()),
        Some("off") => None,
        Some(corner) => Some(serde_json::from_value::<Corner>(json!(corner)).map_err(|_| "bad-watermark")?),
    };
    let mut slot = state(app).0.lock().map_err(|_| failed("state", "poisoned"))?;
    let session = slot.as_mut().ok_or("obs-not-prepared")?;
    if session.recording.as_ref().is_some_and(|r| !r.thread.is_finished()) {
        return Err("already-recording".to_string());
    }
    let mbps = tested_for(&session.config, &session.fingerprint).filter(|t| t.tier == session.tier).map_or_else(|| stats::estimated_mbps(session.tier, session.canvas), |t| t.mbps);
    if let Some(free) = disk_free(&session.folder) {
        if free < stats::bytes_needed(mbps, seconds.unwrap_or(0.0)) {
            return Err("disk-full".to_string());
        }
    }
    let baseline = {
        let mut client = lock(&session.client);
        let (seen, _) = check_capture(&mut client)?;
        if seen == Some(false) {
            return Err("capture-black".to_string());
        }
        place_mark(&mut client, session.canvas, corner)?;
        start_record(&mut client)?
    };
    log(&format!("obs: recording {:?}, watermark {corner:?}", session.tier));
    let stop = Arc::new(AtomicBool::new(false));
    let (app, client, flag) = (app.clone(), session.client.clone(), stop.clone());
    let thread = std::thread::spawn(move || record_loop(app, client, baseline, flag));
    session.recording = Some(Recording { stop, thread });
    Ok(())
}

pub fn record_stop(app: &tauri::AppHandle) -> Result<Recorded, String> {
    let mut slot = state(app).0.lock().map_err(|_| failed("state", "poisoned"))?;
    let session = slot.as_mut().ok_or("obs-not-prepared")?;
    let recording = session.recording.take().ok_or("not-recording")?;
    recording.stop.store(true, Ordering::Relaxed);
    recording.thread.join().map_err(|_| failed("recording", "the loop panicked"))?
}

pub fn restore(app: &tauri::AppHandle) -> Result<(), String> {
    let session = state(app).0.lock().map_err(|_| failed("state", "poisoned"))?.take();
    restore_session(app, session)
}

/// Closes the app's OBS (the session's, or the one an earlier run of the app started) and puts the driver's
/// files back.
fn restore_session(app: &tauri::AppHandle, session: Option<Session>) -> Result<(), String> {
    let config = config_dir(app)?;
    let before = read_json::<Before>(&config.join(RESTORE_FILE));
    let mut session = session;
    if let Some(recording) = session.as_mut().and_then(|s| s.recording.take()) {
        recording.stop.store(true, Ordering::Relaxed);
        let _ = recording.thread.join();
    }
    let pid = session.as_ref().map(|s| s.pid).or(before.as_ref().and_then(|b| b.pid)).filter(|&pid| obs_alive(pid));
    if let Some(pid) = pid {
        let client = match &session {
            Some(s) => Some(s.client.clone()),
            None => before.as_ref().and_then(|b| Client::connect(b.port?, b.password.as_deref()?).ok()).map(|c| Arc::new(Mutex::new(c))),
        };
        match &client {
            Some(client) => close_obs(Some(&mut *lock(client)), pid, &config)?,
            None => close_obs(None, pid, &config)?,
        }
    } else if before.is_some() && obs_running() {
        return Err("obs-running".to_string());
    }
    let Some(before) = before else { return Ok(()) };
    let paths = session.map(|s| s.paths).or_else(|| find_install().as_ref().and_then(obs_paths)).ok_or("obs-not-found")?;
    restore_files(&paths, &before)?;
    fs::remove_file(config.join(RESTORE_FILE)).map_err(|e| failed("obs-restore.json", e))?;
    log("obs: put back as it was");
    Ok(())
}

pub fn open_video(app: &tauri::AppHandle, path: &str) -> Result<(), String> {
    let folder = record_folder(app)?;
    let path = PathBuf::from(path);
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) || !inside(&path, &folder) {
        return Err("not-a-recording".to_string());
    }
    open_with_default_app(&path).map_err(|e| failed("opening the video", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tungstenite::protocol::frame::coding::CloseCode;
    use tungstenite::protocol::CloseFrame;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dd-desktop-obs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A minimal obs-websocket on a free port: the real handshake with the protocol guide's salt and challenge,
    /// then `answer` for every request, one event before the first response.
    fn fake_obs(password: &'static str, answer: fn(&Value) -> Value) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let (salt, challenge) = ("lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=", "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY=");
            ws.send(tungstenite::Message::text(json!({ "op": 0, "d": { "obsStudioVersion": "33.0.1", "obsWebSocketVersion": "5.7.6", "rpcVersion": 1, "authentication": { "salt": salt, "challenge": challenge } } }).to_string())).unwrap();
            let identify: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            if identify["d"]["authentication"] != websocket::auth(password, salt, challenge) {
                let _ = ws.close(Some(CloseFrame { code: CloseCode::from(4009), reason: "Authentication failed.".into() }));
                let _ = ws.flush();
                let _ = ws.read();
                return;
            }
            ws.send(tungstenite::Message::text(r#"{"op":2,"d":{"negotiatedRpcVersion":1}}"#)).unwrap();
            let mut first = true;
            while let Ok(message) = ws.read() {
                let Ok(text) = message.to_text() else { break };
                let request: Value = serde_json::from_str(text).unwrap();
                if first {
                    ws.send(tungstenite::Message::text(json!({ "op": 5, "d": { "eventType": "RecordStateChanged", "eventIntent": 64, "eventData": { "outputActive": false, "outputState": "OBS_WEBSOCKET_OUTPUT_STOPPED", "outputPath": "C:/v/DD 2026-11-09 20-58-01.mp4" } } }).to_string())).unwrap();
                    first = false;
                }
                let d = &request["d"];
                let data = answer(d);
                let ok = !data.is_null();
                ws.send(tungstenite::Message::text(json!({ "op": 7, "d": { "requestType": d["requestType"], "requestId": d["requestId"], "requestStatus": { "result": ok, "code": if ok { 100 } else { 600 } }, "responseData": data } }).to_string())).unwrap();
            }
        });
        port
    }

    fn answers(d: &Value) -> Value {
        match d["requestType"].as_str() {
            Some("GetVersion") => json!({ "obsVersion": "33.0.1", "supportedImageFormats": ["bmp", "jpeg", "png"] }),
            Some("GetOutputStatus") if d["requestData"]["outputName"] == "adv_file_output" => json!({ "outputActive": true, "outputSkippedFrames": 0, "outputTotalFrames": 900 }),
            _ => Value::Null,
        }
    }

    #[test]
    fn logs_in_asks_and_keeps_the_events_that_come_between() {
        let port = fake_obs("secret", answers);
        let mut client = Client::connect(port, "secret").unwrap();
        assert_eq!(client.call("GetVersion", None).unwrap()["obsVersion"], "33.0.1");
        let output: OutputStatus = serde_json::from_value(client.call("GetOutputStatus", Some(json!({ "outputName": "adv_file_output" }))).unwrap()).unwrap();
        assert_eq!(output.output_total_frames, 900);
        assert!(client.call("GetSceneItemId", None).unwrap_err().starts_with("failed: GetSceneItemId: 600"));
        let stopped = client.wait_event("RecordStateChanged", |d| d["outputState"] == websocket::OUTPUT_STOPPED, Duration::from_secs(1)).unwrap();
        assert_eq!(stopped["outputPath"], "C:/v/DD 2026-11-09 20-58-01.mp4");
    }

    #[test]
    fn says_so_when_the_password_is_wrong() {
        let port = fake_obs("secret", answers);
        assert_eq!(Client::connect(port, "wrong").err().as_deref(), Some("obs-auth-failed"));
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(Client::connect(closed, "secret").err().as_deref(), Some("obs-not-reachable"));
    }

    #[test]
    fn remembers_the_drivers_obs_and_puts_it_back() {
        let dir = temp_dir("restore");
        let obs = dir.join("obs-studio");
        let paths = config::paths(&dir, None, false, |_| true);
        write_file(&paths.user_ini, "\u{feff}[Basic]\r\nProfile=0xJoe\r\nProfileDir=0xJoe\r\nSceneCollection=Unbenannt\r\nSceneCollectionFile=Unbenannt\r\n".as_bytes()).unwrap();
        write_file(&paths.websocket, br#"{"auth_required":true,"first_load":false,"server_enabled":false,"server_password":"joes","server_port":4455}"#).unwrap();
        let config = dir.join("app");
        remember_before(&config, &paths).unwrap();
        let gpu = tiers::gpu(dd_core::game_check::Vendor::Nvidia, "NVIDIA GeForce RTX 3070").unwrap();
        let canvas = Canvas { width: 1920, height: 1080 };
        let folder = dir.join("Videos").join(RECORD_FOLDER);
        write_obs_files(&paths, tiers::start(gpu, canvas), gpu, canvas, &folder).unwrap();
        assert!(obs.join("basic").join("profiles").join("DigitalDrivers").join("basic.ini").is_file());
        assert!(obs.join("basic").join("scenes").join("DigitalDrivers.json").is_file());
        assert!(folder.is_dir());
        assert!(config::server_enabled(fs::read_to_string(&paths.websocket).ok().as_deref()));
        // OBS ran with ours and wrote it into user.ini
        write_file(&paths.user_ini, "\u{feff}[Basic]\r\nProfile=Digital Drivers\r\nProfileDir=DigitalDrivers\r\nSceneCollection=Digital Drivers\r\nSceneCollectionFile=DigitalDrivers\r\n".as_bytes()).unwrap();
        // a second prepare before the restore keeps the driver's own
        remember_before(&config, &paths).unwrap();
        let before: Before = read_json(&config.join(RESTORE_FILE)).unwrap();
        assert_eq!(before.choice.profile.as_deref(), Some("0xJoe"));
        restore_files(&paths, &before).unwrap();
        let user = fs::read_to_string(&paths.user_ini).unwrap();
        assert_eq!(config::user_choice(&user), before.choice);
        let websocket: Value = serde_json::from_str(&fs::read_to_string(&paths.websocket).unwrap()).unwrap();
        assert_eq!(websocket, json!({ "auth_required": true, "first_load": false, "server_enabled": false, "server_password": "joes", "server_port": 4455 }));
        // only recordings in the folder open
        write_file(&folder.join("DD 2026-11-09 20-58-01.mp4"), b"video").unwrap();
        assert!(inside(&folder.join("DD 2026-11-09 20-58-01.mp4"), &folder));
        assert!(!inside(&paths.user_ini, &folder));
        assert!(!inside(&folder.join("..").join("..").join("obs-studio").join("user.ini"), &folder));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writes_the_mark_once_and_makes_fresh_passwords() {
        let dir = temp_dir("mark");
        let path = mark_file(&dir, Canvas { width: 1920, height: 1080 }).unwrap();
        assert!(path.ends_with("dd-mark-173.png"));
        assert_eq!(&fs::read(&path).unwrap()[1..4], b"PNG");
        assert!(mark_file(&dir, Canvas { width: 2560, height: 1440 }).unwrap().ends_with("dd-mark-230.png"));
        let (a, b) = (new_password(), new_password());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert_eq!(major("33.0.1"), Some(33));
        fs::remove_dir_all(dir).unwrap();
    }
}
