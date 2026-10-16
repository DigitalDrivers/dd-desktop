//! Evaluating one MoTeC session: its newest file is read, the laps of the app's list inside it are cut out and
//! checked, the track's corner map is made or updated, every lap is measured on it and compared with the reference
//! lap, and the session's report, curves, map and reference are stored. The shell hands over the open file, its
//! name and modification time, the game's logs and the lap list; this does the rest.

use std::io::{self, Read, Seek};
use std::path::Path;

use super::clock::{file_clock, iso_local};
use super::corners::{self, curvature, detect, fit, LapShape, Map};
use super::cut::{grid, windows, Data, Grid};
use super::laps::LocalLap;
use super::ld::Ld;
use super::measure::{measure, report, trace, LapMeasures, CLUB_TARGET_PSI};
use super::sessions::name_parts;
use super::store::{self, id_car, refs_key, RefLap, Refs};
use super::{CornerMap, EngineerSession, Reference, TimingNote};
use crate::laps::is_club_car;

/// A lap's integrated distance may differ this much from the track's.
const DISTANCE: f64 = 0.10;

pub struct Input<'a> {
    /// `<car>/<track>/<file>.ld` below the MoTeC folder
    pub name: &'a str,
    /// The file's modification time, unix ms
    pub modified_ms: i64,
    /// The game's logs, the one that covers the file first
    pub logs: &'a [&'a str],
    pub laps: &'a [LocalLap],
    pub app_version: &'a str,
    pub now_ms: i64,
}

/// What an evaluation did, for the app log and the page.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub id: String,
    pub laps: usize,
    /// Laps of the list in the file that were not evaluated, and why
    pub dropped: Vec<String>,
    /// Whether the file's clock came from the game's log
    pub from_log: bool,
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// A lap cut out of the file.
struct Cut<'a> {
    n: u32,
    lap: &'a LocalLap,
    start: f64,
    end: f64,
    grid: Grid,
    kappa: Vec<f64>,
    found: Vec<corners::Detected>,
}

