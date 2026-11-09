//! Test data: a writer of MoTeC `.ld` files in the layout the game writes, and a car driving a parametric track,
//! so every number of the engineer can be checked against a known answer.

use super::corners::{Map, MapCorner};
use super::cut::Grid;
use super::laps::LocalLap;
use super::Direction;

/// A channel to write: f32 values, or i16 raw values with `dec` decimals (`value = raw × 10^-dec`).
pub struct Chan {
    pub name: &'static str,
    pub unit: &'static str,
    pub rate: u16,
    pub values: Vec<f32>,
    pub i16_dec: Option<i16>,
}

pub fn chan(name: &'static str, unit: &'static str, rate: u16, values: Vec<f32>) -> Chan {
    Chan { name, unit, rate, values, i16_dec: None }
}

const META: usize = 256;
const HEADER: usize = 124;

/// An `.ld` file: the header with `date` (`dd/mm/yyyy`) and `time` (`hh:mm:ss`), the channel list as a linked
/// list of 124-byte headers from byte 256, then the data.
pub fn write_ld(date: &str, time: &str, chans: &[Chan]) -> Vec<u8> {
    let data_start = META + chans.len() * HEADER;
    let mut out = vec![0u8; data_start];
    out[0..4].copy_from_slice(&0x40u32.to_le_bytes());
    out[8..12].copy_from_slice(&(META as u32).to_le_bytes());
    out[12..16].copy_from_slice(&(data_start as u32).to_le_bytes());
    out[94..94 + date.len()].copy_from_slice(date.as_bytes());
    out[126..126 + time.len()].copy_from_slice(time.as_bytes());
    for (i, c) in chans.iter().enumerate() {
        let at = META + i * HEADER;
        let prev = if i == 0 { 0 } else { at - HEADER } as u32;
        let next = if i + 1 == chans.len() { 0 } else { at + HEADER } as u32;
        let data_ptr = out.len() as u32;
        let (dta, dt, dec): (u16, u16, i16) = match c.i16_dec {
            Some(dec) => (3, 2, dec),
            None => (7, 4, 0),
        };
        for v in &c.values {
            match c.i16_dec {
                Some(dec) => out.extend_from_slice(&((*v as f64 * 10f64.powi(dec as i32)).round() as i16).to_le_bytes()),
                None => out.extend_from_slice(&v.to_le_bytes()),
            }
        }
        let h = &mut out[at..at + HEADER];
        h[0..4].copy_from_slice(&prev.to_le_bytes());
        h[4..8].copy_from_slice(&next.to_le_bytes());
        h[8..12].copy_from_slice(&data_ptr.to_le_bytes());
        h[12..16].copy_from_slice(&(c.values.len() as u32).to_le_bytes());
        h[16..18].copy_from_slice(&(12000 + i as u16).to_le_bytes());
        h[18..20].copy_from_slice(&dta.to_le_bytes());
        h[20..22].copy_from_slice(&dt.to_le_bytes());
        h[22..24].copy_from_slice(&c.rate.to_le_bytes());
        h[24..26].copy_from_slice(&0i16.to_le_bytes()); // shift
        h[26..28].copy_from_slice(&1i16.to_le_bytes()); // mul
        h[28..30].copy_from_slice(&1i16.to_le_bytes()); // scale
        h[30..32].copy_from_slice(&dec.to_le_bytes());
        h[32..32 + c.name.len()].copy_from_slice(c.name.as_bytes());
        h[72..72 + c.unit.len()].copy_from_slice(c.unit.as_bytes());
    }
    out
}

/// A piece of the test track.
#[derive(Clone, Copy)]
pub enum Piece {
    Straight(f64),
    /// Radius in m, angle in degrees, left
    Arc(f64, f64, bool),
}

/// The test track: four corners of known radius, 2,784 m. The arcs' centres (m from the start) are in
/// [`ARC_CENTRES`].
pub const TRACK: [Piece; 8] = [
    Piece::Straight(600.0),
    Piece::Arc(60.0, 90.0, true),
    Piece::Straight(450.0),
    Piece::Arc(150.0, 60.0, false),
    Piece::Straight(500.0),
    Piece::Arc(40.0, 180.0, true),
    Piece::Straight(700.0),
    Piece::Arc(200.0, 45.0, false),
];

/// Where each arc of [`TRACK`] starts and ends, and its signed curvature (> 0 left).
pub fn arcs() -> Vec<(f64, f64, f64)> {
    arcs_on(&TRACK)
}

