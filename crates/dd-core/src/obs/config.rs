//! Where OBS keeps its files and what of the driver's own setup the app touches. OBS 31+ keeps the app's settings
//! in `obs-studio/global.ini` and the driver's (with the current profile and collection) in `user.ini`, below
//! `%APPDATA%`, or below `<install>/config` in portable mode. `global.ini [Locations]` may move the profiles, the
//! scene collections and `user.ini` elsewhere: each value is the folder that holds `obs-studio`, and OBS falls back
//! to the default when it does not exist (OBS 33.0.1 `frontend/OBSApp.cpp`). The websocket's settings are in
//! `plugin_config/obs-websocket/config.json`; the app only flips `server_enabled` there, and only while OBS is
//! closed (OBS reads it at start and rewrites it).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Files in OBS's install folder that put it in portable mode (`frontend/obs-main.cpp`).
pub const PORTABLE_MARKERS: [&str; 4] = ["portable_mode", "obs_portable_mode", "portable_mode.txt", "obs_portable_mode.txt"];

/// The files of OBS the app reads or writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub global_ini: PathBuf,
    pub user_ini: PathBuf,
    /// `basic/profiles`
    pub profiles: PathBuf,
    /// `basic/scenes`
    pub scenes: PathBuf,
    /// `plugin_config/obs-websocket/config.json`
    pub websocket: PathBuf,
}

