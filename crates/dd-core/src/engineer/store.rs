//! What the engineer keeps in `<config>/engineer`: the lap list (`laps.jsonl`), what it evaluated
//! (`evaluated.json`), the corner maps (`maps/`), per car, preset, track and layout the reference lap's numbers
//! and the best segment times (`refs/`), and per session its report and curves (`sessions/<id>.json` and
//! `<id>.traces.json`). Sessions stay as long as the driver chose (30 days until they do, or for ever), a
//! reference lap's session for good, one without laps 7 days; above 200 MB the oldest go first. A session can be
//! deleted on its own; forgetting deletes all of it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::corners::file_key;
use super::measure::LapMeasures;
use super::{laps, sessions, EngineerSession, EngineerTrace};

/// The engineer's folder in the app's config folder.
pub const DIR: &str = "engineer";
const SESSIONS: &str = "sessions";
const REFS: &str = "refs";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// A session without laps stays this long: the laps may still come from the beacons or the log once more.
const KEEP_EMPTY_MS: i64 = 7 * DAY_MS;
const CAP_BYTES: u64 = 200 * 1024 * 1024;

/// The reference lap of a car, preset, track and layout: the driver's best valid lap on this map version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefLap {
    pub session_id: String,
    pub lap_id: String,
    pub lap_time_ms: i64,
    pub measures: LapMeasures,
}

/// The reference lap and the best time of each segment over every valid lap evaluated on this map version.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Refs {
    pub map_version: u32,
    pub reference: Option<RefLap>,
    pub best_segments_s: Vec<f64>,
}

pub fn refs_key(car: &str, preset: Option<&str>, track: &str, layout: &str) -> String {
    file_key(&[car, preset.unwrap_or(""), track, layout])
}

pub fn load_refs(dir: &Path, key: &str) -> Option<Refs> {
    fs::read(dir.join(REFS).join(format!("{key}.json"))).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

pub fn save_refs(dir: &Path, key: &str, refs: &Refs) -> io::Result<()> {
    fs::create_dir_all(dir.join(REFS))?;
    fs::write(dir.join(REFS).join(format!("{key}.json")), serde_json::to_vec(refs).map_err(io::Error::other)?)
}

/// Whether `id` is a session id (`<unix seconds>-<car>`), so it is a plain file name.
pub fn is_session_id(id: &str) -> bool {
    let Some((secs, car)) = id.split_once('-') else { return false };
    id.len() <= 100
        && !secs.is_empty()
        && secs.chars().all(|c| c.is_ascii_digit())
        && !car.is_empty()
        && car.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        && !car.contains("..")
}

/// A car for a session id: the car's id, with anything but letters, digits, `_`, `.` and `-` as `_`.
pub fn id_car(car: &str) -> String {
    car.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '_' }).collect::<String>().replace("..", "__")
}

/// The id of a session: `<start in unix seconds>-<car>`, or the id of a stored session of the car that started at
/// most 2 s apart. Every file of a session gives the same start within 15 ms (see [`super::clock`]), but that can
/// still fall on either side of a second.
pub fn session_id(dir: &Path, start_ms: i64, car: &str) -> String {
    let secs = start_ms.div_euclid(1000);
    let car = id_car(car);
    stored(dir)
        .into_iter()
        .map(|(id, start, _)| (id, start / 1000))
        .find(|(id, start)| (start - secs).abs() <= 2 && id.split_once('-').is_some_and(|(_, c)| c == car))
        .map_or_else(|| format!("{secs}-{car}"), |(id, _)| id)
}

fn session_path(dir: &Path, id: &str, suffix: &str) -> PathBuf {
    dir.join(SESSIONS).join(format!("{id}{suffix}"))
}

