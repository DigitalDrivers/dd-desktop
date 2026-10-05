//! The club's own cars for Assetto Corsa EVO. A car is one package, `Saved Games\ACE\mods\<id>.kspkg`, which
//! the app puts there from the platform and keeps up to date. A new version can drop a part or preset that a
//! driver's saved car still points at; the game then crashes on every start while that car is the selected
//! one. So after an update the saved cars of that car are checked against the package's file table, the stale
//! ones move to `SavedCars\stale` (never deleted), and a garage that selects one of them is pointed at a stock
//! car, after a backup.

use std::collections::HashSet;

/// The package's file table: the last 64 MB of the file, XOR'd with this key.
pub const TABLE_SIZE: usize = 0x400_0000;
const KEY: [u8; 8] = [0xc1, 0x35, 0x11, 0x7d, 0xa9, 0x21, 0x97, 0x9f];
const ENTRY: usize = 0x100;
const PATH_BYTES: usize = 0xE0;

/// A car id as the game takes it: a lower-case content folder name.
pub fn is_car_id(id: &str) -> bool {
    (3..=100).contains(&id.len()) && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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

/// What the end of a game log says about why the game stopped, for support: the lines a broken car file leaves
/// (a failed protobuf check, a file not found, a critical error), else the last lines, where a crash cuts the
/// log off. At most three. The game writes many harmless `[error]` lines, so those alone say nothing.
pub fn crash_lines(log: &str) -> Vec<String> {
    let lines: Vec<&str> = log.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
    let marked: Vec<&str> = lines.iter().copied().filter(|l| l.contains("CHECK failed") || l.contains("Failed to find") || l.contains("] [critical]")).collect();
    let pick = if marked.is_empty() { &lines[..] } else { &marked[..] };
    pick[pick.len().saturating_sub(3)..].iter().map(|l| l.chars().take(300).collect()).collect()
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
        let log = "[t] [gameplay] [error] Empty detector_pit?\n[t] [core] [info] loading dd_x\n[t] [core] [critical] Protobuf map CHECK failed: key not found\n[t] [core] [info] last\n";
        assert_eq!(crash_lines(log), vec!["[t] [core] [critical] Protobuf map CHECK failed: key not found"]);
        let plain = "a\nb\n\nc\nd\n";
        assert_eq!(crash_lines(plain), vec!["b", "c", "d"]);
        assert!(crash_lines("").is_empty());
    }

    #[test]
    fn takes_only_car_ids() {
        assert!(is_car_id("dd_bmw_m3_e46_gt3"));
        for bad in ["", "..", "DD_X", "dd x", "dd\\..\\x", "a"] {
            assert!(!is_car_id(bad), "{bad}");
        }
    }
}
