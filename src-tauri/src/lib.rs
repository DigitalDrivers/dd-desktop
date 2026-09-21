//! The Digital Drivers desktop shell. It loads the hosted interface and offers a small, fixed set of
//! native commands to it. Logic that does not need Tauri or Windows APIs lives in `dd-core`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use std::sync::Mutex;
use std::time::SystemTime;

use dd_core::bot_race::{self, BotRaceResult, BotRaceTicket};
use dd_core::join::{self, JoinError, JoinTicket};
use dd_core::links;
use dd_core::scrutineering::{self, FileHash};
use serde::Serialize;
use tauri::Manager;

/// Address of the hosted interface. `DD_PLATFORM_URL` overrides it for development.
const DEFAULT_PLATFORM_URL: &str = "https://digitaldrivers.club";

/// Appends a line to `dd-desktop.log` in the system temp folder. Drivers can send this file to support.
fn log(message: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("dd-desktop.log");
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

#[tauri::command]
fn platform_url() -> String {
    let url = std::env::var("DD_PLATFORM_URL").unwrap_or_else(|_| DEFAULT_PLATFORM_URL.to_string());
    log(&format!("platform_url -> {url}"));
    url
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemCheck {
    app_version: String,
    /// Folder of the Assetto Corsa installation, if one was found.
    assetto_corsa_path: Option<String>,
    /// Whether the app may write into that folder (needed to install content). None without a folder.
    assetto_corsa_writable: Option<bool>,
    /// SteamID64 of the account signed in to Steam on this PC. None while Steam is not running.
    steam_id: Option<String>,
    /// This app can start races against bots and report how they went (since 0.4.0).
    bot_races: bool,
    /// The program Content Manager registered for its `acmanager://` links, if it did and the file is still
    /// there (since 0.5.0). The platform links straight to it, or says what is missing.
    content_manager_path: Option<String>,
    /// Build of the Custom Shaders Patch in the game folder, if it is installed (since 0.5.0).
    csp_build: Option<u32>,
}

#[tauri::command]
fn system_check(webview: tauri::Webview, app: tauri::AppHandle) -> SystemCheck {
    log(&format!("system_check requested by {}", webview.url().map(|u| u.to_string()).unwrap_or_default()));

    let ac = find_assetto_corsa();
    let writable = ac.as_deref().map(can_write_into);
    let steam_id = join::steam_id64(active_steam_user());
    let content_manager = find_content_manager();
    let csp_build = ac.as_deref().and_then(scrutineering::csp_build);
    log(&format!(
        "system_check -> assetto corsa: {ac:?}, writable: {writable:?}, steam account: {steam_id:?}, content manager: {content_manager:?}, CSP build {csp_build:?}"
    ));
    SystemCheck {
        app_version: app.package_info().version.to_string(),
        assetto_corsa_writable: writable,
        assetto_corsa_path: ac.map(|p| p.display().to_string()),
        steam_id,
        bot_races: true,
        content_manager_path: content_manager.map(|p| p.display().to_string()),
        csp_build,
    }
}

/// The race against bots the app has started, until its result has been fetched.
struct BotRaceRun {
    game: std::process::Child,
    started: SystemTime,
    result_file: PathBuf,
}

#[derive(Default)]
struct BotRaceState(Mutex<Option<BotRaceRun>>);

fn game_cfg_dir(app: &tauri::AppHandle) -> Result<PathBuf, JoinError> {
    Ok(app.path().document_dir().map_err(|e| JoinError::Failed(format!("documents folder: {e}")))?.join("Assetto Corsa"))
}

/// Starts a single-player race against the game's own AI: the driver's car against bots in the same car, on
/// the track and over the laps of the ticket. Like `join_race` it writes the game's `race.ini` and starts
/// `acs.exe`; how the race went is fetched with `bot_race_result` once the game has closed.
#[tauri::command]
async fn start_bot_race(webview: tauri::Webview, app: tauri::AppHandle, state: tauri::State<'_, BotRaceState>, ticket: BotRaceTicket) -> Result<(), String> {
    log(&format!(
        "start_bot_race requested by {}: {} on {} against {} bots at {} %, {} laps",
        webview.url().map(|u| u.to_string()).unwrap_or_default(), ticket.car_model, ticket.track, ticket.opponents, ticket.ai_level, ticket.laps
    ));
    let run = launch_bot_race(&app, &ticket).map_err(|error| {
        log(&format!("start_bot_race failed: {error:?}"));
        error.code()
    })?;
    *state.0.lock().unwrap() = Some(run);
    Ok(())
}

fn launch_bot_race(app: &tauri::AppHandle, ticket: &BotRaceTicket) -> Result<BotRaceRun, JoinError> {
    ticket.validate()?;
    let ac = find_assetto_corsa().ok_or(JoinError::AssettoCorsaNotFound)?;
    // Steam has to run for the game to start at all.
    join::steam_id64(active_steam_user()).ok_or(JoinError::SteamNotRunning)?;
    for (kind, name) in [("cars", &ticket.car_model), ("tracks", &ticket.track)] {
        if !ac.join("content").join(kind).join(name).is_dir() {
            return Err(JoinError::ContentMissing(format!("{kind}/{name}")));
        }
    }
    // The liveries the car has on this PC, by name: the driver gets the first, the bots the others in turn.
    let mut skins: Vec<String> = fs::read_dir(ac.join("content").join("cars").join(&ticket.car_model).join("skins"))
        .map(|dir| dir.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    skins.sort();

    let failed = |what: &str, error: std::io::Error| JoinError::Failed(format!("{what}: {error}"));
    let app_id = ac.join("steam_appid.txt");
    if !app_id.is_file() {
        fs::write(&app_id, join::STEAM_APP_ID).map_err(|e| failed("steam_appid.txt", e))?;
    }
    let documents = game_cfg_dir(app)?;
    fs::create_dir_all(documents.join("cfg")).map_err(|e| failed("cfg folder", e))?;
    let race_ini = documents.join("cfg").join("race.ini");
    let previous = fs::read(&race_ini).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
    fs::write(&race_ini, bot_race::render_bot_race_ini(ticket, &skins, &previous)).map_err(|e| failed("race.ini", e))?;

    let started = SystemTime::now();
    let game = std::process::Command::new(ac.join("acs.exe")).current_dir(&ac).spawn().map_err(|e| failed("acs.exe", e))?;
    Ok(BotRaceRun { game, started, result_file: documents.join("out").join("race_out.json") })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BotRaceStatus {
    /// "none": no race was started; "running": the game is still open; "finished": the game has closed
    state: &'static str,
    /// With "finished": how the race went; None when the game left no race result (closed before the start)
    result: Option<BotRaceResult>,
}

/// How the race against bots started with `start_bot_race` stands. The result is handed out once.
#[tauri::command]
fn bot_race_result(state: tauri::State<'_, BotRaceState>) -> BotRaceStatus {
    let mut guard = state.0.lock().unwrap();
    let Some(run) = guard.as_mut() else { return BotRaceStatus { state: "none", result: None } };
    if matches!(run.game.try_wait(), Ok(None)) {
        return BotRaceStatus { state: "running", result: None };
    }
    // The game writes its result when it closes; only a file written after the start is this race's.
    let fresh = fs::metadata(&run.result_file).and_then(|m| m.modified()).map(|at| at >= run.started).unwrap_or(false);
    let result = if fresh { fs::read_to_string(&run.result_file).ok().and_then(|json| bot_race::parse_race_out(&json)) } else { None };
    log(&format!("bot_race_result -> game closed, result: {result:?}"));
    *guard = None;
    BotRaceStatus { state: "finished", result }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JoinStarted {
    /// The livery the race server has for the driver
    skin: String,
}

/// Starts the game on the race server of the ticket, the way a launcher does: asks the server for the
/// driver's slot, writes the game's `race.ini` for an online session there and starts `acs.exe`.
/// An error is a short code (see `JoinError::code`); the hosted interface has the words for it.
#[tauri::command]
async fn join_race(webview: tauri::Webview, app: tauri::AppHandle, ticket: JoinTicket) -> Result<JoinStarted, String> {
    log(&format!(
        "join_race requested by {}: {} on {}:{} ({})",
        webview.url().map(|u| u.to_string()).unwrap_or_default(),
        ticket.car_model,
        ticket.host,
        ticket.game_port,
        ticket.steam_id
    ));
    match start_race(&app, &ticket) {
        Ok(started) => {
            log(&format!("join_race -> game started, skin {}", started.skin));
            Ok(started)
        }
        Err(error) => {
            log(&format!("join_race failed: {error:?}"));
            Err(error.code())
        }
    }
}

fn start_race(app: &tauri::AppHandle, ticket: &JoinTicket) -> Result<JoinStarted, JoinError> {
    ticket.validate()?;
    let ac = find_assetto_corsa().ok_or(JoinError::AssettoCorsaNotFound)?;
    // The race server checks the game's Steam ticket against the slot: it has to be the same account.
    let local_account = join::steam_id64(active_steam_user()).ok_or(JoinError::SteamNotRunning)?;
    if local_account != ticket.steam_id {
        return Err(JoinError::WrongSteamAccount);
    }
    for (kind, name) in [("cars", &ticket.car_model), ("tracks", &ticket.track)] {
        if !ac.join("content").join(kind).join(name).is_dir() {
            return Err(JoinError::ContentMissing(format!("{kind}/{name}")));
        }
    }

    let entry_list = join::fetch_entry_list(&ticket.host, ticket.http_port, &ticket.steam_id, Duration::from_secs(5))?;
    let slot = join::slot_of(&entry_list, &ticket.car_model)?;

    let failed = |what: &str, error: std::io::Error| JoinError::Failed(format!("{what}: {error}"));
    // Without this file Steam starts the game's own launcher instead of the session.
    let app_id = ac.join("steam_appid.txt");
    if !app_id.is_file() {
        fs::write(&app_id, join::STEAM_APP_ID).map_err(|e| failed("steam_appid.txt", e))?;
    }
    let cfg = app.path().document_dir().map_err(|e| JoinError::Failed(format!("documents folder: {e}")))?.join("Assetto Corsa").join("cfg");
    fs::create_dir_all(&cfg).map_err(|e| failed("cfg folder", e))?;
    let race_ini = cfg.join("race.ini");
    let previous = fs::read(&race_ini).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
    fs::write(&race_ini, join::render_race_ini(ticket, &slot, &previous)).map_err(|e| failed("race.ini", e))?;

    std::process::Command::new(ac.join("acs.exe")).current_dir(&ac).spawn().map_err(|e| failed("acs.exe", e))?;
    Ok(JoinStarted { skin: slot.skin })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScrutineeringReport {
    files: Vec<FileHash>,
    /// Build of the installed Custom Shaders Patch; None when it is not installed or switched off.
    csp_build: Option<u32>,
    app_version: String,
}

/// Technical scrutineering: reports the SHA-256 of the game files the hosted interface asks about (only
/// below the game's `content` and `system` folders) and the build of the Custom Shaders Patch. The platform
/// compares that with what the race server will check; the app itself judges nothing.
#[tauri::command]
async fn scrutineer(webview: tauri::Webview, app: tauri::AppHandle, files: Vec<String>) -> Result<ScrutineeringReport, String> {
    log(&format!("scrutineer requested by {}: {} files", webview.url().map(|u| u.to_string()).unwrap_or_default(), files.len()));
    if files.len() > 200 {
        return Err("too-many-files".to_string());
    }
    let ac = find_assetto_corsa().ok_or_else(|| JoinError::AssettoCorsaNotFound.code())?;
    let hashes = scrutineering::hash_files(&ac, &files).map_err(|path| {
        log(&format!("scrutineer refused the path {path}"));
        "invalid-path".to_string()
    })?;
    let csp_build = scrutineering::csp_build(&ac);
    log(&format!("scrutineer -> {} of {} files found, CSP build {csp_build:?}", hashes.iter().filter(|f| f.sha256.is_some()).count(), hashes.len()));
    Ok(ScrutineeringReport { files: hashes, csp_build, app_version: app.package_info().version.to_string() })
}

/// Shows a native notification (a Windows toast), also while the window is minimised. The hosted
/// interface calls it for new notifications of the signed-in driver. Plain text only, cut to a sane length.
#[tauri::command]
fn show_toast(webview: tauri::Webview, app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    let title: String = title.chars().take(80).collect();
    let body: String = body.chars().take(300).collect();
    let app_id = toast_app_id(&app.config().identifier);
    log(&format!(
        "show_toast requested by {}: {title} | {body} (app id {app_id})",
        webview.url().map(|u| u.to_string()).unwrap_or_default()
    ));
    show_native_toast(&app_id, &title, &body).map_err(|error| {
        log(&format!("show_toast failed: {error}"));
        error
    })
}

/// The id Windows files our notifications under. It has to be one Windows knows, or the toast is
/// dropped without an error.
fn toast_app_id(identifier: &str) -> String {
    // Installed from the Store or as MSIX, the package decides the id.
    if let Some(id) = package_app_id() {
        return id;
    }
    // A build started straight from the target folder is not registered with Windows at all;
    // PowerShell's id makes the toast show anyway. An installed build is registered by its identifier.
    let from_target_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(is_cargo_target_dir)).unwrap_or(false);
    if from_target_dir { POWERSHELL_APP_ID.to_string() } else { identifier.to_string() }
}

const POWERSHELL_APP_ID: &str = r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

fn is_cargo_target_dir(dir: &Path) -> bool {
    dir.ends_with(Path::new("target").join("debug")) || dir.ends_with(Path::new("target").join("release"))
}

/// The application user model id of the package this process runs in, if it runs in one.
#[cfg(windows)]
fn package_app_id() -> Option<String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentApplicationUserModelId(length: *mut u32, id: *mut u16) -> i32;
    }
    let mut buffer = [0u16; 256];
    let mut length = buffer.len() as u32;
    // SAFETY: `buffer` outlives the call and `length` holds its size in characters, as the API asks.
    let status = unsafe { GetCurrentApplicationUserModelId(&mut length, buffer.as_mut_ptr()) };
    // 0 is success, the length then counts the terminating zero. Without a package the call
    // answers APPMODEL_ERROR_NO_APPLICATION (15703).
    (status == 0 && length > 1).then(|| String::from_utf16_lossy(&buffer[..length as usize - 1]))
}

#[cfg(not(windows))]
fn package_app_id() -> Option<String> {
    None
}

#[cfg(windows)]
fn show_native_toast(app_id: &str, title: &str, body: &str) -> Result<(), String> {
    tauri_winrt_notification::Toast::new(app_id).title(title).text1(body).show().map_err(|error| error.to_string())
}

#[cfg(not(windows))]
fn show_native_toast(_app_id: &str, _title: &str, _body: &str) -> Result<(), String> {
    Err("Notifications are only implemented on Windows".to_string())
}

/// Writes and removes a small probe file to learn whether the folder is writable.
fn can_write_into(dir: &Path) -> bool {
    let probe = dir.join("dd-desktop-write-probe.tmp");
    let written = fs::write(&probe, b"Digital Drivers write probe").is_ok();
    let _ = fs::remove_file(&probe);
    written
}

fn find_assetto_corsa() -> Option<PathBuf> {
    // Development builds only: a game folder made up for a check (scripts/run-scrutineering-check.ps1).
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("DD_ASSETTO_CORSA_DIR").map(PathBuf::from) {
        return dd_core::steam::is_assetto_corsa_dir(&dir).then_some(dir);
    }
    let steam = steam_root()?;
    let vdf = fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")).ok()?;
    dd_core::steam::find_assetto_corsa(&dd_core::steam::library_paths(&vdf))
}

#[cfg(windows)]
fn steam_root() -> Option<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam").ok()?;
    let path: String = key.get_value("SteamPath").ok()?;
    Some(PathBuf::from(path))
}

#[cfg(not(windows))]
fn steam_root() -> Option<PathBuf> {
    None
}

/// The program registered for `acmanager://` links, as long as it is still there. Content Manager registers
/// itself for the current user when it first starts.
#[cfg(windows)]
fn find_content_manager() -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|root| {
        let command: String = RegKey::predef(root).open_subkey(r"Software\Classes\acmanager\shell\open\command").ok()?.get_value("").ok()?;
        links::protocol_handler_exe(&command).filter(|exe| exe.is_file())
    })
}