/// Where each arc of `track` starts and ends, and its signed curvature (> 0 left).
pub fn arcs_on(track: &[Piece]) -> Vec<(f64, f64, f64)> {
    let mut s = 0.0;
    let mut out = Vec::new();
    for &piece in track {
        match piece {
            Piece::Straight(len) => s += len,
            Piece::Arc(r, deg, left) => {
                let len = r * deg.to_radians();
                out.push((s, s + len, if left { 1.0 / r } else { -1.0 / r }));
                s += len;
            }
        }
    }
    out
}

pub fn track_length() -> f64 {
    arcs().last().unwrap().1
}

/// Signed curvature at `s` metres from the start of a lap.
fn kappa(s: f64) -> f64 {
    let s = s.rem_euclid(track_length());
    arcs().into_iter().find(|(a, b, _)| s >= *a && s < *b).map_or(0.0, |(_, _, k)| k)
}

/// A track that closes on itself, so a car can drive it lap after lap in the world (TRACK's turns add up to 165°):
/// a chicane (R 100 m right and left, 30° each, 20 m apart) on the start straight, then four left turns of 90°
/// (R 60, 40, 150 and 200 m), the last two straights as long as closing the loop needs.
pub fn closed_track() -> Vec<Piece> {
    let (r1, r2, r3, r4, b) = (60.0, 40.0, 150.0, 200.0, 600.0);
    let (rc, angle, gap) = (100.0f64, 30.0f64.to_radians(), 20.0);
    // the chicane moves the car this far ahead and this far to the right
    let (ahead, right) = (2.0 * rc * angle.sin() + gap * angle.cos(), 2.0 * rc * (1.0 - angle.cos()) + gap * angle.sin());
    let a = 800.0 + ahead;
    vec![
        Piece::Straight(400.0),
        Piece::Arc(rc, 30.0, false),
        Piece::Straight(gap),
        Piece::Arc(rc, 30.0, true),
        Piece::Straight(400.0),
        Piece::Arc(r1, 90.0, true),
        Piece::Straight(b),
        Piece::Arc(r2, 90.0, true),
        Piece::Straight(a + r1 + r4 - r2 - r3),
        Piece::Arc(r3, 90.0, true),
        Piece::Straight(b + r1 + r2 - right - r3 - r4),
        Piece::Arc(r4, 90.0, true),
    ]
}

pub fn length_of(track: &[Piece]) -> f64 {
    track.iter().map(|p| match *p {
        Piece::Straight(len) => len,
        Piece::Arc(r, deg, _) => r * deg.to_radians(),
    }).sum()
}

/// The centre line of `track` at `s` metres (round the lap): x and z in the radio's test world (right-handed, Y up;
/// right of a direction (x, z) is (-z, x)), the lap starting at the origin heading along +x.
pub fn pose_on(track: &[Piece], s: f64) -> [f64; 2] {
    let mut left = s.rem_euclid(length_of(track));
    let (mut x, mut z, mut h) = (0.0f64, 0.0f64, 0.0f64);
    for &piece in track {
        let (len, turn) = match piece {
            Piece::Straight(len) => (len, 0.0),
            // a right turn turns the heading towards (-z, x), so the angle atan2(z, x) grows
            Piece::Arc(r, deg, is_left) => (r * deg.to_radians(), if is_left { -1.0 / r } else { 1.0 / r }),
        };
        let u = left.min(len);
        if turn == 0.0 {
            (x, z) = (x + u * h.cos(), z + u * h.sin());
        } else {
            let end = h + turn * u;
            (x, z) = (x + (end.sin() - h.sin()) / turn, z - (end.cos() - h.cos()) / turn);
            h = end;
        }
        left -= u;
        if left <= 0.0 {
            break;
        }
    }
    [x, z]
}

/// A corner driven differently from the car's limit: `slower_kmh` slower through the arc (index into
/// [`arcs_on`]), and braking for it from `brake_at` metres of the lap on, softly enough to reach the arc's speed
/// at its start, when given.
#[derive(Clone, Copy, Debug)]
pub struct Change {
    pub corner: usize,
    pub slower_kmh: f64,
    pub brake_at: Option<f64>,
}

