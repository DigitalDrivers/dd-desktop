use std::fs;
use std::path::PathBuf;

use dd_core::steam::{find_assetto_corsa, is_assetto_corsa_dir, library_paths};

const FIXTURE: &str = include_str!("fixtures/libraryfolders.vdf");

/// Fresh empty directory under the system temp dir, unique per test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dd-core-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Creates a fake Steam library; with `with_ac` it contains an Assetto Corsa executable.
fn fake_library(root: &PathBuf, name: &str, with_ac: bool) -> PathBuf {
    let library = root.join(name);
    let ac = library.join("steamapps/common/assettocorsa");
    fs::create_dir_all(&ac).unwrap();
    if with_ac {
        fs::write(ac.join("AssettoCorsa.exe"), b"").unwrap();
    }
    library
}

#[test]
fn reads_all_library_paths_in_file_order_and_unescapes_backslashes() {
    assert_eq!(
        library_paths(FIXTURE),
        vec![
            PathBuf::from(r"C:\Program Files (x86)\Steam"),
            PathBuf::from(r"D:\SteamLibrary"),
        ]
    );
}

#[test]
fn ignores_other_keys_and_malformed_lines() {
    let vdf = "\"libraryfolders\"\n{\n\t\"label\"\t\t\"path\"\n\t\"path\"\n\t\"path\"\t\t\"\"\n}\n";
    assert!(library_paths(vdf).is_empty());
}

#[test]
fn finds_the_game_in_the_second_library() {
    let root = temp_dir("second");
    let libraries = vec![
        fake_library(&root, "lib-a", false),
        fake_library(&root, "lib-b", true),
    ];
    assert_eq!(
        find_assetto_corsa(&libraries),
        Some(libraries[1].join("steamapps/common/assettocorsa"))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn returns_none_when_no_library_contains_the_game() {
    let root = temp_dir("none");
    let libraries = vec![fake_library(&root, "lib-a", false)];
    assert_eq!(find_assetto_corsa(&libraries), None);
    assert!(!is_assetto_corsa_dir(&libraries[0].join("steamapps/common/assettocorsa")));
    fs::remove_dir_all(root).unwrap();
}
