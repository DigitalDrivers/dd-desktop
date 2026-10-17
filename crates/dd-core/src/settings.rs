//! Settings of the app on this PC that the bundled start page needs before the hosted interface loads, or the shell
//! needs while no page is open: the interface keeps its own settings in its storage, which neither can read.

use std::fs;
use std::io;
use std::path::Path;

/// Marker in the app's config folder: the driver installs the app's updates from the update center only.
const UPDATE_AT_START_OFF: &str = "update-at-start.off";
/// Marker in the app's config folder: the driver switched on the race engineer on this PC.
const ENGINEER_ON: &str = "engineer.on";
/// File in the app's config folder: how many days the race engineer keeps its sessions, 0 for ever.
const ENGINEER_KEEP: &str = "engineer.keep";
/// How long sessions stay until the driver chooses.
pub const ENGINEER_KEEP_DAYS: u32 = 30;

/// Whether the start page installs a newer release before the interface loads; on unless the driver turned
/// it off.
pub fn update_at_start(dir: &Path) -> bool {
    !dir.join(UPDATE_AT_START_OFF).exists()
}

pub fn set_update_at_start(dir: &Path, on: bool) -> io::Result<()> {
    let marker = dir.join(UPDATE_AT_START_OFF);
    if on {
        return match fs::remove_file(marker) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    fs::create_dir_all(dir)?;
    fs::write(marker, b"")
}

/// Whether the race engineer evaluates the driver's laps on this PC ("Evaluate laps on this PC"); off until the
/// driver turns it on. While it is on, the app records its lap list whenever the game runs.
pub fn engineer(dir: &Path) -> bool {
    dir.join(ENGINEER_ON).exists()
}

pub fn set_engineer(dir: &Path, on: bool) -> io::Result<()> {
    let marker = dir.join(ENGINEER_ON);
    if !on {
        return match fs::remove_file(marker) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    fs::create_dir_all(dir)?;
    fs::write(marker, b"")
}

/// How many days the race engineer keeps its sessions ("Keep sessions"): 30 until the driver chooses, None for
/// ever.
pub fn engineer_keep_days(dir: &Path) -> Option<u32> {
    match fs::read_to_string(dir.join(ENGINEER_KEEP)).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(0) => None,
        Some(days) => Some(days),
        None => Some(ENGINEER_KEEP_DAYS),
    }
}

pub fn set_engineer_keep_days(dir: &Path, days: Option<u32>) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join(ENGINEER_KEEP), days.unwrap_or(0).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_sessions_30_days_until_the_driver_chooses() {
        let dir = std::env::temp_dir().join(format!("dd-settings-keep-{}", std::process::id())).join("config");
        assert_eq!(engineer_keep_days(&dir), Some(30));
        set_engineer_keep_days(&dir, Some(90)).unwrap();
        assert_eq!(engineer_keep_days(&dir), Some(90));
        set_engineer_keep_days(&dir, None).unwrap();
        assert_eq!(engineer_keep_days(&dir), None);
        set_engineer_keep_days(&dir, Some(0)).unwrap();
        assert_eq!(engineer_keep_days(&dir), None, "0 is for ever too");
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn installs_at_start_until_the_driver_turns_it_off() {
        let dir = std::env::temp_dir().join(format!("dd-settings-{}", std::process::id())).join("config");
        assert!(update_at_start(&dir));
        set_update_at_start(&dir, true).unwrap();
        assert!(update_at_start(&dir));
        // The config folder does not exist before the first setting.
        set_update_at_start(&dir, false).unwrap();
        assert!(!update_at_start(&dir));
        set_update_at_start(&dir, false).unwrap();
        assert!(!update_at_start(&dir));
        set_update_at_start(&dir, true).unwrap();
        assert!(update_at_start(&dir));
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_engineer_is_off_until_the_driver_turns_it_on() {
        let dir = std::env::temp_dir().join(format!("dd-settings-engineer-{}", std::process::id())).join("config");
        assert!(!engineer(&dir));
        set_engineer(&dir, false).unwrap();
        assert!(!engineer(&dir));
        set_engineer(&dir, true).unwrap();
        assert!(engineer(&dir));
        set_engineer(&dir, true).unwrap();
        set_engineer(&dir, false).unwrap();
        assert!(!engineer(&dir));
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
