//! Settings of the app on this PC that the bundled start page needs before the hosted interface loads: the
//! interface keeps its own settings in its storage, which the start page cannot read.

use std::fs;
use std::io;
use std::path::Path;

/// Marker in the app's config folder: the driver installs the app's updates from the update center only.
const UPDATE_AT_START_OFF: &str = "update-at-start.off";

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
