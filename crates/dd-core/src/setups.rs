//! Car setups for Assetto Corsa EVO. The game keeps them as files below `Saved Games\ACE\Car Setups`, in one
//! folder per car and below it one per track, and lists what it finds there in its setup screen. The app puts
//! a setup of the platform there and says which files are there already; loading one stays with the driver.
//!
//! As with scrutineering the app judges nothing: it reports the SHA-256 of the files the platform asks about,
//! and the platform knows whether that is the setup it hands out.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

fn sha256_of(path: &Path) -> std::io::Result<String> {
    Ok(Sha256::digest(fs::read(path)?).iter().map(|b| format!("{b:02x}")).collect())
}


/// The game's folder below the user's `Saved Games`.
pub const USER_DIR: &str = "ACE";
/// The folder of the car setups below the game's folder.
pub const SETUPS_DIR: &str = "Car Setups";

const EXTENSION: &str = ".carsetup";
/// A setup of the game has a few hundred bytes.
const MAX_SIZE: usize = 64 * 1024;

/// Why a setup was not installed. The code goes to the hosted interface, which has the words for it.
#[derive(Debug, PartialEq, Eq)]
pub enum SetupError {
    /// A value is not what it should be; names the field.
    InvalidSetup(&'static str),
    /// The game has no folder on this PC: it is not installed, or was never started.
    AcEvoNotFound,
    /// A file of that name is there with other content: the driver changed the setup and saved it.
    Changed,
    /// The file could not be written.
    Failed(String),
}

impl SetupError {
    pub fn code(&self) -> String {
        match self {
            SetupError::InvalidSetup(field) => format!("invalid-setup:{field}"),
            SetupError::AcEvoNotFound => "ac-evo-not-found".to_string(),
            SetupError::Changed => "setup-changed".to_string(),
            SetupError::Failed(_) => "failed".to_string(),
        }
    }
}

/// Where a setup belongs, as the hosted interface names it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupFile {
    /// The car's folder below `Car Setups`: the name the game shows for the car
    pub car_folder: String,
    pub track_folder: String,
    /// The name of the file, ending in `.carsetup`
    pub name: String,
}

/// The name of one folder or file as the game makes them ("Porsche 911 GT3 Cup (992)"), and nothing that could
/// leave the folder it is put in.
fn is_plain_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 100
        && value.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '(' | ')' | '-' | '_' | '.'))
        && !value.starts_with([' ', '.'])
        && !value.ends_with([' ', '.'])
}

impl SetupFile {
    pub fn validate(&self) -> Result<(), SetupError> {
        if !is_plain_name(&self.car_folder) {
            return Err(SetupError::InvalidSetup("carFolder"));
        }
        if !is_plain_name(&self.track_folder) {
            return Err(SetupError::InvalidSetup("trackFolder"));
        }
        if !is_plain_name(&self.name) || self.name.len() <= EXTENSION.len() || !self.name.ends_with(EXTENSION) {
            return Err(SetupError::InvalidSetup("name"));
        }
        Ok(())
    }

    fn path_below(&self, setups_dir: &Path) -> PathBuf {
        setups_dir.join(&self.car_folder).join(&self.track_folder).join(&self.name)
    }
}

/// The user's `Saved Games` folder. Windows notes where it is only for a user who moved it (`moved_to`, the
/// value of the folder's id in the registry's `User Shell Folders`); for everybody else it is in the home folder.
pub fn saved_games_dir(moved_to: Option<&str>, home: &Path) -> PathBuf {
    const HOME_VARIABLE: &str = "%USERPROFILE%";
    match moved_to.map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => match path.get(..HOME_VARIABLE.len()) {
            Some(start) if start.eq_ignore_ascii_case(HOME_VARIABLE) => {
                home.join(path[HOME_VARIABLE.len()..].trim_start_matches(['\\', '/']))
            }
            _ => PathBuf::from(path),
        },
        None => home.join("Saved Games"),
    }
}

