//! The club's own cars for Assetto Corsa EVO. A car is one package, `Saved Games\ACE\mods\<id>.kspkg`, which
//! the app puts there from the platform and keeps up to date. A new version can drop a part or preset that a
//! driver's saved car still points at; the game then crashes when it loads that car, and it loads the selected
//! car on every start. So after an update the saved cars of that car which the check against the package's file
//! table finds stale move to `SavedCars\stale` (never deleted), the others stay in the game's "My cars", and a
//! garage that selects any saved car of that car is pointed at a stock car, after a backup.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

/// The package's file table: the last 64 MB of the file, one `ENTRY` per file, XOR'd with `KEY`.
pub const TABLE_SIZE: usize = 0x400_0000;
pub const KEY: [u8; 8] = [0xc1, 0x35, 0x11, 0x7d, 0xa9, 0x21, 0x97, 0x9f];
pub const ENTRY: usize = 0x100;
const PATH_BYTES: usize = 0xE0;

/// A car id as the game takes it: a lower-case content folder name.
pub fn is_car_id(id: &str) -> bool {
    (3..=100).contains(&id.len()) && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The version in the address `install_car` downloads a package from, when the address is exactly the
/// platform's package of car `id` with a download ticket: `/api/garage/<id>/<version>/package?ticket=<expiry>.<hex>`.
/// Anything else is refused, `..` above all: the HTTP client resolves it, so a prefix check let a page download
/// any answer of the platform as the package (security audit 2026-10-09, S9).
pub fn package_version(id: &str, path: &str) -> Option<u32> {
    let rest = path.strip_prefix("/api/garage/")?.strip_prefix(id)?.strip_prefix('/')?;
    let (version, ticket) = rest.split_once("/package?ticket=")?;
    let (expiry, signature) = ticket.split_once('.')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let hex = signature.len() == 64 && signature.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !digits(version) || version.starts_with('0') || !digits(expiry) || !hex {
        return None;
    }
    version.parse().ok()
}

/// The public key of the club's car packages (minisign, Ed25519): its own key, not the one of app updates. The
/// private half stays with the car builder (dd-platform scripts/car-signing.mjs), outside every repository.
pub const PACKAGE_KEY: &str = "RWT0+kQp/ceIjLZHAQhCH/jziaQoii/n2Rmvbt64t9shMOuKF37dlOV9";

/// Whether `signature` (a minisign signature as text, as the platform hands it out) of `key` says that the
/// package with this SHA-256 is version `version` of car `id`. The club signs that statement, not the 200 MB
/// package itself: the app checks the package's SHA-256 anyway, and the statement binds the bytes to the car and
/// version, so a package of another car or version cannot be passed off (security audit 2026-10-09, S4).
pub fn is_signed(key: &str, signature: &str, id: &str, version: u32, sha256: &str) -> bool {
    let (Ok(key), Ok(signature)) = (minisign_verify::PublicKey::from_base64(key), minisign_verify::Signature::decode(signature)) else {
        return false;
    };
    key.verify(format!("dd-car-package:{id}:{version}:{sha256}").as_bytes(), &signature, false).is_ok()
}

/// What installing version `offered` of a car comes to. `installed` is the version the app last put into the
/// game for it (also the highest: it never installs a lower one), `on_disk` whether the package in the game is
/// the offered one (same SHA-256). A lower version than the installed one is refused: a compromised platform
/// could hand out an older version the club once signed, with its faults (security audit 2026-10-09). The club
/// rolls a car back by publishing the old state under a new, higher version.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallPlan {
    Install,
    NothingToDo,
    Downgrade { installed: u32 },
}

pub fn install_plan(installed: Option<u32>, offered: u32, on_disk: bool) -> InstallPlan {
    match installed {
        Some(installed) if offered < installed => InstallPlan::Downgrade { installed },
        _ if on_disk => InstallPlan::NothingToDo,
        _ => InstallPlan::Install,
    }
}

