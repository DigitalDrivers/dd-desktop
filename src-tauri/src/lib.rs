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
        .invoke_handler(tauri::generate_handler![platform_url, system_check])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
