//! Locating Assetto Corsa inside the user's Steam libraries.

use std::path::{Path, PathBuf};

/// Folder of Assetto Corsa relative to a Steam library root.
const AC_RELATIVE_DIR: &str = "steamapps/common/assettocorsa";
/// File that must exist for a folder to count as an Assetto Corsa installation.
const AC_EXECUTABLE: &str = "AssettoCorsa.exe";

/// Extracts the library root paths from the text of Steam's `libraryfolders.vdf`.
///
/// The file lists each library as `"path"  "C:\\Program Files (x86)\\Steam"`; backslashes are
/// escaped by doubling. Entries are returned in file order.
pub fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let mut quoted = line.split('"').skip(1).step_by(2);
            match (quoted.next(), quoted.next()) {
                (Some("path"), Some(value)) if !value.is_empty() => {
                    Some(PathBuf::from(value.replace("\\\\", "\\")))
                }
                _ => None,
            }
        })
        .collect()
}

/// Returns the Assetto Corsa folder of the first library that contains the game.
pub fn find_assetto_corsa(libraries: &[PathBuf]) -> Option<PathBuf> {
    libraries
        .iter()
        .map(|library| library.join(AC_RELATIVE_DIR))
        .find(|dir| is_assetto_corsa_dir(dir))
}

/// True when `dir` looks like an Assetto Corsa installation.
pub fn is_assetto_corsa_dir(dir: &Path) -> bool {
    dir.join(AC_EXECUTABLE).is_file()
}