/// Evaluates the session whose newest file `r` is, into `dir` (the engineer's folder).
pub fn run<R: Read + Seek>(dir: &Path, r: &mut R, input: &Input) -> io::Result<Outcome> {
    let ld = Ld::read(r)?;
    let duration = ld.duration().ok_or_else(|| invalid("the file has no SPEED"))?;
    let written = ld.written_local().ok_or_else(|| invalid("the file's header names no time"))?;
    let parts = name_parts(input.name).ok_or_else(|| invalid("not <car>/<track>/<file>.ld"))?;
    let clock = file_clock(written, input.modified_ms, duration, input.logs);
    let found = windows(input.laps, &parts, clock.start_ms, duration);
    let first = found.first().map(|w| w.lap);
    let car = first.map_or_else(|| id_car(&parts.car_name), |lap| lap.car.clone());
    let preset = first.map_or(parts.preset.clone(), |lap| lap.preset.clone());
    let (track, layout) = first.map_or((parts.track_folder.clone(), String::new()), |lap| (lap.track.clone(), lap.layout.clone()));
    let id = format!("{}-{}", clock.start_ms.div_euclid(1000), id_car(&car));
    let mut dropped = Vec::new();

    // cut the laps out and check them
    let data = if found.is_empty() { None } else { Some(Data::read(&ld, r)?) };
    let mut cuts: Vec<Cut> = Vec::new();
    for w in &found {
        match grid(data.as_ref().unwrap(), w.start, w.end) {
            Ok(g) => {
                let kappa = curvature(&g.speed, &g.yaw);
                let corners_found = detect(&kappa);
                cuts.push(Cut { n: w.n, lap: w.lap, start: w.start, end: w.end, grid: g, kappa, found: corners_found });
            }
            Err(reason) => dropped.push(format!("lap {}: {reason}", w.n)),
        }
    }
    let old = corners::load(dir, &track, &layout);
    let expected = old.as_ref().map(|m| m.length_m).or_else(|| {
        let mut lengths: Vec<f64> = cuts.iter().map(|c| c.grid.length()).collect();
        lengths.sort_by(f64::total_cmp);
        lengths.get(lengths.len() / 2).copied()
    });
    cuts.retain(|c| {
        let ok = expected.is_none_or(|len| (c.grid.length() - len).abs() <= DISTANCE * len);
        if !ok {
            dropped.push(format!("lap {}: {:.0} m, the track is {:.0} m", c.n, c.grid.length(), expected.unwrap_or(0.0)));
        }
        ok
    });

    // the corner map, from the best valid lap (or the best lap)
    let best = (0..cuts.len()).min_by_key(|i| (!cuts[*i].lap.valid, cuts[*i].lap.lap_time_ms));
    let ids: Vec<String> = cuts.iter().map(|c| format!("{id}#{}", c.n)).collect();
    let shapes: Vec<LapShape> = cuts.iter().zip(&ids).map(|(c, lap_id)| LapShape { id: lap_id, found: &c.found, speed: &c.grid.speed }).collect();
    let map: Option<Map> = best.and_then(|best| corners::update(old.clone(), &shapes, best));
    let mut measured: Vec<(&Cut, String, LapMeasures)> = Vec::new();
    if let Some(map) = &map {
        for (c, lap_id) in cuts.iter().zip(&ids) {
            let f = fit(&c.found, map);
            if map.usable(&c.found, &f) {
                measured.push((c, lap_id.clone(), measure(&c.grid, &c.kappa, data.as_ref().unwrap(), c.start, c.end, map, &f)));
            } else {
                dropped.push(format!("lap {}: {} of its corners meet the track's {}", c.n, f.matched, map.corners.len()));
            }
        }
    }

    // the reference and the best segments of this car, preset, track and layout on this map version
    let key = refs_key(&car, preset.as_deref(), &track, &layout);
    let mut refs = store::load_refs(dir, &key).unwrap_or_default();
    if let Some(map) = &map {
        if refs.map_version != map.version {
            refs = Refs { map_version: map.version, ..Refs::default() };
        }
        for (c, lap_id, m) in measured.iter().filter(|(c, _, _)| c.lap.valid) {
            if refs.reference.as_ref().is_none_or(|r| c.lap.lap_time_ms < r.lap_time_ms || r.session_id == id && r.lap_id == *lap_id) {
                refs.reference = Some(RefLap { session_id: id.clone(), lap_id: lap_id.clone(), lap_time_ms: c.lap.lap_time_ms, measures: m.clone() });
            }
            let segments: Vec<f64> = m.corners.iter().map(|c| c.segment_s).collect();
            refs.best_segments_s = if refs.best_segments_s.len() == segments.len() {
                refs.best_segments_s.iter().zip(&segments).map(|(a, b)| a.min(*b)).collect()
            } else {
                segments
            };
        }
    }
    let reference = refs.reference.as_ref().filter(|_| map.is_some());
    let theoretical_best = (!refs.best_segments_s.is_empty()).then(|| (refs.best_segments_s.iter().sum::<f64>() * 1000.0).round() as i64);
    let target = is_club_car(&car).then_some(CLUB_TARGET_PSI);
    let laps = measured.iter().map(|(c, lap_id, m)| report(lap_id, c.n, c.lap, m, reference.map(|r| &r.measures), theoretical_best, target.clone())).collect::<Vec<_>>();
    let traces = measured.iter().map(|(c, lap_id, _)| trace(lap_id, &c.grid)).collect::<Vec<_>>();

    let touristenfahrten = layout.to_lowercase().contains("touristenfahrt") || track.to_lowercase().contains("touristenfahrt");
    let session = EngineerSession {
        id: id.clone(),
        car: car.clone(),
        preset: preset.clone(),
        car_name: parts.car_name.clone(),
        track: track.clone(),
        layout: layout.clone(),
        started_at: iso_local(clock.start_ms, clock.offset_s),
        ended_at: iso_local(clock.start_ms + (duration * 1000.0).round() as i64, clock.offset_s),
        app_version: input.app_version.to_string(),
        reference: reference.map(|r| Reference { session_id: r.session_id.clone(), lap_id: r.lap_id.clone(), lap_time_ms: r.lap_time_ms }),
        corners: map.as_ref().map_or(CornerMap { version: 0, frozen: false, corners: Vec::new() }, Map::contract),
        timing_note: if touristenfahrten || laps.is_empty() { TimingNote::PartialWindow } else { TimingNote::FullLap },
        laps,
    };
    store::save_session(dir, &session, &traces)?;
    if let Some(map) = &map {
        if old.as_ref() != Some(map) {
            corners::save(dir, &track, &layout, map)?;
        }
        store::save_refs(dir, &key, &refs)?;
    }
    store::apply_retention(dir, input.now_ms)?;
    Ok(Outcome { id, laps: session.laps.len(), dropped, from_log: clock.from_log })
}