/// The SHA-256 of the asked setups below the game's setup folder, in the order asked; None: not there.
pub fn hash_setups(setups_dir: &Path, files: &[SetupFile]) -> Result<Vec<Option<String>>, SetupError> {
    files
        .iter()
        .map(|file| {
            file.validate()?;
            Ok(sha256_of(&file.path_below(setups_dir)).ok())
        })
        .collect()
}

/// Puts a setup into the game's setup folder. A file of that name with other content is the driver's own
/// work and stays, unless the driver asked for the original again (`replace`).
pub fn install(setups_dir: &Path, file: &SetupFile, data: &[u8], replace: bool) -> Result<(), SetupError> {
    file.validate()?;
    if data.is_empty() || data.len() > MAX_SIZE {
        return Err(SetupError::InvalidSetup("data"));
    }
    let path = file.path_below(setups_dir);
    let failed = |what: &str, error: std::io::Error| SetupError::Failed(format!("{what}: {error}"));
    match fs::read(&path) {
        Ok(there) if there == data => return Ok(()),
        Ok(_) if !replace => return Err(SetupError::Changed),
        _ => {}
    }
    fs::create_dir_all(path.parent().expect("a setup is in the folder of its track")).map_err(|e| failed("setup folder", e))?;
    fs::write(&path, data).map_err(|e| failed("setup file", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's setup folder, made new for every test.
    fn setups_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dd-core-setups-{name}-{}", std::process::id())).join(USER_DIR).join(SETUPS_DIR);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hotlap() -> SetupFile {
        SetupFile {
            car_folder: "Porsche 911 GT3 Cup (992)".to_string(),
            track_folder: "Nurburgring".to_string(),
            name: "DD Nordschleife Hotlap v1.carsetup".to_string(),
        }
    }

    // SHA-256 of "abc", the test vector of the standard.
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn reads_a_setup_the_way_the_page_names_it() {
        let sent = r#"{"carFolder":"Mercedes-AMG GT2","trackFolder":"Nurburgring","name":"DD Nordschleife Stint v2.carsetup"}"#;
        let file: SetupFile = serde_json::from_str(sent).unwrap();
        assert_eq!(file.validate(), Ok(()));
        assert_eq!((file.car_folder.as_str(), file.name.as_str()), ("Mercedes-AMG GT2", "DD Nordschleife Stint v2.carsetup"));
        assert_eq!(SetupFile { car_folder: "Lamborghini Huracán Super Trofeo EVO2".to_string(), ..hotlap() }.validate(), Ok(()));
    }

    #[test]
    fn refuses_names_that_could_leave_the_setup_folder_or_are_no_setup() {
        let cases = [
            (SetupFile { car_folder: "..".to_string(), ..hotlap() }, "carFolder"),
            (SetupFile { car_folder: "Porsche\\..\\..".to_string(), ..hotlap() }, "carFolder"),
            (SetupFile { car_folder: "C:".to_string(), ..hotlap() }, "carFolder"),
            (SetupFile { car_folder: String::new(), ..hotlap() }, "carFolder"),
            (SetupFile { car_folder: "Porsche ".to_string(), ..hotlap() }, "carFolder"),
            (SetupFile { track_folder: "../Nurburgring".to_string(), ..hotlap() }, "trackFolder"),
            (SetupFile { track_folder: "Nurburgring\r\n".to_string(), ..hotlap() }, "trackFolder"),
            (SetupFile { track_folder: "x".repeat(101), ..hotlap() }, "trackFolder"),
            (SetupFile { name: "start.bat".to_string(), ..hotlap() }, "name"),
            (SetupFile { name: ".carsetup".to_string(), ..hotlap() }, "name"),
            (SetupFile { name: "..\\DD.carsetup".to_string(), ..hotlap() }, "name"),
            (SetupFile { name: "DD.carsetup:stream".to_string(), ..hotlap() }, "name"),
            (SetupFile { name: "DD.carsetup.".to_string(), ..hotlap() }, "name"),
        ];
        for (file, field) in cases {
            assert_eq!(file.validate(), Err(SetupError::InvalidSetup(field)), "{file:?}");
        }
        let dir = setups_dir("refuses");
        let outside = SetupFile { car_folder: "..".to_string(), ..hotlap() };
        assert_eq!(install(&dir, &outside, b"abc", true), Err(SetupError::InvalidSetup("carFolder")));
        assert_eq!(hash_setups(&dir, &[hotlap(), outside]), Err(SetupError::InvalidSetup("carFolder")));
        assert_eq!(install(&dir, &hotlap(), b"", false), Err(SetupError::InvalidSetup("data")));
        assert_eq!(install(&dir, &hotlap(), &vec![0u8; MAX_SIZE + 1], false), Err(SetupError::InvalidSetup("data")));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installs_a_setup_where_the_game_looks_for_it_and_says_what_is_there() {
        let dir = setups_dir("installs");
        let stint = SetupFile { name: "DD Nordschleife Stint v1.carsetup".to_string(), ..hotlap() };
        assert_eq!(hash_setups(&dir, &[hotlap(), stint.clone()]), Ok(vec![None, None]));

        // The car has no folder yet: the game makes it with the first setup saved, and so does the app.
        assert_eq!(install(&dir, &hotlap(), b"abc", false), Ok(()));
        let file = dir.join("Porsche 911 GT3 Cup (992)").join("Nurburgring").join("DD Nordschleife Hotlap v1.carsetup");
        assert_eq!(fs::read(&file).unwrap(), b"abc");
        assert_eq!(hash_setups(&dir, &[hotlap(), stint]), Ok(vec![Some(ABC.to_string()), None]));
        // Once more is no mistake.
        assert_eq!(install(&dir, &hotlap(), b"abc", false), Ok(()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_a_setup_the_driver_changed_unless_asked_for_the_original() {
        let dir = setups_dir("changed");
        install(&dir, &hotlap(), b"abc", false).unwrap();
        let file = hotlap().path_below(&dir);
        fs::write(&file, b"abc with less wing").unwrap();

        let hashes = hash_setups(&dir, &[hotlap()]).unwrap();
        assert!(hashes[0].is_some() && hashes[0].as_deref() != Some(ABC));
        assert_eq!(install(&dir, &hotlap(), b"abc", false), Err(SetupError::Changed));
        assert_eq!(fs::read(&file).unwrap(), b"abc with less wing");

        assert_eq!(install(&dir, &hotlap(), b"abc", true), Ok(()));
        assert_eq!(hash_setups(&dir, &[hotlap()]), Ok(vec![Some(ABC.to_string())]));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn finds_the_saved_games_folder_also_when_it_was_moved() {
        let home = Path::new("C:\\Users\\anna");
        assert_eq!(saved_games_dir(None, home), home.join("Saved Games"));
        assert_eq!(saved_games_dir(Some(""), home), home.join("Saved Games"));
        assert_eq!(saved_games_dir(Some("%USERPROFILE%\\Games\\Saved"), home), home.join("Games\\Saved"));
        assert_eq!(saved_games_dir(Some("%UserProfile%\\Saved Games"), home), home.join("Saved Games"));
        assert_eq!(saved_games_dir(Some("E:\\Anna\\Saved Games"), home), PathBuf::from("E:\\Anna\\Saved Games"));
    }

    #[test]
    fn names_its_errors_by_code() {
        assert_eq!(SetupError::InvalidSetup("name").code(), "invalid-setup:name");
        assert_eq!(SetupError::AcEvoNotFound.code(), "ac-evo-not-found");
        assert_eq!(SetupError::Changed.code(), "setup-changed");
        assert_eq!(SetupError::Failed("setup file: access denied".to_string()).code(), "failed");
    }
}