/// The file in the app's config folder that keeps the version of car `id` the app installed last.
fn version_file(dir: &Path, id: &str) -> Option<std::path::PathBuf> {
    is_car_id(id).then(|| dir.join("cars").join(format!("{id}.version")))
}

/// The version of car `id` the app installed last on this PC; None before its first install by an app that keeps
/// the version.
pub fn installed_version(dir: &Path, id: &str) -> Option<u32> {
    fs::read_to_string(version_file(dir, id)?).ok()?.trim().parse().ok()
}

/// The version of car `id` the app installed last, as long as `package` (the game's `mods\<id>.kspkg`) is the file
/// it installed then: the app records the version after it put the package in place, so a package changed after
/// that (a builder's own build copied in) has no known version.
pub fn installed_package_version(dir: &Path, id: &str, package: &Path) -> Option<u32> {
    let recorded = fs::metadata(version_file(dir, id)?).ok()?.modified().ok()?;
    let changed = fs::metadata(package).ok()?.modified().ok()?;
    if changed > recorded {
        return None;
    }
    installed_version(dir, id)
}

pub fn record_installed(dir: &Path, id: &str, version: u32) -> io::Result<()> {
    let file = version_file(dir, id).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a car id"))?;
    fs::create_dir_all(file.parent().unwrap())?;
    fs::write(file, version.to_string())
}

/// The paths in a package's file table, lower case (`content\cars\<id>\...`). `table` is the table as it is
/// on disk, still XOR'd.
pub fn package_paths(table: &[u8]) -> HashSet<String> {
    table
        .chunks_exact(ENTRY)
        .filter(|entry| entry[0] != KEY[0])
        .map(|entry| {
            let path: Vec<u8> = entry[..PATH_BYTES].iter().enumerate().map(|(i, b)| b ^ KEY[i % 8]).take_while(|b| *b != 0).collect();
            String::from_utf8_lossy(&path).to_lowercase()
        })
        .collect()
}

/// Whether a saved car points at a part, preset, rim or design the package no longer has.
pub fn is_stale(saved_car: &[u8], car_id: &str, package: &HashSet<String>) -> bool {
    references(saved_car, car_id).iter().any(|r| !package.contains(&r.to_lowercase()))
}

const KINDS: [&str; 5] = ["compatiblepart", "mechanicalcarpreset", "visualcarpreset", "compatiblerim", "design"];

/// The paths of the car's own files a saved car refers to (`content\cars\<id>\...`). The game writes each as
/// a protobuf string, so the varint just before it is its length, and every kind of file counts: a saved car
/// that points at a livery's `.material` the new version dropped crashes the game on every start as well
/// (2026-10-05, after the GT3 and Clubsport got new liveries; materials were not among `KINDS`). Where no
/// length fits, the shortest path that ends in one of `KINDS`.
fn references(data: &[u8], car_id: &str) -> Vec<String> {
    let prefix = format!("content\\cars\\{car_id}\\").into_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while let Some(at) = data[i..].windows(prefix.len()).position(|w| w == prefix.as_slice()) {
        let start = i + at;
        if let Some(path) = length_prefixed(data, start, prefix.len()) {
            found.push(path);
            i = start + prefix.len();
            continue;
        }
        let mut end = start + prefix.len();
        while end < data.len() && (0x20..=0x7e).contains(&data[end]) {
            end += 1;
        }
        let text = String::from_utf8_lossy(&data[start..end]).to_string();
        // The shortest path that ends in one of the kinds, as the game writes it.
        let lower = text.to_lowercase();
        let ends = KINDS.iter().filter_map(|k| lower.find(&format!(".{k}")).map(|p| p + 1 + k.len())).min();
        if let Some(end_at) = ends {
            found.push(text[..end_at].to_string());
        }
        i = start + prefix.len();
    }
    found
}