#[cfg(not(windows))]
fn find_content_manager() -> Option<PathBuf> {
    None
}

/// A page the interface opens in a new window goes to the driver's browser; anything else is refused.
fn open_in_browser(url: &str) {
    if !links::opens_in_browser(url) {
        log(&format!("new window refused: {url}"));
        return;
    }
    log(&format!("opening in the browser: {url}"));
    // What the shell does with a link: hand it to the default browser.
    #[cfg(windows)]
    if let Err(error) = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn() {
        log(&format!("opening in the browser failed: {error}"));
    }
}

/// Account id of the user signed in to the running Steam; 0 while Steam is not running.
#[cfg(windows)]
fn active_steam_user() -> u32 {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Valve\Steam\ActiveProcess")
        .and_then(|key| key.get_value::<u32, _>("ActiveUser"))
        .unwrap_or(0)
}

#[cfg(not(windows))]
fn active_steam_user() -> u32 {
    0
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(BotRaceState::default())
        .setup(|app| {
            // The window is built here instead of by the configuration alone, so that links which open a new
            // window (a stream, a download) go to the browser: the app has no tabs, and without this they did
            // nothing at all.
            let config = app.config().app.windows.first().cloned().ok_or("no window in tauri.conf.json")?;
            tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?
                .on_new_window(|url, _features| {
                    open_in_browser(url.as_str());
                    tauri::webview::NewWindowResponse::Deny
                })
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![platform_url, system_check, show_toast, join_race, scrutineer, start_bot_race, bot_race_result])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_a_build_started_from_the_cargo_target_folder() {
        assert!(is_cargo_target_dir(&Path::new("projects").join("dd-desktop").join("target").join("debug")));
        assert!(is_cargo_target_dir(&Path::new("dd-desktop").join("target").join("release")));
        assert!(!is_cargo_target_dir(&Path::new("Program Files").join("Digital Drivers")));
        assert!(!is_cargo_target_dir(&Path::new("WindowsApps").join("DigitalDrivers.Desktop_0.2.0.0_x64__5sms3s9pdbqt0")));
    }
}
