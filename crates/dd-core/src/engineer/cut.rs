//! Cutting laps out of a MoTeC file and putting them on a distance grid. A lap of the app's list whose end lies in
//! the file gives the window [end − lap time, end] in file seconds. The window must show a moving car (SPEED never
//! below 1 m/s for more than 2 s), then SPEED is integrated over time (trapezoid) and every channel is resampled at
//! each metre of the lap, so two laps compare point by point over distance.

use std::io::{self, Read, Seek};

use super::laps::LocalLap;
use super::ld::{Ld, Series};
use super::sessions::{same_track, NameParts};

/// The longest lap evaluated (the Nordschleife with the GP circuit is 25.4 km): a longer window is no lap.
pub const MAX_LAP_M: f64 = 40_000.0;
/// A shorter window is no lap either.
pub const MIN_LAP_M: f64 = 500.0;
/// Standing longer than this inside a window means it is not a flying lap (or not this lap).
const STAND_S: f64 = 2.0;
const STAND_MS: f64 = 1.0;
/// G_LAT is averaged over this many seconds, so a kerb strike (12 g in one 20 Hz sample) does not count as grip.
const G_LAT_WINDOW_S: f64 = 0.5;
pub const WHEELS: [&str; 4] = ["LF", "RF", "LR", "RR"];

/// The channels the engineer reads. A channel the file does not have is empty (its values read as 0).
pub struct Data {
    pub speed: Series,
    pub throttle: Series,
    pub brake: Series,
    pub steer: Series,
    pub gear: Series,
    pub g_lat: Series,
    pub yaw: Series,
    pub fuel: Series,
    pub abs: Series,
    pub pressure: [Series; 4],
    pub tyre_air: [Series; 4],
    /// The surface temperatures L, C, R of each wheel
    pub surface: [[Series; 3]; 4],
    pub brake_temp: [Series; 4],
    pub torque: [Series; 4],
    pub slip: [Series; 4],
}

impl Data {
    pub fn read<R: Read + Seek>(ld: &Ld, r: &mut R) -> io::Result<Data> {
        let mut get = |name: &str| -> io::Result<Series> { Ok(ld.values(r, name)?.unwrap_or(Series { rate: 1.0, values: Vec::new() })) };
        let speed = get("SPEED")?;
        if speed.values.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the file has no SPEED"));
        }
        let mut wheels = |prefix: &str| -> io::Result<[Series; 4]> {
            let mut out = Vec::new();
            for w in WHEELS {
                out.push(get(&format!("{prefix}{w}"))?);
            }
            Ok(out.try_into().unwrap_or_else(|_| unreachable!()))
        };
        let pressure = wheels("TYRE_PRESS_")?;
        let tyre_air = wheels("TYRE_TAIR_")?;
        let brake_temp = wheels("BRAKE_TEMP_")?;
        let torque = wheels("BRAKE_TORQUE_")?;
        let slip = wheels("WHEEL_SR_")?;
        let mut surface = Vec::new();
        for w in WHEELS {
            surface.push([get(&format!("L_SURF_TEMP_{w}"))?, get(&format!("C_SURF_TEMP_{w}"))?, get(&format!("R_SURF_TEMP_{w}"))?]);
        }
        Ok(Data {
            speed,
            throttle: get("THROTTLE")?,
            brake: get("BRAKE")?,
            steer: get("STEERANGLE")?,
            gear: get("GEAR")?,
            g_lat: get("G_LAT")?,
            yaw: get("ROTY")?,
            fuel: get("FUEL")?,
            abs: get("ABS")?,
            pressure,
            tyre_air,
            surface: surface.try_into().unwrap_or_else(|_| unreachable!()),
            brake_temp,
            torque,
            slip,
        })
    }
}

/// A lap of the list inside a file, numbered from 1 in the order they ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Window<'a> {
    pub n: u32,
    pub lap: &'a LocalLap,
    /// File seconds
    pub start: f64,
    pub end: f64,
}