/// A flying lap of `track` (which must close, see [`closed_track`]) at the car's limit but for `changes`: the speed
/// (m/s) every [`STEP`] metres from the lap's start to its end (both ends included) and whether the car brakes
/// there. It is the middle one of three such laps, so it ends at the speed it started with.
pub fn flying_lap(track: &[Piece], changes: &[Change]) -> (Vec<f64>, Vec<bool>) {
    let len = length_of(track);
    let per_lap = (len / STEP).round() as usize;
    let n = 3 * per_lap + 1;
    let arcs = arcs_on(track);
    let at = |i: usize| (i % per_lap) as f64 * STEP;
    let arc_of = |s: f64| arcs.iter().position(|(a, b, _)| s >= *a && s < *b);
    let corner_speed = |c: usize| (LAT * arcs[c].2.abs().recip()).sqrt().min(VMAX) - changes.iter().filter(|x| x.corner == c).map(|x| x.slower_kmh / 3.6).sum::<f64>();
    let limit: Vec<f64> = (0..n).map(|i| arc_of(at(i)).map_or(VMAX, corner_speed)).collect();
    let mut fwd = vec![0.0; n];
    for i in 1..n {
        fwd[i] = (fwd[i - 1] * fwd[i - 1] + 2.0 * ACCEL * STEP).sqrt().min(limit[i]);
    }
    let mut back = vec![0.0; n];
    for i in (0..n - 1).rev() {
        back[i] = (back[i + 1] * back[i + 1] + 2.0 * BRAKE * STEP).sqrt().min(limit[i]);
    }
    let mut v: Vec<f64> = (0..n).map(|i| fwd[i].min(back[i])).collect();
    for change in changes {
        let Some(brake_at) = change.brake_at else { continue };
        let (vc, arc_start) = (corner_speed(change.corner), arcs[change.corner].0);
        for lap in 0..3 {
            let (from, to) = (lap * per_lap + (brake_at / STEP).round() as usize, lap * per_lap + (arc_start / STEP).round() as usize);
            let v0 = v[from];
            let decel = (v0 * v0 - vc * vc) / (2.0 * (to - from) as f64 * STEP);
            for (i, x) in v.iter_mut().enumerate().take(to + 1).skip(from) {
                *x = x.min((vc * vc + 2.0 * decel * (to - i) as f64 * STEP).sqrt());
            }
        }
    }
    let braking: Vec<bool> = (0..n).map(|i| i + 1 < n && v[i + 1] < v[i] - 1e-9).collect();
    (v[per_lap..=2 * per_lap].to_vec(), braking[per_lap..=2 * per_lap].to_vec())
}

pub const G: f64 = 9.81;
const LAT: f64 = 15.0;
const BRAKE: f64 = 12.0;
const ACCEL: f64 = 5.0;
const VMAX: f64 = 75.0;
pub const STEP: f64 = 0.25;

/// What the car does in the session: standing `STAND` seconds, an out lap from standstill, two timed laps, a stop
/// after `TAIL` metres, standing again. `late` makes the car brake harder (and so later) for corner `late` of the
/// second timed lap; `lock` makes the fronts lock there.
pub struct Drive {
    pub late: Option<usize>,
    pub lock: bool,
}

pub const STAND: f64 = 20.0;
const TAIL: f64 = 300.0;

/// The car's speed over distance (`STEP` metres), the braking zone flags and the start of each timed lap.
pub struct Profile {
    pub v: Vec<f64>,
    pub braking: Vec<bool>,
    pub late_zone: Vec<bool>,
}

impl Drive {
    pub fn profile(&self) -> Profile {
        let len = track_length();
        let total = 3.0 * len + TAIL;
        let n = (total / STEP) as usize + 1;
        let limit: Vec<f64> = (0..n).map(|i| {
            let s = i as f64 * STEP;
            let k = kappa(s).abs();
            if s >= 3.0 * len { VMAX } else if k > 0.0 { (LAT / k).sqrt().min(VMAX) } else { VMAX }
        }).collect();
        let late_zone: Vec<bool> = (0..n).map(|i| {
            let s = i as f64 * STEP;
            self.late.is_some_and(|c| {
                let (a, _, _) = arcs()[c];
                s > 2.0 * len + a - 400.0 && s < 2.0 * len + a
            })
        }).collect();
        let mut fwd = vec![0.0; n];
        for i in 1..n {
            fwd[i] = (fwd[i - 1] * fwd[i - 1] + 2.0 * ACCEL * STEP).sqrt().min(limit[i]);
        }
        let mut back = vec![0.0; n];
        back[n - 1] = 0.0;
        for i in (0..n - 1).rev() {
            let decel = if late_zone[i] { 1.8 * BRAKE } else { BRAKE };
            back[i] = (back[i + 1] * back[i + 1] + 2.0 * decel * STEP).sqrt().min(limit[i]);
        }
        let v: Vec<f64> = (0..n).map(|i| fwd[i].min(back[i])).collect();
        let braking: Vec<bool> = (0..n).map(|i| back[i] < fwd[i] - 1e-9 && i + 1 < n && v[i + 1] < v[i]).collect();
        Profile { v, braking, late_zone }
    }
}

