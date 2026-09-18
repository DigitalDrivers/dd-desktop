//! The Digital Drivers desktop shell. It loads the hosted interface and offers a small, fixed set of
//! native commands to it. Logic that does not need Tauri or Windows APIs lives in `dd-core`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

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
}

#[tauri::command]
fn system_check(webview: tauri::Webview, app: tauri::AppHandle) -> SystemCheck {
    log(&format!("system_check requested by {}", webview.url().map(|u| u.to_string()).unwrap_or_default()));

    let ac = find_assetto_corsa();
    let writable = ac.as_deref().map(can_write_into);
    log(&format!("system_check -> assetto corsa: {ac:?}, writable: {writable:?}"));
    SystemCheck {
        app_version: app.package_info().version.to_string(),
        assetto_corsa_writable: writable,
        assetto_corsa_path: ac.map(|p| p.display().to_string()),
    }
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![platform_url, system_check, show_toast])
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