#[cfg(test)]
mod tests {
    use super::super::clock::days_from_civil;
    use super::super::testkit::{self, Drive};
    use super::super::Finding;
    use super::*;
    use std::io::Cursor;

    const NAME: &str = "Test Car/TestRingGP/Test Car_preset_dd_test_car_stage1_TestRingGP_Sun Oct  4 224023 2026.ld";

    /// A session's file written at 23:40:23 CEST on 2026-10-04, 30 s after the last lap completion the log names,
    /// and the app's lap list for it; `day` moves it by days.
    fn session(drive: &Drive, day: i64) -> (Vec<u8>, String, Vec<LocalLap>, i64, i64) {
        let s = testkit::session(drive);
        let written_local = (days_from_civil(2026, 10, 4 + day) * 86_400) + 23 * 3600 + 40 * 60 + 23;
        let date = format!("{:02}/10/2026", 4 + day);
        let bytes = testkit::write_ld(&date, "23:40:23", &s.chans);
        let end_local_ms = (written_local - 30) * 1000;
        let modified_ms = (written_local - 7200) * 1000 + 400;
        let start_ms = end_local_ms - 7_200_000 - (s.duration * 1000.0).round() as i64;
        let secs = end_local_ms / 1000;
        let log = format!(
            "[2026-10-{:02} {:02}:{:02}:{:02}.{:03}] [physics] [info] Lap test evOnLapCompleted 3 completed\n",
            4 + day, secs % 86_400 / 3600, secs % 3600 / 60, secs % 60, end_local_ms % 1000
        );
        (bytes, log, testkit::local_laps(&s, start_ms), modified_ms, start_ms)
    }

    fn evaluate(dir: &Path, drive: &Drive, day: i64) -> (Outcome, i64) {
        let (bytes, log, laps, modified_ms, start_ms) = session(drive, day);
        let input = Input { name: NAME, modified_ms, logs: &[&log], laps: &laps, app_version: "0.25.0", now_ms: modified_ms };
        (run(dir, &mut Cursor::new(bytes), &input).unwrap(), start_ms)
    }

