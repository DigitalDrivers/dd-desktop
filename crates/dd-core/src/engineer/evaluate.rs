//! Evaluating one MoTeC session: its newest file is read, the laps inside it are cut out and checked (from the
//! app's lap list, else from the game's log, else from the `.ldx` beacons next to the file), the track's corner map
//! is made or updated, every lap is measured on it and compared with the reference lap, and the session's report,
//! curves, map and reference are stored, with the coach's digest of it ([`super::digest`]). The shell hands over
//! the open file, its name and modification time, the game's logs, the `.ldx` and the lap list; this does the rest.

use std::io::{self, Read, Seek};
use std::path::Path;

use super::clock::{file_clock, iso_local, FileClock};
use super::corners::{self, curvature, detect, fit, fit_within, LapShape, Map};
use super::cut::{grid, grid_in, windows, Data, Grid};
use super::laps::LocalLap;
use super::ld::Ld;
use super::measure::{measure, report, target_psi, trace, LapMeasures};
use super::sessions::{name_parts, same_track, NameParts};
use super::store::{self, id_car, refs_key, RefLap, Refs};
use super::{beacons, digest, gamelog, CornerMap, EngineerSession, Reference, TargetPsi, TimeSource, TimingNote};

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
    /// The `.ldx` next to the file, when there is one
    pub ldx: Option<&'a str>,
    pub app_version: &'a str,
    pub now_ms: i64,
    /// How long sessions stay (the driver's setting), None for ever
    pub keep_days: Option<u32>,
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
    /// Whether the laps came from the game's log (the app's list had none in the file)
    pub laps_from_log: bool,
    /// Whether the laps came from the `.ldx` beacons (neither the list nor the log had any)
    pub laps_from_beacons: bool,
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
    store::upgrade(dir)?;
    let ld = Ld::read(r)?;
    let duration = ld.duration().ok_or_else(|| invalid("the file has no SPEED"))?;
    let written = ld.written_local().ok_or_else(|| invalid("the file's header names no time"))?;
    let parts = name_parts(input.name).ok_or_else(|| invalid("not <car>/<track>/<file>.ld"))?;
    let clock = file_clock(written, input.modified_ms, duration, input.logs);
    let mut dropped = Vec::new();
    let listed = windows(input.laps, &parts, clock.start_ms, duration);
    // without laps of the app's list (driven before its switch existed, or with it off), the game's log names them
    let logged = if listed.is_empty() { laps_from_log(&parts, &clock, duration, clock.log.map(|i| input.logs[i])) } else { Vec::new() };
    let mut source = if !listed.is_empty() { Some(TimeSource::App) } else if !logged.is_empty() { Some(TimeSource::Log) } else { None };
    // without the log too (it is gone after a few days), the file's beacons: on Touristenfahrten the loops from
    // pit exit to pit exit, in which the timed lap is found on the corner map
    let mut beacon_laps = if source.is_none() { laps_from_beacons(dir, input.laps, &parts, &clock, input.ldx) } else { Vec::new() };
    let mut data: Option<Data> = None;
    if let Some(first) = beacon_laps.first() {
        source = Some(TimeSource::Beacon);
        if touristenfahrten(&first.track, &first.layout) {
            source = Some(TimeSource::Telemetry);
            data = Some(Data::read(&ld, r)?);
            beacon_laps = match corners::load(dir, &first.track, &first.layout) {
                Some(map) => timed_laps(data.as_ref().unwrap(), &beacon_laps, &parts, &clock, duration, &map, &mut dropped),
                None => {
                    dropped.push("no corner map of this track yet to find the timed laps in the loops".to_string());
                    Vec::new()
                }
            };
        }
    }
    let found = match source {
        Some(TimeSource::App) => listed,
        Some(TimeSource::Log) => windows(&logged, &parts, clock.start_ms, duration),
        Some(_) => windows(&beacon_laps, &parts, clock.start_ms, duration),
        None => Vec::new(),
    };
    let first = found.first().map(|w| w.lap);
    let car = first.map_or_else(|| id_car(&parts.car_name), |lap| lap.car.clone());
    let preset = first.map_or(parts.preset.clone(), |lap| lap.preset.clone());
    let (track, layout) = first.map_or((parts.track_folder.clone(), String::new()), |lap| (lap.track.clone(), lap.layout.clone()));
    let id = store::session_id(dir, clock.start_ms, &car);

    // cut the laps out and check them
    let data = if found.is_empty() { None } else { Some(match data { Some(d) => d, None => Data::read(&ld, r)? }) };
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

    // a lap a session that started earlier holds already (overlapping files of one drive) counts there only
    let earlier = store::earlier_overlapping(dir, &id, clock.start_ms);
    measured.retain(|(c, _, m)| match earlier.iter().find(|s| store::holds(s, c.lap.lap_time_ms, &m.fuel)) {
        Some(s) => {
            dropped.push(format!("lap {}: already in session {}", c.n, s.id));
            false
        }
        None => true,
    });

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
        // the driver's own spread: the valid laps within 2 % of the reference
        if let Some(best) = refs.reference.as_ref().map(|r| r.lap_time_ms) {
            for (_, lap_id, m) in measured.iter().filter(|(c, _, _)| c.lap.valid && c.lap.lap_time_ms as f64 <= best as f64 * (1.0 + store::PACE)) {
                refs.pace.add(lap_id, m);
            }
        }
    }
    let reference = refs.reference.as_ref().filter(|_| map.is_some());
    let theoretical_best = (!refs.best_segments_s.is_empty()).then(|| (refs.best_segments_s.iter().sum::<f64>() * 1000.0).round() as i64);
    let log = clock.log.map(|i| input.logs[i]);
    let tf = touristenfahrten(&track, &layout);
    let laps = measured
        .iter()
        .map(|(c, lap_id, m)| report(lap_id, c.n, c.lap, source.unwrap_or_default(), m, reference.map(|r| &r.measures), &refs.pace.spread(lap_id, m), theoretical_best, pressure_target(&car, c.lap, log, clock.offset_s, tf)))
        .collect::<Vec<_>>();
    let traces = measured.iter().map(|(c, lap_id, _)| trace(lap_id, &c.grid)).collect::<Vec<_>>();

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
        timing_note: if touristenfahrten(&track, &layout) || laps.is_empty() || source != Some(TimeSource::App) { TimingNote::PartialWindow } else { TimingNote::FullLap },
        laps,
    };
    store::save_session(dir, &session, &traces)?;
    digest::record(dir, &session, input.now_ms)?;
    if let Some(map) = &map {
        if old.as_ref() != Some(map) {
            corners::save(dir, &track, &layout, map)?;
        }
        store::save_refs(dir, &key, &refs)?;
    }
    store::apply_retention(dir, input.now_ms, input.keep_days)?;
    Ok(Outcome {
        id,
        laps: session.laps.len(),
        dropped,
        from_log: clock.from_log,
        laps_from_log: source == Some(TimeSource::Log),
        laps_from_beacons: matches!(source, Some(TimeSource::Beacon | TimeSource::Telemetry)),
    })
}