pub fn save_session(dir: &Path, session: &EngineerSession, traces: &[EngineerTrace]) -> io::Result<()> {
    fs::create_dir_all(dir.join(SESSIONS))?;
    fs::write(session_path(dir, &session.id, ".traces.json"), serde_json::to_vec(traces).map_err(io::Error::other)?)?;
    fs::write(session_path(dir, &session.id, ".json"), serde_json::to_vec(session).map_err(io::Error::other)?)
}

pub fn session(dir: &Path, id: &str) -> Option<EngineerSession> {
    is_session_id(id).then_some(())?;
    fs::read(session_path(dir, id, ".json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

pub fn traces(dir: &Path, id: &str) -> Option<Vec<EngineerTrace>> {
    is_session_id(id).then_some(())?;
    fs::read(session_path(dir, id, ".traces.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// The ids of the stored sessions with their start (unix ms, from the id) and size in bytes.
fn stored(dir: &Path) -> Vec<(String, i64, u64)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir.join(SESSIONS)).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".json").filter(|id| !id.ends_with(".traces") && is_session_id(id)) else { continue };
        let size = |suffix: &str| fs::metadata(session_path(dir, id, suffix)).map_or(0, |m| m.len());
        let start = id.split_once('-').and_then(|(secs, _)| secs.parse::<i64>().ok()).unwrap_or(0) * 1000;
        out.push((id.to_string(), start, size(".json") + size(".traces.json")));
    }
    out
}

/// What the page lists of a session.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHead {
    pub id: String,
    pub car: String,
    pub car_name: String,
    pub preset: Option<String>,
    pub track: String,
    pub layout: String,
    pub started_at: String,
    pub ended_at: String,
    pub laps: u32,
    pub best_lap_ms: Option<i64>,
}

/// The stored sessions, the newest first.
pub fn heads(dir: &Path) -> Vec<SessionHead> {
    let mut all = stored(dir);
    all.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    all.into_iter()
        .filter_map(|(id, _, _)| session(dir, &id))
        .map(|s| SessionHead {
            best_lap_ms: s.laps.iter().filter(|l| l.valid).filter_map(|l| l.lap_time_ms).min(),
            laps: s.laps.len() as u32,
            id: s.id,
            car: s.car,
            car_name: s.car_name,
            preset: s.preset,
            track: s.track,
            layout: s.layout,
            started_at: s.started_at,
            ended_at: s.ended_at,
        })
        .collect()
}

/// The sessions to delete: older than `keep_days` (None: never for that), one without laps older than 7 days, then
/// the oldest while all together are above 200 MB; never one in `keep` (the reference laps' sessions). `sessions`
/// are (id, start in unix ms, bytes, whether it has laps).
pub fn retention(sessions: &[(String, i64, u64, bool)], keep: &[String], now_ms: i64, keep_days: Option<u32>) -> Vec<String> {
    let mut by_age: Vec<&(String, i64, u64, bool)> = sessions.iter().collect();
    by_age.sort_by_key(|s| (s.1, s.0.clone()));
    let mut total: u64 = sessions.iter().map(|s| s.2).sum();
    let mut out = Vec::new();
    for (id, start, bytes, has_laps) in by_age {
        if keep.contains(id) {
            continue;
        }
        let age = now_ms - start;
        let old = keep_days.is_some_and(|days| age > days as i64 * DAY_MS) || (!has_laps && age > KEEP_EMPTY_MS);
        if old || total > CAP_BYTES {
            out.push(id.clone());
            total -= bytes;
        }
    }
    out
}

/// Deletes a session's report and curves; nothing when there is none of that id. The references stay.
pub fn forget_session(dir: &Path, id: &str) -> io::Result<()> {
    if !is_session_id(id) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a session id"));
    }
    for suffix in [".json", ".traces.json"] {
        match fs::remove_file(session_path(dir, id, suffix)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// Deletes what [`retention`] says; the ids deleted.
pub fn apply_retention(dir: &Path, now_ms: i64, keep_days: Option<u32>) -> io::Result<Vec<String>> {
    let keep: Vec<String> = fs::read_dir(dir.join(REFS))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| fs::read(e.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<Refs>(&bytes).ok())
        .filter_map(|refs| refs.reference.map(|r| r.session_id))
        .collect();
    let sessions: Vec<(String, i64, u64, bool)> = stored(dir)
        .into_iter()
        .map(|(id, start, bytes)| {
            let has_laps = session(dir, &id).is_some_and(|s| !s.laps.is_empty());
            (id, start, bytes, has_laps)
        })
        .collect();
    let gone = retention(&sessions, &keep, now_ms, keep_days);
    for id in &gone {
        forget_session(dir, id)?;
    }
    Ok(gone)
}

/// Deletes everything of the engineer.
pub fn forget(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The numbers of `engineer_status` that come from the folder: laps in the list, stored sessions, and when the last
/// session was evaluated (seconds since 1970).
pub fn counts(dir: &Path) -> (u32, u32, Option<u64>) {
    (laps::load(dir).len() as u32, stored(dir).len() as u32, sessions::evaluated(dir).iter().map(|e| e.at).max())
}

#[cfg(test)]
mod tests {
    use super::super::{CornerMap, TimingNote};
    use super::*;

    const DAY: i64 = 24 * 60 * 60 * 1000;
    const MB: u64 = 1024 * 1024;

    #[test]
    fn keeps_the_chosen_days_and_200_mb_and_every_reference_session() {
        let now = 100 * DAY;
        let s = |id: &str, age_days: i64, mb: u64| (id.to_string(), now - age_days * DAY, mb * MB, true);
        let all = [s("a", 40, 1), s("b", 31, 1), s("c", 29, 1), s("d", 1, 1)];
        assert_eq!(retention(&all, &[], now, Some(30)), ["a", "b"]);
        assert_eq!(retention(&all, &["a".to_string()], now, Some(30)), ["b"], "a reference lap's session stays for good");
        assert_eq!(retention(&all, &[], now, Some(90)), Vec::<String>::new());
        assert_eq!(retention(&all, &[], now, None), Vec::<String>::new(), "for ever");
        // above 200 MB the oldest go first, until it fits, whatever the days
        let big = [s("a", 5, 120), s("b", 4, 50), s("c", 3, 60), s("d", 2, 10)];
        assert_eq!(retention(&big, &[], now, None), ["a"]);
        assert_eq!(retention(&big, &["a".to_string()], now, Some(30)), ["b"]);
        assert!(retention(&[s("a", 0, 10)], &[], now, Some(30)).is_empty());
        // a session without laps goes after 7 days, whatever the days
        let empty = |id: &str, age_days: i64| (id.to_string(), now - age_days * DAY, MB, false);
        assert_eq!(retention(&[empty("a", 8), empty("b", 6)], &[], now, None), ["a"]);
    }

    #[test]
    fn takes_only_plain_session_ids() {
        assert!(is_session_id("1791062547-dd_bmw_m3_e46_rennsport"));
        assert!(is_session_id("1791062547-ks_porsche-911.gt3"));
        for id in ["", "dd", "-dd", "12-", "12/x-dd", "12-../x", "12-a/b", "12-a\\b", "x1-dd", "12-a..b"] {
            assert!(!is_session_id(id), "{id}");
        }
        assert_eq!(id_car("BMW M3 E46 CSL"), "BMW_M3_E46_CSL");
        assert!(is_session_id(&format!("1-{}", id_car("a/../b c"))));
    }

    fn session(id: &str, laps: usize) -> EngineerSession {
        EngineerSession {
            id: id.into(),
            car: "dd_x".into(),
            preset: None,
            car_name: "X".into(),
            track: "T".into(),
            layout: "L".into(),
            started_at: "2026-10-04T23:22:27+02:00".into(),
            ended_at: "2026-10-04T23:39:16+02:00".into(),
            app_version: "0.25.0".into(),
            reference: None,
            corners: CornerMap { version: 1, frozen: false, corners: vec![] },
            laps: (0..laps).map(|_| serde_json::from_str(include_str!("../../tests/fixtures/example-session.json")).map(|s: EngineerSession| s.laps[0].clone()).unwrap()).collect(),
            timing_note: TimingNote::FullLap,
        }
    }

    #[test]
    fn stores_lists_and_forgets_sessions() {
        let dir = std::env::temp_dir().join(format!("dd-engineer-store-{}", std::process::id())).join(DIR);
        let (old, new) = (session("1791062547-dd_x", 1), session("1791999999-dd_x", 2));
        save_session(&dir, &old, &[]).unwrap();
        save_session(&dir, &new, &[EngineerTrace { lap_id: "1791999999-dd_x#1".into(), step_m: 5.0, speed_kmh: vec![1.0], throttle: vec![0.0], brake: vec![0.0], steer_deg: vec![0.0], gear: vec![1] }]).unwrap();
        assert_eq!(super::session(&dir, &old.id), Some(old.clone()));
        assert_eq!(traces(&dir, &new.id).unwrap().len(), 1);
        assert_eq!(super::session(&dir, "../x"), None);
        let listed = heads(&dir);
        assert_eq!(listed.iter().map(|h| (h.id.as_str(), h.laps)).collect::<Vec<_>>(), [("1791999999-dd_x", 2), ("1791062547-dd_x", 1)]);
        assert_eq!(listed[0].best_lap_ms, new.laps[0].lap_time_ms);
        assert_eq!(counts(&dir), (0, 2, None));
        // a later file of the session whose start falls on the next second keeps the session's id
        assert_eq!(session_id(&dir, 1_791_062_548_004, "dd_x"), old.id);
        assert_eq!(session_id(&dir, 1_791_062_550_004, "dd_x"), "1791062550-dd_x");
        assert_eq!(session_id(&dir, 1_791_062_548_004, "dd_y"), "1791062548-dd_y");
        // retention keeps the reference's session, deletes the other old one
        let refs = Refs { map_version: 1, reference: Some(RefLap { session_id: old.id.clone(), lap_id: format!("{}#1", old.id), lap_time_ms: 1, measures: serde_json::from_str(REF_MEASURES).unwrap() }), best_segments_s: vec![] };
        save_refs(&dir, &refs_key("dd_x", None, "T", "L"), &refs).unwrap();
        assert_eq!(load_refs(&dir, "dd_x____T__L"), Some(refs));
        assert!(apply_retention(&dir, 1_791_999_999_000 + 31 * DAY, Some(30)).unwrap() == ["1791999999-dd_x"]);
        assert_eq!(counts(&dir).1, 1);
        // one session on its own; a wrong id is refused, a missing one is nothing
        save_session(&dir, &new, &[]).unwrap();
        forget_session(&dir, &new.id).unwrap();
        assert!(super::session(&dir, &new.id).is_none() && super::session(&dir, &old.id).is_some());
        assert!(forget_session(&dir, "../x").is_err() && forget_session(&dir, &new.id).is_ok());
        forget(&dir).unwrap();
        assert!(!dir.exists());
        forget(&dir).unwrap();
    }

    const REF_MEASURES: &str = r#"{"distanceM":1.0,"topSpeedKmh":1.0,"fuel":{"startL":1.0,"usedL":1.0},"tyres":{"pressureStart":[0,0,0,0],"pressureAvg":[0,0,0,0],"pressureEnd":[0,0,0,0],"surfaceTempAvg":[0,0,0,0],"airTempAvg":[0,0,0,0],"targetPsi":null},"brakes":{"tempMax":[0,0,0,0],"secondsAbove700":0,"frontShare":0},"fade":null,"bands":[[null,0],[null,0],[null,0],[null,0]],"traction":[],"corners":[]}"#;
}
