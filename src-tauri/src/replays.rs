//! The replay page's native side (since 0.29.0): AC EVO's replays in `Saved Games\ACE\Replay` (`Saved`, `Temp`),
//! listed, kept and deleted through `dd_core::replays`, and the game check, which judges the video settings of the
//! game's newest log through `dd_core::game_check`. Keeping and deleting wait for the game to be closed: it
//! rotates its Temp folder. Opening the folder starts Explorer on that one folder, never on a path from the page.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use dd_core::game_check::{self, Check, GameVideo};
use dd_core::replays::{self, Folder, Listing};
use serde::Serialize;

use crate::{ac_evo_running, find_ac_evo_user_dir, log};

/// What `replay_list` answers.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayList {
    #[serde(flatten)]
    pub listing: Listing,
    /// The Replay folder on this PC, for the page's line under the list
    pub folder: String,
}

/// What `game_check` answers.
#[derive(Debug, Serialize)]
pub struct GameCheck {
    pub checks: Vec<Check>,
    pub video: GameVideo,
}

fn replay_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    find_ac_evo_user_dir(app).map(|dir| dir.join(replays::REPLAY_DIR)).ok_or_else(|| "ac-evo-not-found".to_string())
}

fn list_in(dir: &Path) -> ReplayList {
    ReplayList { listing: replays::list(dir), folder: dir.display().to_string() }
}

pub fn list(app: &tauri::AppHandle) -> Result<ReplayList, String> {
    let list = list_in(&replay_dir(app)?);
    log(&format!("replay_list -> {} saved ({} bytes), {} temp ({} bytes)", list.listing.saved.len(), list.listing.saved_bytes, list.listing.temp.len(), list.listing.temp_bytes));
    Ok(list)
}

pub fn keep(app: &tauri::AppHandle, name: &str) -> Result<String, String> {
    let dir = replay_dir(app)?;
    if ac_evo_running() {
        return Err("game-running".to_string());
    }
    let kept = replays::keep(&dir, name).map_err(|error| {
        log(&format!("replay_keep failed: {error:?}"));
        error.code()
    })?;
    log(&format!("replay_keep -> Temp\\{name} is Saved\\{kept}"));
    Ok(kept)
}

pub fn delete(app: &tauri::AppHandle, folder: Folder, name: &str) -> Result<(), String> {
    let dir = replay_dir(app)?;
    if ac_evo_running() {
        return Err("game-running".to_string());
    }
    replays::delete(&dir, folder, name).map_err(|error| {
        log(&format!("replay_delete failed: {error:?}"));
        error.code()
    })?;
    log(&format!("replay_delete -> {}\\{name} deleted", folder.dir_name()));
    Ok(())
}

pub fn open_folder(app: &tauri::AppHandle) -> Result<(), String> {
    let dir = replay_dir(app)?;
    if !dir.is_dir() {
        return Err("no-replay-folder".to_string());
    }
    open_in_explorer(&dir).map_err(|error| {
        log(&format!("replay_open_folder failed: {error}"));
        format!("failed: {error}")
    })
}

#[cfg(windows)]
fn open_in_explorer(dir: &Path) -> std::io::Result<()> {
    std::process::Command::new("explorer").arg(dir).spawn().map(|_| ())
}

#[cfg(not(windows))]
fn open_in_explorer(_dir: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("only on Windows"))
}

/// How many of the newest logs are looked at: a run that ended before the renderer started has no settings.
const LOGS_LOOKED_AT: usize = 5;

/// The video settings of the newest of the game's logs (`Logs\log-*.txt`) that has them.
fn video_from_logs(logs: &Path) -> Option<GameVideo> {
    let mut found: Vec<(SystemTime, PathBuf)> = fs::read_dir(logs)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().to_str().is_some_and(|name| name.starts_with("log-") && name.ends_with(".txt")))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    found.sort_by(|a, b| b.cmp(a));
    found.into_iter().take(LOGS_LOOKED_AT).find_map(|(_, path)| game_check::parse(&String::from_utf8_lossy(&fs::read(path).ok()?)))
}

/// The video settings the game logged last; the OBS helper takes the canvas and the graphics card from them.
pub fn video(app: &tauri::AppHandle) -> Result<GameVideo, String> {
    let logs = find_ac_evo_user_dir(app).ok_or_else(|| "ac-evo-not-found".to_string())?.join("Logs");
    video_from_logs(&logs).ok_or_else(|| "no-video-settings".to_string())
}

pub fn check(app: &tauri::AppHandle) -> Result<GameCheck, String> {
    let video = video(app)?;
    let checks = game_check::judge(&video);
    log(&format!(
        "game_check -> settings of {}: {}",
        video.logged_at,
        checks.iter().map(|c| format!("{} {:?} {}", c.id, c.level, c.value)).collect::<Vec<_>>().join(", ")
    ));
    Ok(GameCheck { checks, video })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = include_str!("../../crates/dd-core/tests/fixtures/evo-log-video-settings.txt");

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dd-desktop-replays-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(dir: &Path, name: &str, text: &str, modified: u64) {
        let path = dir.join(name);
        fs::write(&path, text).unwrap();
        fs::File::options().write(true).open(&path).unwrap().set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(modified)).unwrap();
    }

    #[test]
    fn takes_the_video_settings_of_the_newest_log_that_has_them() {
        let logs = temp_dir("logs");
        assert_eq!(video_from_logs(&logs), None);
        put(&logs, "log-261107-210511.txt", &LOG.replace("DLSS_Performance", "DLSS_Quality"), 1_000);
        put(&logs, "log-261108-154657.txt", LOG, 2_000);
        // the newest: a start that ended before the renderer
        put(&logs, "log-261109-120000.txt", "[2026-11-09 12:00:00.000] [others] [info] Build release 0.9.1, version 0.9.1+release.6, built\n", 3_000);
        // no log of the game
        put(&logs, "crash.txt", &LOG.replace("DLSS_Performance", "DLSS_DLAA"), 4_000);
        assert_eq!(video_from_logs(&logs).unwrap().upscaler_mode.as_deref(), Some("Performance"));
        fs::remove_file(logs.join("log-261108-154657.txt")).unwrap();
        assert_eq!(video_from_logs(&logs).unwrap().upscaler_mode.as_deref(), Some("Quality"));
        assert_eq!(video_from_logs(&logs.join("nowhere")), None);
        fs::remove_dir_all(logs).unwrap();
    }

    #[test]
    fn answers_the_list_with_the_folder_beside_both_lists() {
        let dir = temp_dir("list").join(replays::REPLAY_DIR);
        fs::create_dir_all(dir.join("Temp")).unwrap();
        fs::write(dir.join("Temp").join("ACEVO_261108-151152_T_ks_car_Imola.rpy"), b"short").unwrap();
        let json = serde_json::to_value(list_in(&dir)).unwrap();
        assert_eq!(json["folder"], dir.display().to_string());
        assert_eq!((json["savedBytes"].as_u64(), json["tempBytes"].as_u64()), (Some(0), Some(5)));
        assert_eq!(json["temp"][0]["damaged"], true);
        assert_eq!(json["saved"], serde_json::json!([]));
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn answers_the_check_with_the_checks_and_the_values() {
        let video = game_check::parse(LOG).unwrap();
        let json = serde_json::to_value(GameCheck { checks: game_check::judge(&video), video }).unwrap();
        assert_eq!(json["checks"][0], serde_json::json!({ "id": "window-mode", "level": "good", "value": "borderless" }));
        assert_eq!(json["video"]["loggedAt"], "2026-11-08 15:47:11");
    }
}