/// Touristenfahrten: the recording starts at the pit exit, the timed lap at the start split.
fn touristenfahrten(track: &str, layout: &str) -> bool {
    layout.to_lowercase().contains("touristenfahrt") || track.to_lowercase().contains("touristenfahrt")
}

/// The hot pressure window a lap is judged by: that of the compound the game's log names as fitted when the lap
/// started ([`gamelog::compound`], [`target_psi`]). None, no judgement, without the log or a compound in it, for a car
/// or compound the table does not list, and on a circuit for the run's first lap (out of the pit box, the tyres
/// still cold) and a lap through the pit lane. On Touristenfahrten every stint's timed lap starts at the pit exit
/// and ends 20 km later with hot tyres; tlm.py judges the stint's end there too.
fn pressure_target(car: &str, lap: &LocalLap, log: Option<&str>, offset_s: i64, touristenfahrten: bool) -> Option<TargetPsi> {
    if !touristenfahrten && (lap.pit || lap.start_ms() <= lap.session_start_ms) {
        return None;
    }
    target_psi(car, &gamelog::compound(log?, lap.start_ms() + offset_s * 1000)?)
}

/// The laps between the beacons of the file's `.ldx` (see [`beacons`]), as the app's lap list would have them. The
/// car, preset, track and layout come from the lap list's or a stored session's entry of the file's track and
/// preset (so the laps meet their corner map and reference), else from the file's name. Like the log's laps, every
/// one counts as valid and none as a pit lap.
fn laps_from_beacons(dir: &Path, laps: &[LocalLap], parts: &NameParts, clock: &FileClock, ldx: Option<&str>) -> Vec<LocalLap> {
    let markers = ldx.map(beacons::markers).unwrap_or_default();
    if markers.len() < 2 {
        return Vec::new();
    }
    let same = |track: &str, layout: &str, preset: &Option<String>| same_track(track, layout, &parts.track_folder) && (parts.preset.is_none() || *preset == parts.preset);
    let (car, preset, track, layout) = laps
        .iter()
        .rev()
        .find(|l| same(&l.track, &l.layout, &l.preset))
        .map(|l| (l.car.clone(), l.preset.clone(), l.track.clone(), l.layout.clone()))
        .or_else(|| store::heads(dir).into_iter().find(|h| same(&h.track, &h.layout, &h.preset)).map(|h| (h.car, h.preset, h.track, h.layout)))
        .unwrap_or_else(|| (id_car(&parts.car_name), parts.preset.clone(), parts.track_folder.clone(), String::new()));
    let at = |s: f64| clock.start_ms + (s * 1000.0).round() as i64;
    markers
        .windows(2)
        .map(|w| LocalLap {
            end_ms: at(w[1]),
            lap_time_ms: ((w[1] - w[0]) * 1000.0).round() as i64,
            valid: true,
            pit: false,
            car: car.clone(),
            preset: preset.clone(),
            track: track.clone(),
            layout: layout.clone(),
            session_start_ms: at(markers[0]),
        })
        .collect()
}

