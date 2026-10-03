use std::fs;
use std::path::PathBuf;

use dd_core::steam::{find_ac_evo, library_paths, steam_id64};

const FIXTURE: &str = include_str!("fixtures/libraryfolders.vdf");

/// Fresh empty directory under the system temp dir, unique per test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dd-core-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
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
fn finds_assetto_corsa_evo_in_the_second_library() {
    let root = temp_dir("evo");
    let libraries = vec![root.join("lib-a"), root.join("lib-b")];
    for library in &libraries {
        fs::create_dir_all(library.join("steamapps/common")).unwrap();
    }
    assert_eq!(find_ac_evo(&libraries), None);
    let evo = libraries[1].join("steamapps/common/Assetto Corsa EVO");
    fs::create_dir_all(&evo).unwrap();
    // The folder alone is what Steam leaves behind after the game was removed.
    assert_eq!(find_ac_evo(&libraries), None);
    fs::write(evo.join("AssettoCorsaEVO.exe"), b"").unwrap();
    assert_eq!(find_ac_evo(&libraries), Some(evo));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn works_out_the_steam_id_of_the_active_account() {
    assert_eq!(steam_id64(4_782_979), Some("76561197965048707".to_string()));
    assert_eq!(steam_id64(0), None);
}
