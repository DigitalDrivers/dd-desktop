//! Locating Assetto Corsa EVO inside the user's Steam libraries, and the account Steam runs with.

use std::path::PathBuf;

/// SteamID64 of the first individual account; an account's id is added to it.
const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;

/// SteamID64 of the account Steam reports as active (`ActiveProcess\ActiveUser` in the registry);
/// 0 there means Steam is not running or nobody is signed in.
pub fn steam_id64(active_user: u32) -> Option<String> {
    (active_user != 0).then(|| (STEAM_ID64_BASE + u64::from(active_user)).to_string())
}

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

/// Returns the Assetto Corsa EVO folder of the first library that contains the game.
pub fn find_ac_evo(libraries: &[PathBuf]) -> Option<PathBuf> {
    libraries
        .iter()
        .map(|library| library.join("steamapps").join("common").join("Assetto Corsa EVO"))
        .find(|dir| dir.join("AssettoCorsaEVO.exe").is_file())
}