/// The timed laps inside Touristenfahrten loops (pit exit to pit exit): each loop is put on the track's corner map
/// with the shift the loop's extra length allows, and the stretch from the map's start to its end is the lap, its
/// time read from the telemetry (the grid is per metre, so to about 0.1 s). A loop the map does not fit, or that
/// does not hold the whole timed lap, is dropped. The car may stand in the loop (the parking lot); the timed lap
/// is checked for that like every lap, when it is cut.
fn timed_laps(data: &Data, loops: &[LocalLap], parts: &NameParts, clock: &FileClock, duration: f64, map: &Map, dropped: &mut Vec<String>) -> Vec<LocalLap> {
    let mut out = Vec::new();
    for w in windows(loops, parts, clock.start_ms, duration) {
        let g = match grid_in(data, w.start, w.end, false) {
            Ok(g) => g,
            Err(reason) => {
                dropped.push(format!("loop {}: {reason}", w.n));
                continue;
            }
        };
        let found = detect(&curvature(&g.speed, &g.yaw));
        let extra = (g.length() - map.length_m).max(0.0).ceil() as i32;
        let f = fit_within(&found, map, -(extra + 300), 300);
        if !map.usable(&found, &f) {
            dropped.push(format!("loop {}: {} of its corners meet the track's {}", w.n, f.matched, map.corners.len()));
            continue;
        }
        let (a, b) = (f.to_lap(0.0), f.to_lap(map.length_m));
        if a < 0.0 || b > g.length() {
            dropped.push(format!("loop {}: the timed lap is not whole in it", w.n));
            continue;
        }
        let (start, end) = (w.start + g.time_at(a), w.start + g.time_at(b));
        out.push(LocalLap {
            end_ms: clock.start_ms + (end * 1000.0).round() as i64,
            lap_time_ms: ((end - start) * 1000.0).round() as i64,
            ..w.lap.clone()
        });
    }
    out
}