/// The string that starts at `start` when the varint before it (one or two bytes) is its length: printable
/// ASCII, at least `min` long, ending in a file extension.
fn length_prefixed(data: &[u8], start: usize, min: usize) -> Option<String> {
    let last = *data.get(start.checked_sub(1)?)?;
    if last & 0x80 != 0 {
        return None;
    }
    let mut lengths = Vec::new();
    if let Some(first) = start.checked_sub(2).and_then(|p| data.get(p)).filter(|b| *b & 0x80 != 0) {
        lengths.push(usize::from(first & 0x7f) | usize::from(last) << 7);
    }
    lengths.push(usize::from(last));
    lengths.into_iter().find_map(|len| {
        let bytes = data.get(start..start.checked_add(len)?)?;
        let name = bytes.rsplit(|b| *b == b'\\').next()?;
        let extension = name.rsplit(|b| *b == b'.').next().filter(|e| e.len() < name.len() && !e.is_empty())?;
        let fits = len >= min && bytes.iter().all(|b| (0x20..=0x7e).contains(b)) && extension.iter().all(u8::is_ascii_alphanumeric);
        fits.then(|| String::from_utf8_lossy(bytes).to_string())
    })
}

/// The pguid in a saved car's file name (`<id>_<pguid with dashes>.carfinalstatewithconsumable`), as 32 hex chars.
pub fn pguid_of(file_name: &str) -> Option<String> {
    let stem = file_name.strip_suffix(".carfinalstatewithconsumable")?;
    let hex: String = stem.rsplit('_').next()?.chars().filter(|c| *c != '-').collect::<String>().to_lowercase();
    (hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(hex)
}

fn varint(data: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// The car the garage has selected (`DriverGarageData.selected_car_pguid`, field 4 with a = 1, b = 2), as
/// 32 hex chars like `pguid_of`.
pub fn selected_pguid(garage: &[u8]) -> Option<String> {
    let mut at = 0;
    while at < garage.len() {
        let key = varint(garage, &mut at)?;
        match key & 7 {
            0 => {
                varint(garage, &mut at)?;
            }
            2 => {
                let len = varint(garage, &mut at)? as usize;
                let body = garage.get(at..at + len)?;
                at += len;
                if key >> 3 == 4 {
                    let (mut a, mut b, mut i) = (0, 0, 0);
                    while i < body.len() {
                        let k = varint(body, &mut i)?;
                        let v = varint(body, &mut i)?;
                        match k >> 3 {
                            1 => a = v,
                            2 => b = v,
                            _ => {}
                        }
                    }
                    return Some(format!("{a:016x}{b:016x}"));
                }
            }
            1 => at += 8,
            5 => at += 4,
            _ => return None,
        }
    }
    None
}

/// The last 20 lines of a game log, for support. A crash cuts the log off right after the line that says why
/// (2026-10-05: `[critical] Protobuf: ... CHECK failed: it != end(): key not found:`), with what the game was
/// loading just before it; a clean exit ends with the shutdown. Picking marked lines misleads: a normal session
/// logs harmless `[critical]` lines too (`tyre_texture_data is empty`).
pub fn crash_lines(log: &str) -> Vec<String> {
    let lines: Vec<&str> = log.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(20)..].iter().map(|l| l.chars().take(300).collect()).collect()
}

/// A garage that selects a stock car every player has, the Kunos Porsche 992 GT3 Cup
/// (4f44a4be-5be3-3c37-2b05-1984457156ac), version 7 as the game writes it.
pub const RESCUED_GARAGE: [u8; 24] = [
    0x22, 0x14, 0x08, 0xb7, 0xf8, 0x8c, 0xdf, 0xe5, 0x97, 0xa9, 0xa2, 0x4f, 0x10, 0xac, 0xad, 0xc5, 0xab, 0xc4, 0xb0, 0xc6, 0x82, 0x2b, 0x50, 0x07,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn xor_entry(path: &str) -> Vec<u8> {
        let mut entry = vec![0u8; ENTRY];
        entry[..path.len()].copy_from_slice(path.as_bytes());
        entry.iter().enumerate().map(|(i, b)| b ^ KEY[i % 8]).collect()
    }

    #[test]
    fn reads_the_paths_of_a_package() {
        let mut table = xor_entry("content\\cars\\dd_x\\Presets\\GT3.mechanicalcarpreset");
        table.extend(xor_entry("content\\cars\\dd_x\\parts\\wing.compatiblepart"));
        // An empty entry is all key bytes once XOR'd.
        table.extend(xor_entry(""));
        let paths = package_paths(&table);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains("content\\cars\\dd_x\\presets\\gt3.mechanicalcarpreset"));
    }

    #[test]
    fn finds_a_saved_car_that_points_at_a_part_the_package_lost() {
        let package: HashSet<String> = ["content\\cars\\dd_x\\presets\\gt3.mechanicalcarpreset".to_string()].into();
        let ok = b"\x0a\x31content\\cars\\dd_x\\Presets\\GT3.mechanicalcarpreset\x12\x00".to_vec();
        assert!(!is_stale(&ok, "dd_x", &package));
        let lost = [ok.clone(), b"\x1acontent\\cars\\dd_x\\parts\\csl_rim.compatiblerim\x00".to_vec()].concat();
        assert!(is_stale(&lost, "dd_x", &package));
        // Kunos files the car uses by path are not the package's business.
        let kunos = [ok, b"content\\cars\\ks_bmw_m4_gt3\\rims\\x.compatiblerim".to_vec()].concat();
        assert!(!is_stale(&kunos, "dd_x", &package));
    }

    #[test]
    fn finds_a_saved_car_that_points_at_a_livery_material_the_package_lost() {
        let material = "content\\cars\\dd_x\\skins\\dd_mstripes\\EXT_SKIN_DD_MSTRIPES.material";
        let saved = [vec![0x42, material.len() as u8], material.as_bytes().to_vec(), b"R\x0b".to_vec()].concat();
        let with: HashSet<String> = [material.to_lowercase()].into();
        assert!(!is_stale(&saved, "dd_x", &with));
        assert!(is_stale(&saved, "dd_x", &HashSet::new()));
    }

    #[test]
    fn reads_a_path_whose_length_takes_two_bytes() {
        let path = format!("content\\cars\\dd_x\\{}\\part.compatiblepart", "p".repeat(120));
        let saved = [vec![0x0a, 0x80 | (path.len() & 0x7f) as u8, (path.len() >> 7) as u8], path.as_bytes().to_vec(), b"Z".to_vec()].concat();
        assert_eq!(references(&saved, "dd_x"), vec![path]);
    }

    #[test]
    fn reads_the_selected_car_and_the_pguid_of_a_saved_car() {
        // The user's garage selecting the GT3 Cup, as the game wrote it.
        assert_eq!(selected_pguid(&RESCUED_GARAGE).as_deref(), Some("4f44a4be5be33c372b051984457156ac"));
        assert_eq!(
            pguid_of("dd_bmw_m3_e46_gt3_4F44A4BE-5BE3-3C37-2B05-1984457156AC.carfinalstatewithconsumable").as_deref(),
            Some("4f44a4be5be33c372b051984457156ac")
        );
        assert_eq!(pguid_of("dd_bmw_m3_e46_gt3_x.carfinalstatewithconsumable"), None);
        assert_eq!(selected_pguid(b"\x50\x07"), None);
    }

    #[test]
    fn tells_why_the_game_stopped() {
        // A harmless critical line early in the session, the crash at the end, as the game logs them.
        let mut log = String::from("[t] [dataUtils] [critical] tyre_texture_data is empty (tyre_compund is 120)\n");
        for i in 0..30 {
            log += &format!("[t] [core] [info] line {i}\n\n");
        }
        log += "[t] [platformCore] [critical] Protobuf: map.h:1060 CHECK failed: it != end(): key not found:\n";
        let lines = crash_lines(&log);
        assert_eq!(lines.len(), 20);
        assert_eq!(lines[0], "[t] [core] [info] line 11");
        assert!(lines[19].ends_with("key not found:"));
        assert_eq!(crash_lines("a\n\nb\n"), vec!["a", "b"]);
        assert!(crash_lines("").is_empty());
    }

    #[test]
    fn takes_only_the_package_address_of_the_car() {
        let ticket = format!("1760000000.{}", "ab".repeat(32));
        assert_eq!(package_version("dd_x", &format!("/api/garage/dd_x/31/package?ticket={ticket}")), Some(31));
        for bad in [
            // `..` is resolved by the HTTP client: this is /api/evo/hotlaps (security audit 2026-10-09, S9).
            format!("/api/garage/dd_x/../../evo/hotlaps?ticket={ticket}"),
            format!("/api/garage/dd_x/31/../../dd_y/31/package?ticket={ticket}"),
            format!("/api/garage/dd_x/31/%2e%2e/package?ticket={ticket}"),
            format!("/api/garage/dd_x/31/preview?ticket={ticket}"),
            format!("/api/garage/dd_y/31/package?ticket={ticket}"),
            format!("/api/garage/dd_x/31/package?ticket={ticket}&x=1"),
            format!("/api/garage/dd_x/31/package?ticket={ticket}#x"),
            format!("/api/garage/dd_x/031/package?ticket={ticket}"),
            format!("/api/garage/dd_x/+31/package?ticket={ticket}"),
            "/api/garage/dd_x/31/package?ticket=1760000000.ab".to_string(),
            format!("/api/garage/dd_x/31/package?ticket=1760000000.{}", "AB".repeat(32)),
            format!("/api/garage/dd_x/31/package?ticket=.{}", "ab".repeat(32)),
            "/api/garage/dd_x/31/package".to_string(),
            format!("//evil.example/api/garage/dd_x/31/package?ticket={ticket}"),
        ] {
            assert_eq!(package_version("dd_x", &bad), None, "{bad}");
        }
    }

    /// Made by dd-platform's scripts/car-signing.mjs with a key made for this test only.
    const TEST_KEY: &str = "RWSk5AM3Hs0I5zE/5SvFlmAijnh/pKxFsGCUEGzl2ME0l8ir7c2j4YzC";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from the Digital Drivers car package key\nRUSk5AM3Hs0I520lPCdJo/qX5epVcQ0jJmRDmUgiKxIBiLg0UcXYxDwYqCDVYPrpjWGleHefksT5Mt8gqkJCyshZYIdwYRmGawM=\ntrusted comment: dd-car-package:dd_x:31:abababababababababababababababababababababababababababababababab\njC3dz0OnMffwh0gj63gZEJctYUWi4vPTPNj09tm/3+KOzjxNCqzyfRDyUU56FuJx88OsZ6J+iCVD4wV6r/w3AQ==\n";

    #[test]
    fn installs_only_a_package_the_club_signed_for_this_car_and_version() {
        let sha = "ab".repeat(32);
        assert!(is_signed(TEST_KEY, TEST_SIGNATURE, "dd_x", 31, &sha));
        // The signature names car, version and package: none of them can be swapped.
        assert!(!is_signed(TEST_KEY, TEST_SIGNATURE, "dd_y", 31, &sha));
        assert!(!is_signed(TEST_KEY, TEST_SIGNATURE, "dd_x", 30, &sha));
        assert!(!is_signed(TEST_KEY, TEST_SIGNATURE, "dd_x", 31, &"cd".repeat(32)));
        // Another key's signature, a changed one, none at all.
        assert!(!is_signed(PACKAGE_KEY, TEST_SIGNATURE, "dd_x", 31, &sha));
        assert!(!is_signed(TEST_KEY, &TEST_SIGNATURE.replace("trusted comment: dd", "trusted comment: xx"), "dd_x", 31, &sha));
        assert!(!is_signed(TEST_KEY, &TEST_SIGNATURE.replace("RUSk5AM3Hs0I520l", "RUSk5AM3Hs0I520m"), "dd_x", 31, &sha));
        assert!(!is_signed(TEST_KEY, "", "dd_x", 31, &sha));
        assert!(!is_signed(TEST_KEY, "not a signature", "dd_x", 31, &sha));
    }

    #[test]
    fn has_the_club_key_built_in() {
        assert!(minisign_verify::PublicKey::from_base64(PACKAGE_KEY).is_ok());
        assert_ne!(PACKAGE_KEY, TEST_KEY);
    }

    #[test]
    fn never_installs_a_lower_version_than_the_one_installed() {
        // A compromised platform could hand out an older version the club once signed (security audit 2026-10-09).
        assert_eq!(install_plan(Some(31), 30, false), InstallPlan::Downgrade { installed: 31 });
        assert_eq!(install_plan(Some(31), 1, false), InstallPlan::Downgrade { installed: 31 });
        // Also when a driver put the older package back by hand: the app does not install it again.
        assert_eq!(install_plan(Some(31), 30, true), InstallPlan::Downgrade { installed: 31 });
        // The same version already in the game: nothing to do; missing or damaged, it comes again.
        assert_eq!(install_plan(Some(31), 31, true), InstallPlan::NothingToDo);
        assert_eq!(install_plan(Some(31), 31, false), InstallPlan::Install);
        // Newer versions, and cars the app has not installed yet (or before it kept their version).
        assert_eq!(install_plan(Some(31), 32, false), InstallPlan::Install);
        assert_eq!(install_plan(None, 5, false), InstallPlan::Install);
        assert_eq!(install_plan(None, 5, true), InstallPlan::NothingToDo);
    }

    #[test]
    fn keeps_the_installed_version_of_each_car() {
        let dir = std::env::temp_dir().join(format!("dd-garage-{}", std::process::id())).join("config");
        assert_eq!(installed_version(&dir, "dd_x"), None);
        // The config folder does not exist before the first car.
        record_installed(&dir, "dd_x", 31).unwrap();
        record_installed(&dir, "dd_y", 4).unwrap();
        assert_eq!(installed_version(&dir, "dd_x"), Some(31));
        assert_eq!(installed_version(&dir, "dd_y"), Some(4));
        record_installed(&dir, "dd_x", 32).unwrap();
        assert_eq!(installed_version(&dir, "dd_x"), Some(32));
        // Only car ids name a file.
        assert!(record_installed(&dir, "..", 1).is_err());
        assert_eq!(installed_version(&dir, ".."), None);
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn names_the_installed_version_only_while_the_package_is_the_one_the_app_installed() {
        let root = std::env::temp_dir().join(format!("dd-garage-package-{}", std::process::id()));
        let (dir, package) = (root.join("config"), root.join("mods").join("dd_x.kspkg"));
        fs::create_dir_all(package.parent().unwrap()).unwrap();
        // no package, or none installed by the app
        assert_eq!(installed_package_version(&dir, "dd_x", &package), None);
        fs::write(&package, b"v31").unwrap();
        assert_eq!(installed_package_version(&dir, "dd_x", &package), None);
        // the app writes the package, then its version
        let at = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        fs::File::options().write(true).open(&package).unwrap().set_modified(at).unwrap();
        record_installed(&dir, "dd_x", 31).unwrap();
        assert_eq!(installed_package_version(&dir, "dd_x", &package), Some(31));
        // a package copied in after that (a builder's own build) is not version 31
        fs::File::options().write(true).open(&package).unwrap().set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60)).unwrap();
        assert_eq!(installed_package_version(&dir, "dd_x", &package), None);
        assert_eq!(installed_package_version(&dir, "..", &package), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn takes_only_car_ids() {
        assert!(is_car_id("dd_bmw_m3_e46_gt3"));
        for bad in ["", "..", "DD_X", "dd x", "dd\\..\\x", "a"] {
            assert!(!is_car_id(bad), "{bad}");
        }
    }
}