/// The laps of `laps` driven on the file's track (and preset, when both name one) that lie wholly in the file,
/// which starts at `start_ms` (unix ms) and holds `duration` seconds.
pub fn windows<'a>(laps: &'a [LocalLap], parts: &NameParts, start_ms: i64, duration: f64) -> Vec<Window<'a>> {
    let mut found: Vec<&LocalLap> = laps
        .iter()
        .filter(|lap| same_track(&lap.track, &lap.layout, &parts.track_folder))
        .filter(|lap| lap.preset.is_none() || parts.preset.is_none() || lap.preset == parts.preset)
        .filter(|lap| {
            let (a, b) = ((lap.start_ms() - start_ms) as f64 / 1000.0, (lap.end_ms - start_ms) as f64 / 1000.0);
            // a lap from the game's log may end at the data's last millisecond
            a >= -0.01 && b <= duration + 0.01
        })
        .collect();
    found.sort_by_key(|lap| lap.end_ms);
    found.dedup_by_key(|lap| lap.end_ms);
    found
        .into_iter()
        .enumerate()
        .map(|(i, lap)| Window { n: i as u32 + 1, lap, start: (lap.start_ms() - start_ms) as f64 / 1000.0, end: (lap.end_ms - start_ms) as f64 / 1000.0 })
        .collect()
}

/// A lap over distance: every channel at each metre from the window's start (index = metre).
#[derive(Debug, Clone, Default)]
pub struct Grid {
    /// Seconds from the window's start
    pub t: Vec<f64>,
    /// m/s
    pub speed: Vec<f64>,
    pub throttle: Vec<f64>,
    pub brake: Vec<f64>,
    pub steer: Vec<f64>,
    pub gear: Vec<f64>,
    /// g, averaged over half a second
    pub g_lat: Vec<f64>,
    /// rad/s
    pub yaw: Vec<f64>,
    pub slip: [Vec<f64>; 4],
}

impl Grid {
    /// The lap's distance in metres.
    pub fn length(&self) -> f64 {
        self.t.len().saturating_sub(1) as f64
    }

    /// Seconds spent from metre `i` to the next.
    pub fn dt(&self, i: usize) -> f64 {
        if i + 1 < self.t.len() { self.t[i + 1] - self.t[i] } else { 0.0 }
    }

    /// The time at `m` metres, between the grid's points.
    pub fn time_at(&self, m: f64) -> f64 {
        let m = m.clamp(0.0, self.length());
        let i = (m.floor() as usize).min(self.t.len().saturating_sub(2));
        let f = m - i as f64;
        self.t[i] * (1.0 - f) + self.t.get(i + 1).copied().unwrap_or(self.t[i]) * f
    }
}

/// The mean of a series' samples within [a, b] seconds; the value at the middle when none lies inside.
pub fn mean(series: &Series, a: f64, b: f64) -> f64 {
    let range = series.range(a, b);
    if range.is_empty() {
        return series.at((a + b) / 2.0);
    }
    let n = range.len() as f64;
    series.values[range].iter().map(|v| *v as f64).sum::<f64>() / n
}

/// The lap of window [start, end] on the distance grid, or why it is not a lap.
pub fn grid(data: &Data, start: f64, end: f64) -> Result<Grid, String> {
    grid_in(data, start, end, true)
}