/// The value of `key` in `[section]` of an ini text, as written.
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current = "";
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name;
        } else if current == section {
            if let Some((k, v)) = line.split_once('=') {
                if k == key {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// The text with the given keys of `[section]` set; keys the section does not have stay away. None when nothing
/// changes. Line endings and everything else stay as they are.
pub fn set_ini_values(text: &str, section: &str, values: &[(&str, &str)]) -> Option<String> {
    let mut current = String::new();
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let bare = body.trim_start_matches('\u{feff}').trim();
        if let Some(name) = bare.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.to_string();
        } else if current == section {
            if let Some((key, old)) = bare.split_once('=') {
                if let Some((_, new)) = values.iter().find(|(k, _)| *k == key).filter(|(_, new)| *new != old) {
                    let at = body.find('=').expect("a key line") + 1;
                    out.push_str(&body[..at]);
                    out.push_str(new);
                    out.push_str(&line[body.len()..]);
                    changed = true;
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    changed.then_some(out)
}

/// OBS's install folder from its `bin/64bit/obs64.exe`.
pub fn install_root(exe: &Path) -> Option<&Path> {
    exe.parent()?.parent()?.parent()
}

pub fn portable(install_root: &Path) -> bool {
    PORTABLE_MARKERS.iter().any(|marker| install_root.join(marker).is_file())
}

/// The paths below `root` (`%APPDATA%`, or `<install>/config` in portable mode), with the moved locations of
/// `global_ini` when they exist (`exists`) and OBS is not portable.
pub fn paths(root: &Path, global_ini: Option<&str>, portable: bool, exists: impl Fn(&Path) -> bool) -> Paths {
    let located = |kind: &str| -> PathBuf {
        let moved = (!portable).then(|| global_ini.and_then(|text| ini_value(text, "Locations", kind))).flatten().map(|value| PathBuf::from(value.replace("\\\\", "\\")));
        moved.filter(|dir| exists(dir)).unwrap_or_else(|| root.to_path_buf()).join("obs-studio")
    };
    let app = root.join("obs-studio");
    Paths {
        global_ini: app.join("global.ini"),
        user_ini: located("Configuration").join("user.ini"),
        profiles: located("Profiles").join("basic").join("profiles"),
        scenes: located("SceneCollections").join("basic").join("scenes"),
        websocket: app.join("plugin_config").join("obs-websocket").join("config.json"),
    }
}

/// The driver's current profile and scene collection (`user.ini [Basic]`), which launching OBS with ours
/// overwrites and the app puts back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UserChoice {
    pub profile: Option<String>,
    pub profile_dir: Option<String>,
    pub collection: Option<String>,
    pub collection_file: Option<String>,
}

const KEYS: [&str; 4] = ["Profile", "ProfileDir", "SceneCollection", "SceneCollectionFile"];

pub fn user_choice(user_ini: &str) -> UserChoice {
    let get = |key: &str| ini_value(user_ini, "Basic", key);
    UserChoice { profile: get(KEYS[0]), profile_dir: get(KEYS[1]), collection: get(KEYS[2]), collection_file: get(KEYS[3]) }
}

/// `user_ini` with `choice` back in `[Basic]`; None when it is there already.
pub fn with_user_choice(user_ini: &str, choice: &UserChoice) -> Option<String> {
    let values: Vec<(&str, &str)> = KEYS.iter().zip([&choice.profile, &choice.profile_dir, &choice.collection, &choice.collection_file]).filter_map(|(key, value)| Some((*key, value.as_deref()?))).collect();
    set_ini_values(user_ini, "Basic", &values)
}

/// Whether the websocket server starts with OBS; off when there is no config yet (obs-websocket's default).
pub fn server_enabled(config: Option<&str>) -> bool {
    config.and_then(|text| serde_json::from_str::<Value>(text).ok()).and_then(|v| v["server_enabled"].as_bool()).unwrap_or(false)
}

/// The websocket config with `server_enabled` set and everything else kept; a missing config gets
/// `first_load: false` too, so obs-websocket does not greet the driver with its first-start dialog. None for a config
/// that is not a JSON object: the app does not overwrite what it cannot read.
pub fn with_server_enabled(config: Option<&str>, on: bool) -> Option<String> {
    let mut object = match config {
        Some(text) => match serde_json::from_str::<Value>(text).ok()? {
            Value::Object(object) => object,
            _ => return None,
        },
        None => Map::from_iter([("first_load".to_string(), Value::Bool(false))]),
    };
    object.insert("server_enabled".to_string(), Value::Bool(on));
    Some(serde_json::to_string_pretty(&Value::Object(object)).expect("a JSON value"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Joe's user.ini [Basic] keys after a run (OBS 33.0.1), as `03-obs-restore.ps1` reset them.
    const USER_INI: &str = "\u{feff}[General]\r\nPre31Migrated=true\r\n\r\n[Basic]\r\nProfile=DD Test\r\nProfileDir=DD_Test\r\nSceneCollection=DD Test\r\nSceneCollectionFile=DD_Test\r\nConfigOnNewProfile=true\r\n\r\n[BasicWindow]\r\nProfile=other\r\n";

    #[test]
    fn reads_and_puts_back_the_drivers_profile_and_collection() {
        let ours = user_choice(USER_INI);
        assert_eq!(ours.profile.as_deref(), Some("DD Test"));
        assert_eq!(ours.collection_file.as_deref(), Some("DD_Test"));
        let joes = UserChoice { profile: Some("0xJoe".into()), profile_dir: Some("0xJoe".into()), collection: Some("Unbenannt".into()), collection_file: Some("Unbenannt".into()) };
        let back = with_user_choice(USER_INI, &joes).unwrap();
        assert_eq!(user_choice(&back), joes);
        assert!(back.starts_with('\u{feff}') && back.contains("\r\nConfigOnNewProfile=true\r\n"));
        assert!(back.ends_with("[BasicWindow]\r\nProfile=other\r\n"), "only [Basic] changes");
        assert_eq!(back.len(), USER_INI.len() - "DD TestDD_TestDD TestDD_Test".len() + "0xJoe0xJoeUnbenanntUnbenannt".len());
        assert_eq!(with_user_choice(&back, &joes), None, "nothing to change");
        // a key the driver's file never had stays away, a choice without a value leaves the key
        let partial = UserChoice { profile: Some("0xJoe".into()), ..Default::default() };
        let text = with_user_choice("[Basic]\nProfile=DD Test\n", &partial).unwrap();
        assert_eq!(text, "[Basic]\nProfile=0xJoe\n");
    }

    #[test]
    fn flips_only_server_enabled() {
        // Joe's config.json (A-obs-quality.md 5.2), password made up
        let joe = r#"{"alerts_enabled":false,"auth_required":true,"first_load":false,"server_enabled":false,"server_password":"abc","server_port":4455}"#;
        assert!(!server_enabled(Some(joe)));
        let on = with_server_enabled(Some(joe), true).unwrap();
        assert!(server_enabled(Some(&on)));
        let mut expected: Value = serde_json::from_str(joe).unwrap();
        expected["server_enabled"] = Value::Bool(true);
        assert_eq!(serde_json::from_str::<Value>(&on).unwrap(), expected);
        let fresh: Value = serde_json::from_str(&with_server_enabled(None, true).unwrap()).unwrap();
        assert_eq!(fresh, serde_json::json!({ "first_load": false, "server_enabled": true }));
        assert!(!server_enabled(None));
        assert_eq!(with_server_enabled(Some("not json"), true), None);
        assert_eq!(with_server_enabled(Some("[]"), true), None);
    }

    #[test]
    fn finds_the_files_also_where_locations_moved_them() {
        let root = Path::new(r"C:\Users\thecy\AppData\Roaming");
        let plain = paths(root, None, false, |_| true);
        assert_eq!(plain.user_ini, root.join("obs-studio").join("user.ini"));
        assert_eq!(plain.profiles, root.join("obs-studio").join("basic").join("profiles"));
        assert_eq!(plain.websocket, root.join("obs-studio").join("plugin_config").join("obs-websocket").join("config.json"));
        let global = "[General]\nLastVersion=553648129\n\n[Locations]\nConfiguration=C:\\\\Users\\\\thecy\\\\AppData\\\\Roaming\nProfiles=D:\\\\OBS\nSceneCollections=E:\\\\gone\n";
        let moved = paths(root, Some(global), false, |dir| !dir.to_string_lossy().starts_with("E:"));
        assert_eq!(moved.profiles, Path::new(r"D:\OBS").join("obs-studio").join("basic").join("profiles"));
        assert_eq!(moved.scenes, plain.scenes, "a location that does not exist falls back");
        assert_eq!(moved.user_ini, plain.user_ini);
        assert_eq!(moved.global_ini, root.join("obs-studio").join("global.ini"));
        assert_eq!(paths(root, Some(global), true, |_| true).profiles, plain.profiles, "portable ignores the locations");
    }

    #[test]
    fn knows_portable_mode_by_its_marker() {
        let dir = std::env::temp_dir().join(format!("dd-obs-portable-{}", std::process::id()));
        let exe = dir.join("bin").join("64bit").join("obs64.exe");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let root = install_root(&exe).unwrap();
        assert_eq!(root, dir);
        assert!(!portable(root));
        fs::write(dir.join("obs_portable_mode.txt"), b"").unwrap();
        assert!(portable(root));
        fs::remove_dir_all(dir).unwrap();
    }
}
