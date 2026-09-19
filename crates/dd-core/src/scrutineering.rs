//! Technical scrutineering: the app reports what is installed, the platform judges. For every file the
//! platform asks about, the app answers with its SHA-256, or that it is not there, and it reports the build
//! of the Custom Shaders Patch. Only files of the game's `content` and `system` folders can be asked about.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::join::{ini_value, is_content_name};

/// A file the platform asked about.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct FileHash {
    pub path: String,
    /// None: the file is not there
    pub sha256: Option<String>,
}

/// A path the app answers for: `content/...` or `system/...` inside the game folder, forward slashes, every
/// part a plain name. Nothing outside the game, and nothing of the game but its content.
pub fn is_checkable_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    (2..=8).contains(&parts.len()) && matches!(parts[0], "content" | "system") && parts.iter().all(|part| is_content_name(part))
}

fn sha256_of(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The hashes of the asked files below the game folder. Err names a path the app does not answer for.
pub fn hash_files(game_dir: &Path, paths: &[String]) -> Result<Vec<FileHash>, String> {
    paths
        .iter()
        .map(|path| {
            if !is_checkable_path(path) {
                return Err(path.clone());
            }
            let file = path.split('/').fold(game_dir.to_path_buf(), |dir, part| dir.join(part));
            Ok(FileHash { path: path.clone(), sha256: sha256_of(&file).ok() })
        })
        .collect()
}

/// Build number of the installed Custom Shaders Patch. None when the patch is not installed or switched
/// off: its loader `dwrite.dll` has to be in the game folder, the number is in its own manifest.
pub fn csp_build(game_dir: &Path) -> Option<u32> {
    if !game_dir.join("dwrite.dll").is_file() {
        return None;
    }
    let manifest = std::fs::read(game_dir.join("extension").join("config").join("data_manifest.ini")).ok()?;
    ini_value(&String::from_utf8_lossy(&manifest), "VERSION", "SHADERS_PATCH_BUILD")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A game folder with one car, made new for every test.
    fn game_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dd-core-scrutineering-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("content").join("cars").join("cup_car")).unwrap();
        fs::write(dir.join("content").join("cars").join("cup_car").join("data.acd"), b"abc").unwrap();
        dir
    }

    const DATA_ACD: &str = "content/cars/cup_car/data.acd";
    // SHA-256 of "abc", the test vector of the standard.
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn hashes_the_files_it_is_asked_about_and_says_which_are_not_there() {
        let dir = game_dir("hashes");
        let asked = [DATA_ACD.to_string(), "content/cars/cup_car/collider.kn5".to_string()];
        assert_eq!(
            hash_files(&dir, &asked),
            Ok(vec![
                FileHash { path: asked[0].clone(), sha256: Some(ABC.to_string()) },
                FileHash { path: asked[1].clone(), sha256: None },
            ])
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_changed_data_acd_has_another_hash() {
        let dir = game_dir("changed");
        fs::write(dir.join("content").join("cars").join("cup_car").join("data.acd"), b"abc with more power").unwrap();
        let hashes = hash_files(&dir, &[DATA_ACD.to_string()]).unwrap();
        assert!(hashes[0].sha256.is_some() && hashes[0].sha256.as_deref() != Some(ABC));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hashes_files_larger_than_its_buffer() {
        let dir = game_dir("large");
        fs::write(dir.join("content").join("cars").join("cup_car").join("data.acd"), vec![b'a'; 1_000_000]).unwrap();
        // SHA-256 of a million times "a", another test vector of the standard.
        assert_eq!(
            hash_files(&dir, &[DATA_ACD.to_string()]).unwrap()[0].sha256.as_deref(),
            Some("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn answers_only_for_the_content_of_the_game() {
        for path in ["content/cars/cup_car/data.acd", "system/data/surfaces.ini", "content/tracks/ring/gp/data/surfaces.ini"] {
            assert!(is_checkable_path(path), "{path}");
        }
        for path in [
            "cfg/race.ini", "content", "content/../../Documents/secret.txt", "content/cars/../../../x", "/content/cars/x/data.acd",
            "content\\cars\\x\\data.acd", "C:/Users/x/file", "content/cars//data.acd", "content/a/b/c/d/e/f/g/h", "",
        ] {
            assert!(!is_checkable_path(path), "{path}");
        }
        let dir = game_dir("refuses");
        assert_eq!(hash_files(&dir, &[DATA_ACD.to_string(), "../outside.txt".to_string()]), Err("../outside.txt".to_string()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_the_build_of_the_custom_shaders_patch() {
        let dir = game_dir("csp");
        assert_eq!(csp_build(&dir), None);
        fs::create_dir_all(dir.join("extension").join("config")).unwrap();
        fs::write(dir.join("extension").join("config").join("data_manifest.ini"), "[VERSION]\r\n; hidden\r\nSHADERS_PATCH=0.3.0-preview581\r\nSHADERS_PATCH_BUILD=4116\r\n").unwrap();
        // The files of the patch without its loader: switched off.
        assert_eq!(csp_build(&dir), None);
        fs::write(dir.join("dwrite.dll"), b"").unwrap();
        assert_eq!(csp_build(&dir), Some(4116));
        fs::remove_dir_all(dir).unwrap();
    }
}