/// [`grid`] of a window that may hold a standing car (a Touristenfahrten loop through the parking lot, in which
/// the timed lap is still to be found): `must_move` false.
pub fn grid_in(data: &Data, start: f64, end: f64, must_move: bool) -> Result<Grid, String> {
    let speed = &data.speed;
    // standing: SPEED below 1 m/s for more than 2 s in a row
    let mut still_since: Option<f64> = None;
    for i in speed.range(start, end).filter(|_| must_move) {
        let t = i as f64 / speed.rate;
        if (speed.values[i] as f64) < STAND_MS {
            let since = *still_since.get_or_insert(t);
            if t - since > STAND_S {
                return Err(format!("the car stands still at {since:.1} s of the file"));
            }
        } else {
            still_since = None;
        }
    }
    // distance at the SPEED samples, with the window's ends
    let mut times = vec![start];
    times.extend(speed.range(start, end).map(|i| i as f64 / speed.rate).filter(|t| *t > start && *t < end));
    times.push(end);
    let mut dist = vec![0.0];
    for k in 1..times.len() {
        let (a, b) = (times[k - 1], times[k]);
        dist.push(dist[k - 1] + (speed.at(a) + speed.at(b)) / 2.0 * (b - a));
    }
    let length = *dist.last().unwrap();
    if length > MAX_LAP_M {
        return Err(format!("{length:.0} m is longer than any lap"));
    }
    if length < MIN_LAP_M {
        return Err(format!("{length:.0} m is shorter than any lap"));
    }
    let mut g = Grid::default();
    let mut k = 0;
    for m in 0..=(length.floor() as usize) {
        let m = m as f64;
        while k + 1 < dist.len() - 1 && dist[k + 1] < m {
            k += 1;
        }
        let (d0, d1) = (dist[k], dist[k + 1]);
        let f = if d1 > d0 { ((m - d0) / (d1 - d0)).clamp(0.0, 1.0) } else { 0.0 };
        let t = times[k] + f * (times[k + 1] - times[k]);
        g.t.push(t - start);
        g.speed.push(speed.at(t));
        g.throttle.push(data.throttle.at(t));
        g.brake.push(data.brake.at(t));
        g.steer.push(data.steer.at(t));
        g.gear.push(data.gear.at(t).round());
        g.g_lat.push(mean(&data.g_lat, t - G_LAT_WINDOW_S / 2.0, t + G_LAT_WINDOW_S / 2.0));
        g.yaw.push(data.yaw.at(t));
        for w in 0..4 {
            g.slip[w].push(data.slip[w].at(t));
        }
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{self, Drive};
    use super::*;
    use std::io::Cursor;

    fn data(drive: &Drive) -> (testkit::Session, Data) {
        let session = testkit::session(drive);
        let bytes = testkit::write_ld("04/10/2026", "23:40:23", &session.chans);
        let mut r = Cursor::new(bytes);
        let ld = Ld::read(&mut r).unwrap();
        let data = Data::read(&ld, &mut r).unwrap();
        (session, data)
    }

    fn parts() -> NameParts {
        NameParts { car_name: "Test Car".into(), track_folder: "TestRingGP".into(), preset: Some("preset_dd_test_car_stage1".into()) }
    }

    #[test]
    fn finds_the_laps_of_the_list_inside_the_file() {
        let session = testkit::session(&Drive { late: None, lock: false });
        let start_ms = 1_791_000_000_000;
        let mut laps = testkit::local_laps(&session, start_ms);
        let found = windows(&laps, &parts(), start_ms, session.duration);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].n, found[1].n), (1, 2));
        assert!((found[0].start - session.laps[0].0).abs() < 0.001 && (found[1].end - session.laps[1].1).abs() < 0.001);
        // another track, another preset, a lap that ended after the file or started before it: not in it
        laps[0].layout = "Nordschleife".into();
        assert_eq!(windows(&laps, &parts(), start_ms, session.duration).len(), 1);
        laps[0].layout = "GP".into();
        laps[1].preset = Some("preset_dd_test_car_stage2".into());
        assert_eq!(windows(&laps, &parts(), start_ms, session.duration).len(), 1);
        assert!(windows(&laps, &parts(), start_ms + (session.duration * 1000.0) as i64, session.duration).is_empty());
        let mut early = laps[0].clone();
        early.lap_time_ms = early.end_ms - start_ms + 1000;
        assert!(windows(&[early], &parts(), start_ms, session.duration).is_empty());
    }

    #[test]
    fn puts_a_lap_on_a_one_metre_grid() {
        let (session, data) = data(&Drive { late: None, lock: false });
        let (a, b) = session.laps[0];
        let lap = grid(&data, a, b).unwrap();
        assert!((lap.length() - testkit::track_length()).abs() < 2.0, "{} m against {}", lap.length(), testkit::track_length());
        assert!((lap.t[lap.t.len() - 1] - (b - a)).abs() < 0.05);
        // in the hairpin (R 40 m, 15 m/s²): 24.5 m/s, 1.53 g, yaw v/R
        let (hairpin, _, _) = testkit::arcs()[2];
        let i = hairpin as usize + 60;
        assert!((lap.speed[i] - 24.49).abs() < 0.1, "{}", lap.speed[i]);
        assert!((lap.g_lat[i] - 1.529).abs() < 0.02 && (lap.yaw[i] - 24.49 / 40.0).abs() < 0.01);
        assert_eq!((lap.throttle[i], lap.brake[i]), (30.0, 0.0));
        // the time over distance adds up
        assert!((lap.time_at(lap.length()) - (b - a)).abs() < 0.05);
        assert!((0..lap.t.len() - 1).map(|i| lap.dt(i)).sum::<f64>() - (b - a) < 0.05);
    }

    #[test]
    fn drops_a_window_in_which_the_car_stands_still() {
        let (session, data) = data(&Drive { late: None, lock: false });
        let error = grid(&data, 5.0, session.laps[0].1).unwrap_err();
        assert!(error.contains("stands still at 5.0 s"), "{error}");
        assert!(grid(&data, session.laps[1].0, session.laps[1].0 + 3.0).unwrap_err().contains("shorter"));
    }
}