/// The session as channels at the game's rates, the file's duration, and the timed laps' start and end in file
/// seconds.
pub struct Session {
    pub chans: Vec<Chan>,
    pub duration: f64,
    pub laps: [(f64, f64); 2],
    /// The file time at each `STEP` metres of the drive
    times: Vec<f64>,
}

impl Session {
    /// The file time at `m` metres of the drive (0 = the start of the out lap).
    pub fn at_m(&self, m: f64) -> f64 {
        STAND + self.times[((m / STEP).round() as usize).min(self.times.len() - 1)]
    }
}

pub fn session(drive: &Drive) -> Session {
    let p = drive.profile();
    let len = track_length();
    let n = p.v.len();
    // time at each distance step, from the start of driving
    let mut t = vec![0.0; n];
    for i in 1..n {
        let v = (p.v[i] + p.v[i - 1]) / 2.0;
        t[i] = t[i - 1] + if v > 1e-6 { STEP / v } else { (2.0 * STEP / ACCEL).sqrt() };
    }
    let at_s = |s: f64| STAND + t[((s / STEP).round() as usize).min(n - 1)];
    let laps = [(at_s(len), at_s(2.0 * len)), (at_s(2.0 * len), at_s(3.0 * len))];
    let drive_end = STAND + t[n - 1];
    let duration = drive_end + 10.0;
    // the state at a file time: distance index and whether the car moves
    let index = |time: f64| -> Option<usize> {
        if time < STAND || time > drive_end {
            return None;
        }
        let x = time - STAND;
        Some(t.partition_point(|v| *v < x).min(n - 1))
    };
    let sample = |rate: u16, f: &dyn Fn(Option<usize>, f64) -> f64| -> Vec<f32> {
        let count = (duration * rate as f64) as usize;
        (0..count).map(|k| {
            let time = k as f64 / rate as f64;
            f(index(time), time) as f32
        }).collect()
    };
    let v = |i: Option<usize>| i.map_or(0.0, |i| p.v[i]);
    let k = |i: Option<usize>| i.map_or(0.0, |i| kappa(i as f64 * STEP));
    let braking = |i: Option<usize>| i.is_some_and(|i| p.braking[i]);
    let accelerating = |i: Option<usize>| i.is_some_and(|i| i + 1 < n && p.v[i + 1] > p.v[i] + 1e-9);
    let s_of = |i: Option<usize>| i.map_or(0.0, |i| i as f64 * STEP);
    let locking = |i: Option<usize>| drive.lock && i.is_some_and(|i| p.late_zone[i] && p.braking[i]);
    let mut chans = vec![
        chan("SPEED", "m/s", 60, sample(60, &|i, _| v(i))),
        chan("THROTTLE", "%", 60, sample(60, &|i, _| if braking(i) || i.is_none() { 0.0 } else if accelerating(i) { 100.0 } else { 30.0 })),
        chan("BRAKE", "%", 60, sample(60, &|i, _| if braking(i) { 90.0 } else { 0.0 })),
        chan("STEERANGLE", "deg", 60, sample(60, &|i, _| 13.0 * (2.7 * k(i)).atan().to_degrees())),
        chan("ROTY", "rad/s", 20, sample(20, &|i, _| v(i) * k(i))),
        chan("G_LAT", "m/s2", 20, sample(20, &|i, _| v(i) * v(i) * k(i) / G)),
        chan("GEAR", "no", 20, sample(20, &|i, _| if i.is_none() { 0.0 } else { (1.0 + (v(i) / 13.0).floor()).min(6.0) })),
        chan("FUEL", "L", 20, sample(20, &|i, _| 60.0 - s_of(i) / 1000.0)),
        chan("AIR_TEMP", "C", 20, sample(20, &|_, _| 21.0)),
        chan("ROAD_TEMP", "C", 20, sample(20, &|_, _| 30.0)),
    ];
    for (w, wheel) in ["LF", "RF", "LR", "RR"].iter().enumerate() {
        let front = w < 2;
        let name = |prefix: &str| -> &'static str { Box::leak(format!("{prefix}{wheel}").into_boxed_str()) };
        // pressures rise from 24 to 27.5 psi over the first 2,000 m, then stay
        chans.push(Chan { name: name("TYRE_PRESS_"), unit: "psi", rate: 20, values: sample(20, &|i, _| 24.0 + 3.5 * (s_of(i) / 2000.0).min(1.0) + 0.1 * w as f64), i16_dec: Some(2) });
        chans.push(chan(name("TYRE_TAIR_"), "C", 20, sample(20, &|_, _| 95.0)));
        for side in ["L_SURF_TEMP_", "C_SURF_TEMP_", "R_SURF_TEMP_"] {
            chans.push(chan(name(side), "C", 60, sample(60, &|_, _| if side == "C_SURF_TEMP_" { 88.0 } else { 85.0 })));
        }
        chans.push(chan(name("BRAKE_TEMP_"), "C", 20, sample(20, &|i, _| if !front { 300.0 } else if braking(i) { 750.0 } else { 500.0 })));
        chans.push(chan(name("BRAKE_TORQUE_"), "Nm", 60, sample(60, &|i, _| if !braking(i) { 0.0 } else if front { 3450.0 } else { 1550.0 })));
        chans.push(chan(name("WHEEL_SR_"), "..", 100, sample(100, &|i, _| if front && locking(i) { -0.2 } else { 0.0 })));
    }
    chans.push(chan("ABS", "..", 20, sample(20, &|_, _| 0.0)));
    Session { chans, duration, laps, times: t }
}