    fn temp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("dd-engineer-run-{tag}-{}", std::process::id()))
    }

    #[test]
    fn evaluates_a_session_against_its_best_lap() {
        let dir = temp("one");
        let (outcome, start_ms) = evaluate(&dir, &Drive { late: Some(2), lock: false }, 0);
        assert!(outcome.from_log && outcome.dropped.is_empty(), "{outcome:?}");
        assert_eq!((outcome.id.clone(), outcome.laps), (format!("{}-dd_test_car", start_ms / 1000), 2));
        let s = store::session(&dir, &outcome.id).unwrap();
        assert_eq!((s.car.as_str(), s.preset.as_deref(), s.car_name.as_str(), s.track.as_str(), s.layout.as_str()), ("dd_test_car", Some("preset_dd_test_car_stage1"), "Test Car", "Test Ring", "GP"));
        assert_eq!((s.started_at.as_str(), s.timing_note), ("2026-10-04T23:36:23+02:00", TimingNote::FullLap));
        assert_eq!((s.corners.version, s.corners.frozen, s.corners.corners.len()), (1, false, 4));
        // the second lap brakes later for the hairpin and is faster: it is the reference
        let reference = s.reference.clone().unwrap();
        assert_eq!((reference.lap_id.as_str(), reference.lap_time_ms), (s.laps[1].id.as_str(), s.laps[1].lap_time_ms.unwrap()));
        assert!(s.laps[0].corners[2].dt_s.unwrap() > 0.2 && s.laps[1].corners[2].dt_s == Some(0.0));
        assert!(s.laps[0].findings.contains(&Finding::BrakeEarlier));
        // the theoretical best is the sum of both laps' best segments: the reference's lap time here, less what the
        // 20 Hz yaw rate leaves between equal segments (up to 0.02 s each)
        let best = s.laps[0].theoretical_best_ms.unwrap();
        assert!(best <= reference.lap_time_ms && reference.lap_time_ms - best <= 60, "{best} against {}", reference.lap_time_ms);
        // the test car's id starts with dd_: a club car, judged by the club's pressure window
        assert_eq!(s.laps[0].tyres.target_psi, Some(CLUB_TARGET_PSI));
        assert_eq!(store::traces(&dir, &outcome.id).unwrap().len(), 2);
        assert!(corners::load(&dir, "Test Ring", "GP").is_some());
        store::forget(&dir).unwrap();
    }

    #[test]
    fn freezes_the_map_with_the_third_lap_and_keeps_the_reference_across_sessions() {
        let dir = temp("two");
        let (first, _) = evaluate(&dir, &Drive { late: Some(2), lock: false }, 0);
        let (second, _) = evaluate(&dir, &Drive { late: None, lock: true }, 1);
        let s = store::session(&dir, &second.id).unwrap();
        assert!(s.corners.frozen, "four laps agreed");
        // the reference stays the first session's faster lap
        assert_eq!(s.reference.unwrap().session_id, first.id);
        assert!(s.laps.iter().all(|l| l.corners[2].dt_s.unwrap() > 0.2));
        // evaluating the first session again changes nothing of the map
        let map = corners::load(&dir, "Test Ring", "GP").unwrap();
        evaluate(&dir, &Drive { late: Some(2), lock: false }, 0);
        assert_eq!(corners::load(&dir, "Test Ring", "GP").unwrap(), map);
        store::forget(&dir).unwrap();
    }

    #[test]
    fn says_why_a_lap_is_not_evaluated_and_keeps_a_session_without_laps() {
        let dir = temp("three");
        let (bytes, log, mut laps, modified_ms, _) = session(&Drive { late: None, lock: false }, 0);
        // the app's clock was 30 s off for the first lap: its window holds the out lap's standstill... or not a lap
        laps[0].end_ms -= 30_000;
        let input = Input { name: NAME, modified_ms, logs: &[&log], laps: &laps, app_version: "0.25.0", now_ms: modified_ms };
        let outcome = run(&dir, &mut Cursor::new(bytes.clone()), &input).unwrap();
        assert_eq!(outcome.laps, 1);
        assert!(outcome.dropped[0].starts_with("lap 1: "), "{:?}", outcome.dropped);
        // the switch was off: no laps of the list, a session that says so
        let none = Input { laps: &[], ..input };
        let outcome = run(&dir, &mut Cursor::new(bytes), &none).unwrap();
        let s = store::session(&dir, &outcome.id).unwrap();
        assert_eq!((s.laps.len(), s.timing_note, s.car.as_str(), s.track.as_str()), (0, TimingNote::PartialWindow, "Test_Car", "TestRingGP"));
        assert!(s.reference.is_none() && s.corners.corners.is_empty());
        store::forget(&dir).unwrap();
    }
}
