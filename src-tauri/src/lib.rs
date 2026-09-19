//! The Digital Drivers desktop shell. It loads the hosted interface and offers a small, fixed set of
//! native commands to it. Logic that does not need Tauri or Windows APIs lives in `dd-core`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dd_core::join::{self, JoinError, JoinTicket};
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
}

#[tauri::command]
fn system_check(webview: tauri::Webview, app: tauri::AppHandle) -> SystemCheck {
    log(&format!("system_check requested by {}", webview.url().map(|u| u.to_string()).unwrap_or_default()));

    let ac = find_assetto_corsa();
    let writable = ac.as_deref().map(can_write_into);
    let steam_id = join::steam_id64(active_steam_user());
    log(&format!("system_check -> assetto corsa: {ac:?}, writable: {writable:?}, steam account: {steam_id:?}"));
    SystemCheck {
        app_version: app.package_info().version.to_string(),
        assetto_corsa_writable: writable,
        assetto_corsa_path: ac.map(|p| p.display().to_string()),
        steam_id,
    }
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
        .invoke_handler(tauri::generate_handler![platform_url, system_check, show_toast, join_race, scrutineer])
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
