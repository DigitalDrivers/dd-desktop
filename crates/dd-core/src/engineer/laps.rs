//! The app's own lap list for the engineer, `<config>/engineer/laps.jsonl`: the shell turns the shared memory into
//! laps ([`crate::laps::LapRecorder`]) and, while the local switch is on, appends each finished lap with the PC's
//! clock at the moment it settled. The MoTeC files have no lap times and no usable lap lines (the `.ldx` beacons
//! are not the timing lines on Touristenfahrten), so these are the laps the engineer cuts out of them.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::laps::LapSummary;
use crate::motec::CHAIN_GAP_SECS;

pub const FILE: &str = "laps.jsonl";

/// A finished lap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalLap {
    /// When the lap ended, unix ms (the shell's clock when the lap settled)
    pub end_ms: i64,
    pub lap_time_ms: i64,
    pub valid: bool,
    pub pit: bool,
    pub car: String,
    pub preset: Option<String>,
    pub track: String,
    pub layout: String,
    /// When the run of laps it belongs to started (the start of its first lap), unix ms
    pub session_start_ms: i64,
}

impl LocalLap {
    pub fn start_ms(&self) -> i64 {
        self.end_ms - self.lap_time_ms
    }
}

/// The entry for a lap the recorder returned at `end_ms`, the car's id and preset filled in by the shell. A lap of
/// the same car, preset, track and layout that starts at most 30 minutes after the `previous` entry ended belongs
/// to its run, as files of a MoTeC session do; None for a lap without a time.
pub fn entry(previous: Option<&LocalLap>, lap: &LapSummary, end_ms: i64) -> Option<LocalLap> {
    if lap.lap_time_ms <= 0 {
        return None;
    }
    let start = end_ms - lap.lap_time_ms as i64;
    let same_run = previous.filter(|p| {
        p.car == lap.car && p.preset == lap.preset && p.track == lap.track && p.layout == lap.layout && start - p.end_ms <= CHAIN_GAP_SECS as i64 * 1000 && start >= p.end_ms - 1000
    });
    Some(LocalLap {
        end_ms,
        lap_time_ms: lap.lap_time_ms as i64,
        valid: lap.valid,
        pit: lap.pit,
        car: lap.car.clone(),
        preset: lap.preset.clone(),
        track: lap.track.clone(),
        layout: lap.layout.clone(),
        session_start_ms: same_run.map_or(start, |p| p.session_start_ms),
    })
}

/// The laps of a list; lines that do not read (a line cut off by a crash) are skipped.
pub fn parse(text: &str) -> Vec<LocalLap> {
    text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

pub fn load(dir: &Path) -> Vec<LocalLap> {
    fs::read_to_string(dir.join(FILE)).map(|text| parse(&text)).unwrap_or_default()
}

pub fn append(dir: &Path, lap: &LocalLap) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(dir.join(FILE))?;
    writeln!(file, "{}", serde_json::to_string(lap).map_err(io::Error::other)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(car: &str, lap_time_ms: i32) -> LapSummary {
        let tyre = crate::laps::Tyre {
            pressure_avg: 0.0, pressure_max: 0.0, pressure_end: 0.0, core_temp_avg: 0.0, core_temp_max: 0.0, brake_temp_max: 0.0,
            travel_max: 0.0, travel_avg: 0.0, lock_share: 0.0, spin_share: 0.0,
        };
        LapSummary {
            car: car.into(), track: "Nurburgring".into(), layout: "Touristenfahrten".into(), online: true, lap_time_ms, valid: true, pit: false,
            fuel_used_l: 0.0, air_temp_c: 0.0, road_temp_c: 0.0, top_speed_kmh: 0.0, tyres: std::array::from_fn(|_| tyre.clone()),
            ride_height_min: [0.0; 2], ride_height_avg: [0.0; 2], brake_bias: 0.0, brake_front: 0.0, balance_deg: 0.0, rpm_max: 0,
            gear_at_top_speed: 0, lat_g_max: 0.0, brake_g_max: 0.0, preset: Some("preset_dd_bmw_m3_e46_rennsport_unl3".into()),
            package_version: None, setup_name: None, first: Default::default(),
        }
    }

    #[test]
    fn a_lap_joins_the_run_of_the_one_before_within_half_an_hour() {
        let rennsport = summary("dd_bmw_m3_e46_rennsport", 375_780);
        let first = entry(None, &rennsport, 1_000_000_000).unwrap();
        assert_eq!((first.start_ms(), first.session_start_ms, first.lap_time_ms), (999_624_220, 999_624_220, 375_780));
        // the next lap after a pit stop, within 30 minutes
        let second = entry(Some(&first), &summary("dd_bmw_m3_e46_rennsport", 368_283), 1_000_505_665).unwrap();
        assert_eq!(second.session_start_ms, first.session_start_ms);
        // more than 30 minutes later, another car, or another preset: a new run
        let late = entry(Some(&first), &rennsport, 1_000_000_000 + 375_780 + CHAIN_GAP_SECS as i64 * 1000 + 1).unwrap();
        assert_eq!(late.session_start_ms, late.start_ms());
        assert_ne!(entry(Some(&first), &summary("dd_bmw_m3_e46_gt3", 375_780), 1_000_400_000).unwrap().session_start_ms, first.session_start_ms);
        let mut stage = rennsport.clone();
        stage.preset = Some("preset_dd_bmw_m3_e46_rennsport_unl2".into());
        assert_ne!(entry(Some(&first), &stage, 1_000_400_000).unwrap().session_start_ms, first.session_start_ms);
        assert!(entry(None, &summary("x", 0), 1).is_none());
    }

    #[test]
    fn appends_one_line_per_lap_and_skips_broken_lines() {
        let dir = std::env::temp_dir().join(format!("dd-engineer-laps-{}", std::process::id()));
        let lap = entry(None, &summary("dd_bmw_m3_e46_rennsport", 375_780), 1_000_000_000).unwrap();
        append(&dir, &lap).unwrap();
        append(&dir, &lap).unwrap();
        let text = fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(text.lines().next().unwrap(), r#"{"endMs":1000000000,"lapTimeMs":375780,"valid":true,"pit":false,"car":"dd_bmw_m3_e46_rennsport","preset":"preset_dd_bmw_m3_e46_rennsport_unl3","track":"Nurburgring","layout":"Touristenfahrten","sessionStartMs":999624220}"#);
        fs::write(dir.join(FILE), format!("{text}{{\"endMs\":12")).unwrap();
        assert_eq!(load(&dir), vec![lap.clone(), lap]);
        fs::remove_dir_all(&dir).unwrap();
        assert!(load(&dir).is_empty());
    }
}