/// The local lap list of the two timed laps when the file starts at `start_ms` (unix ms): one run, as
/// [`super::laps::entry`] chains them.
pub fn local_laps(session: &Session, start_ms: i64) -> Vec<LocalLap> {
    session.laps.iter().map(|(a, b)| LocalLap {
        end_ms: start_ms + (b * 1000.0).round() as i64,
        lap_time_ms: ((b - a) * 1000.0).round() as i64,
        valid: true,
        pit: false,
        car: "dd_test_car".into(),
        preset: Some("preset_dd_test_car_stage1".into()),
        track: "Test Ring".into(),
        layout: "GP".into(),
        session_start_ms: start_ms + (session.laps[0].0 * 1000.0).round() as i64,
    }).collect()
}

/// One corner by hand on a 500 m grid, a point every metre, for the smoothness: the car slows from 40 to 20 m/s at
/// the apex (250 m) and speeds up again, corners at 1 g from 100 to 400 m, turns in to 60° of steering from 100 to
/// 150 m and unwinds from 350 to 400 m; the throttle comes at the apex, rises to 100 % at 300 m. `correction` takes
/// 10° off the steering from 195 to 200 m and puts it back by 210 m, `lift` lifts the throttle by 30 % from 280 to
/// 285 m, `jitter` wobbles the steering with that rate in °/s at most (a sine of 0.25 s).
pub fn corner_grid(correction: bool, lift: bool, jitter: f64) -> (Grid, Map) {
    let n = 501;
    let speed: Vec<f64> = (0..n).map(|i| 20.0 + 20.0 * (i as f64 - 250.0).abs() / 250.0).collect();
    let mut t = vec![0.0; n];
    for i in 1..n {
        t[i] = t[i - 1] + 2.0 / (speed[i] + speed[i - 1]);
    }
    let period = 0.25;
    let steer: Vec<f64> = (0..n)
        .map(|i| {
            let m = i as f64;
            let base = match m {
                m if m < 100.0 => 0.0,
                m if m < 150.0 => 60.0 * (m - 100.0) / 50.0,
                m if m < 350.0 => 60.0,
                m if m < 400.0 => 60.0 * (400.0 - m) / 50.0,
                _ => 0.0,
            };
            let dip = match m {
                m if correction && (195.0..200.0).contains(&m) => 10.0 * (m - 195.0) / 5.0,
                m if correction && (200.0..205.0).contains(&m) => 10.0,
                m if correction && (205.0..210.0).contains(&m) => 10.0 * (210.0 - m) / 5.0,
                _ => 0.0,
            };
            base - dip + jitter * period / std::f64::consts::TAU * (std::f64::consts::TAU * t[i] / period).sin()
        })
        .collect();
    let throttle: Vec<f64> = (0..n)
        .map(|i| {
            let m = i as f64;
            let base = if m < 250.0 { 0.0 } else { (100.0 * (m - 250.0) / 50.0).min(100.0) };
            if lift && (280.0..285.0).contains(&m) { base - 30.0 } else { base }
        })
        .collect();
    let g = Grid {
        t,
        throttle,
        brake: vec![0.0; n],
        steer,
        gear: vec![3.0; n],
        g_lat: (0..n).map(|i| if (100..400).contains(&i) { 1.0 } else { 0.0 }).collect(),
        yaw: vec![0.0; n],
        slip: std::array::from_fn(|_| vec![0.0; n]),
        speed,
    };
    let corner = MapCorner { n: 1, start_m: 0.0, end_m: 500.0, apex_m: 250.0, turn_in_m: 100.0, turn_out_m: 400.0, direction: Direction::L, radius_m: 40.0 };
    (g, Map { version: 1, frozen: false, agreed: vec![], length_m: 500.0, corners: vec![corner] })
}
