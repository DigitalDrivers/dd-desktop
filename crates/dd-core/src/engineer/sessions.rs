//! Which MoTeC sessions to evaluate: the sessions as [`motec::sessions`] finds them (one folder and preset,
//! cumulative files, at most 30 minutes apart), each once it ended, by its newest file. The app remembers what it
//! evaluated (`<config>/engineer/evaluated.json`, by name, size and time), as it does for the uploads; a session
//! that grew by a lap after its evaluation is evaluated again, by its new newest file, and so is one evaluated by
//! an older evaluator ([`EVALUATOR`]: 2 since the `.ldx` beacons give laps to sessions without the lap list).

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::motec::{self, MotecFile, HEADER_ONLY, MAX_AGE_SECS, QUIET_SECS};

const LEDGER: &str = "evaluated.json";
/// The evaluator's version: raised when evaluating again can give more than before.
pub const EVALUATOR: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evaluated {
    pub name: String,
    pub size: u64,
    pub modified: u64,
    /// When it was evaluated, seconds since 1970
    pub at: u64,
    /// The evaluator's version it was evaluated by; 0 (absent) before 0.25.0's beacons
    #[serde(default)]
    pub version: u32,
}

/// The newest file of every session to evaluate, the oldest first: the session ended (the game is closed, or
/// wrote no file of it for 10 minutes), its newest file is more than a header and of the last 30 days, and it was
/// not evaluated as it is.
pub fn ready<'a>(files: &'a [MotecFile], evaluated: &[Evaluated], now: u64, game_running: bool) -> Vec<&'a MotecFile> {
    motec::sessions(files)
        .into_iter()
        .filter_map(|chain| chain.last().copied())
        .filter(|f| !game_running || now.saturating_sub(f.modified) >= QUIET_SECS)
        .filter(|f| f.size >= HEADER_ONLY && now.saturating_sub(f.modified) <= MAX_AGE_SECS)
        .filter(|f| !evaluated.iter().any(|e| e.name == f.name && e.size == f.size && e.modified == f.modified && e.version >= EVALUATOR))
        .collect()
}

pub fn evaluated(dir: &Path) -> Vec<Evaluated> {
    fs::read(dir.join(LEDGER)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

pub fn record_evaluated(dir: &Path, file: &MotecFile, at: u64) -> io::Result<()> {
    let mut all = evaluated(dir);
    all.retain(|e| e.name != file.name);
    all.push(Evaluated { name: file.name.clone(), size: file.size, modified: file.modified, at, version: EVALUATOR });
    fs::create_dir_all(dir)?;
    fs::write(dir.join(LEDGER), serde_json::to_vec(&all).map_err(io::Error::other)?)
}

/// What a file's name says: `<car name>/<track folder>/<car name>_<preset>_<track folder>_<time>.ld`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameParts {
    pub car_name: String,
    /// Track and layout without spaces, as the game names the folder (`NurburgringTouristenfahrten`)
    pub track_folder: String,
    pub preset: Option<String>,
}

pub fn name_parts(name: &str) -> Option<NameParts> {
    let mut parts = name.split('/');
    let (car_name, track_folder, file) = (parts.next()?, parts.next()?, parts.next()?);
    let preset = file
        .strip_prefix(&format!("{car_name}_"))
        .and_then(|rest| rest.find(&format!("_{track_folder}_")).map(|end| rest[..end].to_string()))
        .filter(|p| !p.is_empty());
    Some(NameParts { car_name: car_name.to_string(), track_folder: track_folder.to_string(), preset })
}

/// Whether a lap of the list was driven on the track of this folder: track and layout without spaces, ignoring
/// case.
pub fn same_track(track: &str, layout: &str, folder: &str) -> bool {
    format!("{track}{layout}").replace(' ', "").eq_ignore_ascii_case(&folder.replace(' ', ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_unl3_NurburgringTouristenfahrten_Sun Oct  4 224023 2026.ld";

    fn file(name: &str, size: u64, modified: u64) -> MotecFile {
        MotecFile { name: name.to_string(), size, modified }
    }

    #[test]
    fn reads_car_track_and_preset_from_the_name() {
        assert_eq!(name_parts(NAME), Some(NameParts {
            car_name: "BMW M3 E46 CSL GT3 Rennsport".into(),
            track_folder: "NurburgringTouristenfahrten".into(),
            preset: Some("preset_dd_bmw_m3_e46_rennsport_unl3".into()),
        }));
        assert_eq!(name_parts("a/b/c.ld").unwrap().preset, None);
        assert_eq!(name_parts("a/b"), None);
        assert!(same_track("Nurburgring", "Touristenfahrten", "NurburgringTouristenfahrten"));
        assert!(same_track("Nurburgring GP", "", "NurburgringGP"));
        assert!(!same_track("Nurburgring", "GP", "NurburgringTouristenfahrten"));
    }

    #[test]
    fn evaluates_each_ended_session_once_by_its_newest_file() {
        let a = |t: &str| format!("Car/Track/Car_preset_p_Track_{t}.ld");
        let files = [file(&a("1"), 1_000_000, 1000), file(&a("2"), 2_000_000, 1500), file(&a("3"), 30_000, 9000)];
        let now = 1500 + QUIET_SECS;
        // the game runs: the session of 1500 has been quiet for 10 minutes, 9000 is a session of its own and running
        let names = |found: Vec<&MotecFile>| found.iter().map(|f| f.modified).collect::<Vec<_>>();
        assert_eq!(names(ready(&files, &[], now - 1, true)), Vec::<u64>::new());
        assert_eq!(names(ready(&files, &[], now, true)), [1500]);
        // the game is closed: every ended session, but not a header-only file
        assert_eq!(names(ready(&files, &[], 9001, false)), [1500]);
        let done = [Evaluated { name: a("2"), size: 2_000_000, modified: 1500, at: 2000, version: EVALUATOR }];
        assert!(ready(&files, &done, now, false).is_empty());
        // evaluated by an older evaluator: once more
        let older = [Evaluated { version: EVALUATOR - 1, ..done[0].clone() }];
        assert_eq!(names(ready(&files, &older, now, false)), [1500]);
        // the session grew by a lap after its evaluation
        let grown = [file(&a("1"), 1_000_000, 1000), file(&a("2"), 2_000_000, 1500), file(&a("4"), 3_000_000, 1900)];
        assert_eq!(names(ready(&grown, &done, 1900 + QUIET_SECS, true)), [1900]);
        // older than 30 days
        assert!(ready(&files, &[], 1500 + MAX_AGE_SECS + 1, false).is_empty());
    }

    #[test]
    fn remembers_what_it_evaluated() {
        let dir = std::env::temp_dir().join(format!("dd-engineer-evaluated-{}", std::process::id()));
        assert!(evaluated(&dir).is_empty());
        record_evaluated(&dir, &file(NAME, 16_007_476, 1_791_158_423), 1_791_160_000).unwrap();
        record_evaluated(&dir, &file(NAME, 16_007_476, 1_791_158_423), 1_791_160_100).unwrap();
        assert_eq!(evaluated(&dir), [Evaluated { name: NAME.into(), size: 16_007_476, modified: 1_791_158_423, at: 1_791_160_100, version: EVALUATOR }]);
        // an entry written before the version existed reads as version 0
        assert_eq!(serde_json::from_str::<Evaluated>(r#"{"name":"a","size":1,"modified":2,"at":3}"#).unwrap().version, 0);
        fs::remove_dir_all(&dir).unwrap();
    }
}