/// The laps the game's log names in the file (see [`gamelog`]), as the app's lap list would have them: the car and
/// preset it selected last (of the file's preset), its track, the lap time as the time between the log's lines.
/// The log says nothing about a lap's validity or the pit lane: every such lap counts as valid, none as a pit lap.
fn laps_from_log(parts: &NameParts, clock: &FileClock, duration: f64, log: Option<&str>) -> Vec<LocalLap> {
    let Some(log) = log else { return Vec::new() };
    let offset = clock.offset_s * 1000;
    let from = clock.start_ms + offset;
    let to = from + (duration * 1000.0).round() as i64;
    let found = gamelog::laps(log, from, to);
    let Some(first) = found.first() else { return Vec::new() };
    let (car, preset) = gamelog::car(log, to, parts.preset.as_deref()).map_or_else(|| (id_car(&parts.car_name), parts.preset.clone()), |(car, preset)| (car, Some(preset)));
    let (track, layout) = gamelog::track(log, to)
        .and_then(|track| parts.track_folder.strip_prefix(track.as_str()).map(|layout| (track.clone(), layout.trim().to_string())))
        .unwrap_or_else(|| (parts.track_folder.clone(), String::new()));
    found
        .iter()
        .map(|lap| LocalLap {
            end_ms: lap.end - offset,
            lap_time_ms: lap.end - lap.start,
            valid: true,
            pit: false,
            car: car.clone(),
            preset: preset.clone(),
            track: track.clone(),
            layout: layout.clone(),
            session_start_ms: first.start - offset,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::clock::days_from_civil;
    use super::super::testkit::{self, Drive};
    use super::super::{Finding, TargetPsi, TimeSource};
    use super::*;
    use std::io::Cursor;

    const NAME: &str = "Test Car/TestRingGP/Test Car_preset_dd_test_car_stage1_TestRingGP_Sun Oct  4 224023 2026.ld";

    /// A session's file written at 23:40:23 CEST on 2026-10-04, 30 s after the last lap completion the log names,
    /// and the app's lap list for it; `day` moves it by days.
    fn session(drive: &Drive, day: i64) -> (Vec<u8>, String, Vec<LocalLap>, i64, i64) {
        session_at(drive, day, 0)
    }

    /// [`session`] written `shift_s` seconds later.
    fn session_at(drive: &Drive, day: i64, shift_s: i64) -> (Vec<u8>, String, Vec<LocalLap>, i64, i64) {
        let s = testkit::session(drive);
        let written_local = (days_from_civil(2026, 10, 4 + day) * 86_400) + 23 * 3600 + 40 * 60 + 23 + shift_s;
        let date = format!("{:02}/10/2026", 4 + day);
        let time = format!("{:02}:{:02}:{:02}", written_local % 86_400 / 3600, written_local % 3600 / 60, written_local % 60);
        let bytes = testkit::write_ld(&date, &time, &s.chans);
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
        let input = Input { name: NAME, modified_ms, logs: &[&log], laps: &laps, app_version: "0.25.0", now_ms: modified_ms, ldx: None, keep_days: Some(30) };
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
        // two pace laps are no spread yet: no corner finding, though lap 1 brakes 79 m earlier and loses 0.2 s
        assert!(s.laps[0].corners[2].brake.vs_ref_m.is_some_and(|d| d < -70.0));
        assert!(!s.laps[0].findings.contains(&Finding::BrakeEarlier), "{:?}", s.laps[0].findings);
        let refs = store::load_refs(&dir, &refs_key("dd_test_car", Some("preset_dd_test_car_stage1"), "Test Ring", "GP")).unwrap();
        assert_eq!(refs.pace.laps, [s.laps[0].id.clone(), s.laps[1].id.clone()]);
        assert!(refs.pace.corners.len() == 4 && refs.pace.corners.iter().all(|c| c[2].n == 2), "{:?}", refs.pace);
        // evaluated again, its laps count once
        evaluate(&dir, &Drive { late: Some(2), lock: false }, 0);
        assert_eq!(store::load_refs(&dir, &refs_key("dd_test_car", Some("preset_dd_test_car_stage1"), "Test Ring", "GP")).unwrap().pace, refs.pace);
        assert!(s.laps.iter().all(|l| l.braking_s.is_some_and(|b| b > 4.0)));
        // the theoretical best is the sum of both laps' best segments: the reference's lap time here, less what the
        // 20 Hz yaw rate leaves between equal segments (up to 0.02 s each)
        let best = s.laps[0].theoretical_best_ms.unwrap();
        assert!(best <= reference.lap_time_ms && reference.lap_time_ms - best <= 60, "{best} against {}", reference.lap_time_ms);
        // the log names no compound (and the table no test car): the pressures are not judged
        assert!(s.laps.iter().all(|l| l.tyres.target_psi.is_none()));
        assert_eq!(store::traces(&dir, &outcome.id).unwrap().len(), 2);
        assert!(corners::load(&dir, "Test Ring", "GP").is_some());
        // the coach's digest of the session is written with it
        let digests = digest::digests(&dir);
        assert_eq!((digests.len(), digests[0].id.as_str(), digests[0].best_ms, digests[0].valid_laps), (1, outcome.id.as_str(), reference.lap_time_ms, 2));
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
    fn a_lap_another_session_already_holds_is_evaluated_once() {
        let dir = temp("twice");
        let drive = Drive { late: Some(2), lock: false };
        let (first, _) = evaluate(&dir, &drive, 0);
        // the same two laps in a file written a minute later (overlapping MoTeC files of 2026-10-26): a session of
        // its own, without them
        let (bytes, log, laps, modified_ms, _) = session_at(&drive, 0, 60);
        let input = Input { name: NAME, modified_ms, logs: &[&log], laps: &laps, app_version: "0.25.0", now_ms: modified_ms, ldx: None, keep_days: Some(30) };
        let second = run(&dir, &mut Cursor::new(bytes), &input).unwrap();
        assert_ne!(second.id, first.id);
        assert_eq!(second.laps, 0, "{second:?}");
        assert_eq!(second.dropped, [format!("lap 1: already in session {}", first.id), format!("lap 2: already in session {}", first.id)]);
        // the earlier session keeps them when it is evaluated again
        assert_eq!(evaluate(&dir, &drive, 0).0.laps, 2);
        // the same laps a day later are other laps
        assert_eq!(evaluate(&dir, &drive, 1).0.laps, 2);
        store::forget(&dir).unwrap();
    }

    #[test]
    fn a_file_without_brake_temperatures_still_gives_a_report_that_reads_back() {
        let dir = temp("four");
        let (_, log, laps, modified_ms, _) = session(&Drive { late: None, lock: false }, 0);
        let mut chans = testkit::session(&Drive { late: None, lock: false }).chans;
        chans.retain(|c| !c.name.starts_with("BRAKE_TEMP_") && c.name != "FUEL");
        let bytes = testkit::write_ld("04/10/2026", "23:40:23", &chans);
        let input = Input { name: NAME, modified_ms, logs: &[&log], laps: &laps, app_version: "0.25.0", now_ms: modified_ms, ldx: None, keep_days: Some(30) };
        let outcome = run(&dir, &mut Cursor::new(bytes), &input).unwrap();
        let s = store::session(&dir, &outcome.id).expect("the stored report reads");
        assert_eq!((s.laps.len(), s.laps[0].brakes.temp_max, s.laps[0].brakes.seconds_above700, s.laps[0].fuel.used_l), (2, [0.0; 4], 0.0, 0.0));
        store::forget(&dir).unwrap();
    }

    /// `[2026-10-04 hh:mm:ss.mmm]` of a local time in ms.
    fn stamp(local_ms: i64) -> String {
        let secs = local_ms / 1000;
        format!("[2026-10-04 {:02}:{:02}:{:02}.{:03}]", secs % 86_400 / 3600, secs % 3600 / 60, secs % 60, local_ms % 1000)
    }

    /// The session's file cut after the second timed lap (the game ends a file's data at a lap completion), its
    /// local start in ms, and the log lines' prefix with the car and track.
    fn cut_session() -> (testkit::Session, Vec<u8>, i64, i64) {
        let mut s = testkit::session(&Drive { late: Some(2), lock: false });
        let end = s.laps[1].1;
        for c in &mut s.chans {
            c.values.truncate((end * c.rate as f64) as usize + 1);
        }
        let duration = s.chans[0].values.len() as f64 / 60.0;
        let written_local = days_from_civil(2026, 10, 4) * 86_400 + 23 * 3600 + 40 * 60 + 23;
        let start_local_ms = (written_local - 30) * 1000 - (duration * 1000.0).round() as i64;
        let bytes = testkit::write_ld("04/10/2026", "23:40:23", &s.chans);
        (s, bytes, start_local_ms, (written_local - 7200) * 1000 + 400)
    }

    const SELECTED: &str = "[2026-10-04 23:00:00.000] [gameplay] [info] ACEVO-2629 onSetPlayerCurrentCarCommand: Set new car 4422f4ed-904e-8717-71fa-e328d8e09daa content\\cars\\dd_test_car\\presets\\preset_dd_test_car_stage1.mechanicalcarpreset\n[2026-10-04 23:00:01.000] [physics] [info] Creating physics track: TestRing\n";

    #[test]
    fn without_the_lap_list_takes_the_laps_between_lap_completions_from_the_game_s_log() {
        let dir = temp("log");
        let (s, bytes, start, modified_ms) = cut_session();
        let at = |t: f64| start + (t * 1000.0).round() as i64;
        let completed = |t: f64, n: u32| format!("{} [physics] [info] Lap test evOnLapCompleted {n} completed\n", stamp(at(t)));
        let end = s.chans[0].values.len() as f64 / 60.0;
        let log = format!("{SELECTED}{}{}{}", completed(s.laps[0].0, 1), completed(s.laps[0].1, 2), completed(end, 3));
        let input = Input { name: NAME, modified_ms, logs: &[&log], laps: &[], app_version: "0.25.0", now_ms: modified_ms, ldx: None, keep_days: Some(30) };
        let outcome = run(&dir, &mut Cursor::new(bytes), &input).unwrap();
        // the first stretch, from the file's start, holds the car standing in its box
        assert!(outcome.laps_from_log && outcome.laps == 2, "{outcome:?}");
        assert!(outcome.dropped.len() == 1 && outcome.dropped[0].starts_with("lap 1: the car stands still"), "{:?}", outcome.dropped);
        let session = store::session(&dir, &outcome.id).unwrap();
        assert_eq!((session.car.as_str(), session.track.as_str(), session.layout.as_str(), session.timing_note), ("dd_test_car", "TestRing", "GP", TimingNote::PartialWindow));
        let times: Vec<i64> = session.laps.iter().map(|l| l.lap_time_ms.unwrap()).collect();
        let want = [s.laps[0].1 - s.laps[0].0, end - s.laps[1].0].map(|t| (t * 1000.0).round() as i64);
        assert!(times.iter().zip(want).all(|(a, b)| (a - b).abs() <= 1), "{times:?} against {want:?}");
        assert!(session.laps.iter().all(|l| l.valid && !l.pit) && session.laps[0].corners[2].dt_s.unwrap() > 0.2);
        store::forget(&dir).unwrap();
    }

    #[test]
    fn without_the_lap_list_takes_touristenfahrten_laps_from_split_to_split() {
        let dir = temp("splits");
        let (s, bytes, start, modified_ms) = cut_session();
        let at = |t: f64| stamp(start + (t * 1000.0).round() as i64);
        let end = s.chans[0].values.len() as f64 / 60.0;
        let mut log = SELECTED.to_string();
        for (a, b) in s.laps {
            log += &format!("{} [gameplay] [info] Received split: isStart true, isEnd false, index 0 \n", at(a));
            log += &format!("{} [gameplay] [info] Received split: isStart false, isEnd true, index 1 \n", at(b - 0.5));
        }
        log += &format!("{} [physics] [info] Lap test evOnLapCompleted 3 completed\n", at(end));
        let input = Input { name: NAME, modified_ms, logs: &[&log], laps: &[], app_version: "0.25.0", now_ms: modified_ms, ldx: None, keep_days: Some(30) };
        let outcome = run(&dir, &mut Cursor::new(bytes), &input).unwrap();
        assert!(outcome.laps_from_log && outcome.laps == 2 && outcome.dropped.is_empty(), "{outcome:?}");
        // with laps of the list in the file, the log's are not used
        let (_, log2, laps, _, _) = session(&Drive { late: None, lock: false }, 0);
        let (bytes, ..) = session(&Drive { late: None, lock: false }, 0);
        let listed = Input { name: NAME, modified_ms, logs: &[&log2], laps: &laps, app_version: "0.25.0", now_ms: modified_ms, ldx: None, keep_days: Some(30) };
        assert!(!run(&dir, &mut Cursor::new(bytes), &listed).unwrap().laps_from_log);
        store::forget(&dir).unwrap();
    }

    /// The `.ldx` with a beacon at each of `times` (file seconds).
    fn ldx(times: &[f64]) -> String {
        let markers: String = times.iter().enumerate().map(|(i, t)| format!("<Marker Version=\"100\" ClassName=\"BCN\" Name=\"{i}, id=99\" Flags=\"13\" Time=\"{}\" />", t * 1e6)).collect();
        format!("<?xml version=\"1.0\"?><LDXFile><Layers><Layer LayerName=\"\"><MarkerBlock><MarkerGroup Name=\"Beacons\">{markers}</MarkerGroup></MarkerBlock></Layer></Layers></LDXFile>")
    }

    #[test]
    fn without_the_lap_list_and_the_log_takes_the_laps_between_the_beacons() {
        let dir = temp("beacons");
        let drive = Drive { late: Some(2), lock: false };
        let s = testkit::session(&drive);
        let (bytes, _, laps, modified_ms, _) = session(&drive, 0);
        // the lap list holds laps of another day, so it names the car, preset, track and layout but no lap here
        let (_, _, other_day, _, _) = session(&drive, 3);
        let beacons = ldx(&[s.laps[0].0, s.laps[0].1, s.laps[1].1]);
        let input = Input { name: NAME, modified_ms, logs: &[], laps: &other_day, app_version: "0.25.0", now_ms: modified_ms, ldx: Some(&beacons), keep_days: Some(30) };
        let outcome = run(&dir, &mut Cursor::new(bytes), &input).unwrap();
        assert!(outcome.laps_from_beacons && !outcome.laps_from_log && outcome.laps == 2 && outcome.dropped.is_empty(), "{outcome:?}");
        let stored = store::session(&dir, &outcome.id).unwrap();
        assert_eq!((stored.car.as_str(), stored.preset.as_deref(), stored.track.as_str(), stored.layout.as_str(), stored.timing_note), ("dd_test_car", Some("preset_dd_test_car_stage1"), "Test Ring", "GP", TimingNote::PartialWindow));
        let times: Vec<i64> = stored.laps.iter().map(|l| l.lap_time_ms.unwrap()).collect();
        assert!(times.iter().zip(&laps).all(|(a, l)| (a - l.lap_time_ms).abs() <= 1), "{times:?} against {laps:?}");
        assert!(stored.laps.iter().all(|l| l.time_source == TimeSource::Beacon && l.valid && !l.pit));
        // nothing without an .ldx, nothing with one beacon
        let none = Input { ldx: None, ..input };
        assert_eq!(run(&dir, &mut Cursor::new(session(&drive, 0).0), &none).unwrap().laps, 0);
        let one = ldx(&[s.laps[0].0]);
        assert_eq!(run(&dir, &mut Cursor::new(session(&drive, 0).0), &Input { ldx: Some(&one), ..input }).unwrap().laps, 0);
        store::forget(&dir).unwrap();
    }

    /// The hot pressures at the end of the lap of Joe's GT2 RS MR base session of 2026-10-15 on Touristenfahrten
    /// (his stored report 1792097734-dd_porsche_gt2rs_mr, lap 1; the engineer called them too high).
    const JOE_PSI: [f64; 4] = [29.8, 29.6, 29.8, 29.6];

    const SLICK: &str = "setCompound Tyre: 0 compound name: S";

    /// The session of `car` on Touristenfahrten (`tf`) or the GP, the pressures at its laps' ends at `psi`, the
    /// second lap through the pit lane when `pit`, and the log's `lines` (at file seconds) after the car's selection.
    fn tyre_session(dir: &Path, car: &str, tf: bool, pit: bool, psi: [f64; 4], lines: &[(f64, &str)]) -> EngineerSession {
        let drive = Drive { late: None, lock: false };
        let (_, completed, mut laps, modified_ms, start_ms) = session(&drive, 0);
        let mut chans = testkit::session(&drive).chans;
        // the test car's pressures end at 27.5 psi plus 0.1 per wheel
        for (w, wheel) in ["LF", "RF", "LR", "RR"].iter().enumerate() {
            let c = chans.iter_mut().find(|c| c.name == format!("TYRE_PRESS_{wheel}")).unwrap();
            let shift = (psi[w] - (27.5 + 0.1 * w as f64)) as f32;
            c.values.iter_mut().for_each(|v| *v += shift);
        }
        let bytes = testkit::write_ld("04/10/2026", "23:40:23", &chans);
        for lap in &mut laps {
            lap.car = car.into();
            if tf {
                lap.layout = "Touristenfahrten".into();
            }
        }
        laps[1].pit = pit;
        let local = |t: f64| start_ms + 7_200_000 + (t * 1000.0).round() as i64;
        let lines: String = lines.iter().map(|(t, line)| format!("{} [physics] [info] {line}\n", stamp(local(*t)))).collect();
        let log = format!("{SELECTED}{lines}{completed}");
        let input = Input { name: if tf { NAME_T } else { NAME }, modified_ms, logs: &[&log], laps: &laps, app_version: "0.25.0", now_ms: modified_ms, ldx: None, keep_days: Some(30) };
        let outcome = run(dir, &mut Cursor::new(bytes), &input).unwrap();
        assert!(outcome.laps == 2 && outcome.from_log, "{outcome:?}");
        store::session(dir, &outcome.id).unwrap()
    }

    fn window(min: f64, max: f64) -> Option<TargetPsi> {
        Some(TargetPsi { min, max })
    }

    fn pressure_findings(lap: &super::super::EngineerLap) -> Vec<Finding> {
        lap.findings.iter().filter(|f| matches!(f, Finding::PressureHigh | Finding::PressureLow)).cloned().collect()
    }

    #[test]
    fn joe_s_porsche_on_trofeo_rs_at_29_8_psi_is_in_its_window() {
        let dir = temp("porsche");
        // the preset loads on the package's Cup 2 R, his setup fits Trofeo RS (the order of his log of 2026-10-27;
        // that of 10-15 is gone, and both are 30 psi)
        let tyres = [(1.0, "setCompound Tyre: 0 compound name: C2R"), (5.0, "setCompound Tyre: 0 compound name: TRS")];
        let s = tyre_session(&dir, "dd_porsche_gt2rs_mr", true, false, JOE_PSI, &tyres);
        for lap in &s.laps {
            assert_eq!(lap.tyres.target_psi, window(29.5, 30.5));
            assert_eq!(pressure_findings(lap), [], "{:?}", lap.tyres);
        }
        store::forget(&dir).unwrap();
    }

    #[test]
    fn a_rennsport_on_slicks_at_29_8_psi_is_too_high_and_wets_fitted_count_from_the_next_lap() {
        let dir = temp("slicks");
        let second = testkit::session(&Drive { late: None, lock: false }).laps[1].0;
        let s = tyre_session(&dir, "dd_bmw_m3_e46_rennsport", true, false, JOE_PSI, &[(1.0, SLICK), (second - 1.0, "setCompound Tyre: 0 compound name: W")]);
        let targets: Vec<Option<TargetPsi>> = s.laps.iter().map(|l| l.tyres.target_psi.clone()).collect();
        assert_eq!(targets, [window(26.5, 27.5), window(25.5, 26.5)]);
        assert!(s.laps.iter().all(|l| pressure_findings(l) == [Finding::PressureHigh]));
        store::forget(&dir).unwrap();
    }

    #[test]
    fn without_the_compound_in_the_log_or_for_a_kunos_car_the_pressures_are_not_judged() {
        let dir = temp("unknown");
        let unknown = tyre_session(&dir, "dd_bmw_m3_e46_rennsport", true, false, JOE_PSI, &[]);
        let kunos = tyre_session(&dir, "ks_bmw_m4_gt3", true, false, JOE_PSI, &[(1.0, SLICK)]);
        for lap in unknown.laps.iter().chain(&kunos.laps) {
            assert_eq!((lap.tyres.target_psi.clone(), pressure_findings(lap)), (None, vec![]));
        }
        store::forget(&dir).unwrap();
    }

    #[test]
    fn on_a_circuit_the_run_s_first_lap_and_a_lap_through_the_pit_lane_are_not_judged() {
        let dir = temp("cold");
        // the first lap of the run is out of the pit box: its tyres are still cold
        let s = tyre_session(&dir, "dd_bmw_m3_e46_rennsport", false, false, JOE_PSI, &[(1.0, SLICK)]);
        assert_eq!((s.laps[0].tyres.target_psi.clone(), pressure_findings(&s.laps[0])), (None, vec![]));
        assert_eq!((s.laps[1].tyres.target_psi.clone(), pressure_findings(&s.laps[1])), (window(26.5, 27.5), vec![Finding::PressureHigh]));
        let s = tyre_session(&dir, "dd_bmw_m3_e46_rennsport", false, true, JOE_PSI, &[(1.0, SLICK)]);
        assert!(s.laps.iter().all(|l| l.tyres.target_psi.is_none() && pressure_findings(l).is_empty()));
        store::forget(&dir).unwrap();
    }

    const NAME_T: &str = "Test Car/TestRingTouristenfahrten/Test Car_preset_dd_test_car_stage1_TestRingTouristenfahrten_Sun Oct  4 224023 2026.ld";

    #[test]
    fn finds_the_timed_laps_inside_touristenfahrten_loops_from_the_beacons() {
        let dir = temp("loops");
        let drive = Drive { late: Some(2), lock: false };
        let s = testkit::session(&drive);
        let len = testkit::track_length();
        // the timed lap is 200 m shorter at each end than the loop from pit exit to pit exit (the beacons)
        let timed = [(s.at_m(len + 200.0), s.at_m(2.0 * len - 200.0)), (s.at_m(2.0 * len + 200.0), s.at_m(3.0 * len - 200.0))];
        let (bytes, log, mut laps, modified_ms, start_ms) = session(&drive, 0);
        for (lap, (a, b)) in laps.iter_mut().zip(&timed) {
            lap.end_ms = start_ms + (b * 1000.0).round() as i64;
            lap.lap_time_ms = ((b - a) * 1000.0).round() as i64;
            lap.layout = "Touristenfahrten".into();
        }
        // the first session, with the lap list: the corner map of the timed lap
        let first = Input { name: NAME_T, modified_ms, logs: &[&log], laps: &laps, app_version: "0.25.0", now_ms: modified_ms, ldx: None, keep_days: Some(30) };
        let outcome = run(&dir, &mut Cursor::new(bytes), &first).unwrap();
        assert!(outcome.laps == 2 && outcome.dropped.is_empty(), "{outcome:?}");
        let map = corners::load(&dir, "Test Ring", "Touristenfahrten").unwrap();
        assert!((map.length_m - (len - 400.0)).abs() < 2.0, "{}", map.length_m);
        // the next day's session without the list and the log: the loops from the beacons, the timed laps in them
        let (bytes, _, _, modified_ms, _) = session(&drive, 1);
        let beacons = ldx(&[s.at_m(len), s.at_m(2.0 * len), s.at_m(3.0 * len)]);
        let next = Input { name: NAME_T, modified_ms, logs: &[], laps: &laps, app_version: "0.25.0", now_ms: modified_ms, ldx: Some(&beacons), keep_days: Some(30) };
        let outcome = run(&dir, &mut Cursor::new(bytes), &next).unwrap();
        assert!(outcome.laps_from_beacons && outcome.laps == 2 && outcome.dropped.is_empty(), "{outcome:?}");
        let stored = store::session(&dir, &outcome.id).unwrap();
        assert_eq!((stored.track.as_str(), stored.layout.as_str(), stored.timing_note), ("Test Ring", "Touristenfahrten", TimingNote::PartialWindow));
        for (l, want) in stored.laps.iter().zip(&laps) {
            assert_eq!(l.time_source, TimeSource::Telemetry);
            assert!((l.lap_time_ms.unwrap() - want.lap_time_ms).abs() <= 100, "{} against {}", l.lap_time_ms.unwrap(), want.lap_time_ms);
            assert!((l.distance_m - map.length_m).abs() <= 3.0, "{} m", l.distance_m);
        }
        // measured against the first session's reference, on the same corners: lap 2 brakes late for corner 3
        assert_eq!(stored.reference.as_ref().map(|r| r.session_id.as_str()), Some(store::heads(&dir)[1].id.as_str()));
        assert!(stored.laps[0].corners[2].dt_s.unwrap() > 0.2, "{:?}", stored.laps[0].corners[2]);
        // without a map of the track yet, the loops give nothing
        store::forget(&dir).unwrap();
        let outcome = run(&dir, &mut Cursor::new(session(&drive, 1).0), &next).unwrap();
        assert!(outcome.laps == 0 && outcome.dropped[0].starts_with("no corner map"), "{outcome:?}");
        store::forget(&dir).unwrap();
    }

    #[test]
    fn says_why_a_lap_is_not_evaluated_and_keeps_a_session_without_laps() {
        let dir = temp("three");
        let (bytes, log, mut laps, modified_ms, _) = session(&Drive { late: None, lock: false }, 0);
        // the app's clock was 30 s off for the first lap: its window holds the out lap's standstill... or not a lap
        laps[0].end_ms -= 30_000;
        let input = Input { name: NAME, modified_ms, logs: &[&log], laps: &laps, app_version: "0.25.0", now_ms: modified_ms, ldx: None, keep_days: Some(30) };
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
