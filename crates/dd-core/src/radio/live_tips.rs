//! Live tips ("Live-Tipps", free, no AI): while the driver drives, the team radio compares each corner he has just
//! driven with his own best lap and says one short templated tip on the straight after it ("Hatzenbach: acht
//! Stundenkilometer weniger Kurventempo als in deiner Bestrunde, du bremst fünfzehn Meter zu früh.").
//!
//! **The reference** is a lap the radio recorded itself from the shared memory, at the shell's 20 Hz: the race
//! engineer's store cannot serve, because its corners and reference numbers lie on the distance of a MoTeC lap, and
//! MoTeC has no position at all (research/2026-10-05-club-build/telemetry-core.md §4: dead reckoning misses by 160 m
//! on the Nordschleife). The best valid lap of a car, preset, track and layout is kept next to the engineer's own
//! references (`<config>/engineer/live/`, [`load_reference`], [`save_reference`]), so the next session starts with
//! it; a lap driven faster over all of its corners takes its place.
//!
//! **One definition of a corner** with the engineer: the corners are found on the reference lap with the engineer's
//! [`corners::detect`] (the same limit, length and merging) on its curvature averaged over ±20 m as
//! [`corners::smooth`] averages it; only the curvature comes from the world positions (the change of heading over
//! distance) instead of ROTY / SPEED, because `localAngularVel` is not verified for EVO yet and positions are. Both
//! laps are measured with the engineer's [`measure::corner`] (brake point where the pedal first passed 5 %, walking
//! back from the hardest braking; lowest speed within 30 m of the turn; throttle pick-up above 40 %), on the
//! reference's distance: the live position is projected onto the reference's line, so a brake point compares
//! metre for metre with the reference's.
//!
//! **What a corner is judged over**: to be said on the straight after it, a corner is judged when the car is 30 m
//! past its turn-out (or at the middle of the straight to the next corner, when that comes first), over the stretch
//! from where the corner before was judged: its braking zone, the turn and the start of its exit. The engineer's
//! segments run on to the middle of the next straight; the exit beyond those 30 m is not in a live tip.
//!
//! **Accuracy at 20 Hz**: a brake point lies between two samples, so each lap's is known to one sample's travel
//! (v / 20: 3 m at 216 km/h, 4 m at 288 km/h) and the difference of two laps to the same; tips round metres to 5
//! and need 10 m. The lowest speed is flat where it is reached: to well under 1 km/h; tips need 4 km/h, the
//! engineer's threshold. Times over a stretch: to a few milliseconds; tips need the corner to have cost 0.1 s.
//!
//! **Which tip, when**: at most one per corner, and for a run of corners without a straight long enough to speak
//! between them (4 s on the reference) one after the run's last: the corner that cost the most time, the engineer's
//! ranking (the platform's "biggest losses"), with at most two of its findings (corner speed, brake point, throttle
//! pick-up). A corner clearly better than the reference (0.2 s) gets a word of praise, at most every three minutes.
//! Never more than one tip in 20 s, never the same corner twice in a row, nothing on a circuit's out lap, in the pit
//! lane, in a corner where the lap became invalid or that cost more than 2 s (an off, a spin, an aborted lap), or
//! with another car within 30 m (the spotter's alongside included). The queue says it on the next straight
//! ([`Priority::Engineer`]) and drops it after 8 s rather than say it late into the next corner.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::frame::Frame;
use super::phrases::{de_number, en_number};
use super::queue::{Call, Priority};
use crate::engineer::corners::{self, Detected, Fit, Map};
use crate::engineer::cut::{Grid, MIN_LAP_M};
use crate::engineer::measure::{self, CornerMeasure, APEX_MARGIN};
use crate::engineer::{evaluate, store, Direction};

/// The phrase key of every live tip: its sentence is built here and spoken live.
pub const KEY: &str = "live_tip";
/// The engineer's folder holds the live references in this folder.
const DIR: &str = "live";
const FILE_VERSION: u32 = 1;
/// A brake point is looked for this far (m) before the stretch a corner is judged over.
const LOOKBACK_M: f64 = 200.0;
/// The car is on the reference's line within this distance (m): wider than a track, narrower than two tracks.
const ON_LINE_M: f64 = 25.0;
/// The heading at a metre of the line is that of the chord over ±5 m, so position noise hardly turns it.
const CHORD_M: usize = 5;
/// A lap's first and last point this close (m) make a closed line (a circuit's lap from line to line).
const CLOSED_M: f64 = 30.0;
/// A car that moved further than this (m) plus twice its speed's travel from one sample to the next was put
/// elsewhere (back to the pits, a restart).
const JUMP_M: f64 = 20.0;
/// From one sample to the next the car moves along the reference's line by its speed's travel, within this (m)
/// plus a share of it (the driver's line differs from the reference's: 12 % on the inside of a 40 m hairpin).
const PROGRESS_M: f64 = 3.0;
const PROGRESS_SHARE: f64 = 0.3;
/// The frames of the last seconds are kept, to put the car on a new reference's line from before its line: the lap
/// after a new best is judged from its first corner on.
const HISTORY_S: f64 = 15.0;
/// A lap is recorded for at most this many samples (an hour at 20 Hz); a longer one cannot become a reference.
const MAX_SAMPLES: usize = 72_000;

/// The thresholds of the tips.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// A corner's findings are said only when it cost at least this much (s) against the reference (the engineer
    /// asks 0.05 s and the driver's own spread; live there is no spread, so more)
    pub cost_s: f64,
    /// Brake point earlier or later (m): the engineer's 8 m plus the 20 Hz sampling
    pub brake_m: f64,
    /// Lower corner speed (km/h): the engineer's
    pub min_speed_kmh: f64,
    /// Later throttle pick-up (m): the engineer's
    pub throttle_m: f64,
    /// A corner this much (s) faster than the reference is praised
    pub better_s: f64,
    /// A corner that cost more than this (s) was an off, a spin or an aborted lap: nothing to say
    pub max_loss_s: f64,
    /// Seconds between two tips at least
    pub gap_s: f64,
    /// Seconds between two words of praise at least
    pub praise_gap_s: f64,
    /// A tip waits for the end of a run of corners when the reference reaches the next corner's brake point (or
    /// turn-in) less than this many seconds after judging one
    pub straight_s: f64,
    /// A tip not said within this many seconds is dropped
    pub expires_s: f64,
    /// A corner with another car within this distance (m) is not judged
    pub traffic_m: f64,
    /// Every lap starts at the pit exit (Touristenfahrten: the loop is the lap, the counter ticks at the end split);
    /// on a circuit the lap from the pit exit is the out lap
    pub laps_start_at_pit_exit: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cost_s: 0.1,
            brake_m: 10.0,
            min_speed_kmh: measure::SPEED_KMH,
            throttle_m: measure::THROTTLE_M,
            better_s: 0.2,
            max_loss_s: 2.0,
            gap_s: 20.0,
            praise_gap_s: 180.0,
            straight_s: 4.0,
            expires_s: 8.0,
            traffic_m: 30.0,
            laps_start_at_pit_exit: false,
        }
    }
}

impl Config {
    /// The defaults for a track and layout as the game names them.
    pub fn for_layout(track: &str, layout: &str) -> Self {
        Self { laps_start_at_pit_exit: evaluate::touristenfahrten(track, layout), ..Self::default() }
    }
}

/// A tip for the queue, with its sentence to speak.
#[derive(Debug, Clone, PartialEq)]
pub struct Tip {
    pub call: Call,
    pub text: String,
}

/// One sample of a recorded lap, in the frame's units (m/s, pedals 0 to 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub t: f64,
    pub pos: [f32; 3],
    pub speed: f32,
    pub brake: f32,
    pub throttle: f32,
}

/// A point on the reference's distance: metres, seconds, m/s, pedals in % as the engineer's grids have them.
#[derive(Debug, Clone, Copy)]
struct Point {
    s: f64,
    t: f64,
    v: f64,
    brake: f64,
    throttle: f64,
}

/// The lap every corner is compared with: as recorded, and what is derived from it.
#[derive(Debug, Clone)]
pub struct Reference {
    samples: Vec<Sample>,
    /// The line at each metre from the lap's start
    path: Vec<[f64; 3]>,
    closed: bool,
    /// The corners, each judged over the stretch from the corner before's judging point to its own
    map: Map,
    corners: Vec<RefCorner>,
}

#[derive(Debug, Clone)]
struct RefCorner {
    measure: CornerMeasure,
    /// Seconds from judging this corner to the next one's brake point (or turn-in) on the reference
    gap_s: f64,
}

/// The engineer's grid of points every metre from `from` to `to`, interpolated between the points (sorted by s).
fn grid_of(points: &[Point], from: usize, to: usize) -> Grid {
    let mut g = Grid::default();
    let mut k = 0;
    for m in from..=to {
        let m = m as f64;
        while k + 2 < points.len() && points[k + 1].s < m {
            k += 1;
        }
        let (a, b) = (points[k], points[(k + 1).min(points.len() - 1)]);
        let f = if b.s > a.s { ((m - a.s) / (b.s - a.s)).clamp(0.0, 1.0) } else { 1.0 };
        let at = |x: f64, y: f64| x + (y - x) * f;
        g.t.push(at(a.t, b.t));
        g.speed.push(at(a.v, b.v));
        g.brake.push(at(a.brake, b.brake));
        g.throttle.push(at(a.throttle, b.throttle));
    }
    let n = g.t.len();
    (g.steer, g.gear, g.g_lat, g.yaw) = (vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n]);
    g.slip = std::array::from_fn(|_| vec![0.0; n]);
    g
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn wide(p: [f32; 3]) -> [f64; 3] {
    p.map(f64::from)
}

fn point(s: f64, x: &Sample) -> Point {
    Point { s, t: x.t, v: x.speed as f64, brake: x.brake as f64 * 100.0, throttle: x.throttle as f64 * 100.0 }
}

impl Reference {
    /// The reference made from a recorded lap; None when it is no lap (shorter than 500 m) or has no corner.
    pub fn from_samples(samples: Vec<Sample>) -> Option<Reference> {
        let mut points = Vec::with_capacity(samples.len());
        let mut s = 0.0;
        for (i, x) in samples.iter().enumerate() {
            if i > 0 {
                s += distance(wide(samples[i - 1].pos), wide(x.pos));
            }
            points.push(point(s, x));
        }
        if s < MIN_LAP_M {
            return None;
        }
        let last = s.floor() as usize;
        let grid = grid_of(&points, 0, last);
        // the line at each metre
        let mut path = Vec::with_capacity(last + 1);
        let mut k = 0;
        for m in 0..=last {
            let m = m as f64;
            while k + 2 < points.len() && points[k + 1].s < m {
                k += 1;
            }
            let (a, b) = (points[k].s, points[k + 1].s);
            let f = if b > a { ((m - a) / (b - a)).clamp(0.0, 1.0) } else { 1.0 };
            let (p, q) = (wide(samples[k].pos), wide(samples[k + 1].pos));
            path.push([p[0] + (q[0] - p[0]) * f, p[1] + (q[1] - p[1]) * f, p[2] + (q[2] - p[2]) * f]);
        }
        // the curvature: the change of heading in the x/z plane (Y is up), > 0 to the left, which in the radio's
        // world (right of a direction (x, z) is (-z, x)) turns the angle atan2(z, x) down
        let heading = |j: usize| {
            let (a, b) = (path[j.saturating_sub(CHORD_M)], path[(j + CHORD_M).min(last)]);
            (b[2] - a[2]).atan2(b[0] - a[0])
        };
        let raw: Vec<f64> = (0..=last)
            .map(|i| {
                if i == last || !corners::turning_speed(grid.speed[i]) {
                    return 0.0;
                }
                let turn = heading(i + 1) - heading(i);
                -(turn + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) + std::f64::consts::PI
            })
            .collect();
        let found = corners::detect(&corners::smooth(&raw));
        if found.is_empty() {
            return None;
        }
        let mut map = Map::from_lap(&found, &grid.speed, 1);
        // judged 30 m past the turn-out, or at the segment's end when that comes first; from there the next
        let mut from = 0.0;
        for c in &mut map.corners {
            c.end_m = c.end_m.min(c.turn_out_m + APEX_MARGIN);
            c.start_m = from;
            from = c.end_m;
        }
        let whole = Fit { scale: 1.0, offset: 0.0, matched: 0 };
        let measures: Vec<CornerMeasure> = (0..map.corners.len()).map(|c| measure::corner(&grid, &map, &whole, c)).collect();
        let corners = measures
            .iter()
            .enumerate()
            .map(|(c, m)| {
                let gap_s = map.corners.get(c + 1).map_or(f64::INFINITY, |next| {
                    let to = measures[c + 1].point_m.unwrap_or(next.turn_in_m).max(map.corners[c].end_m);
                    grid.time_at(to) - grid.time_at(map.corners[c].end_m)
                });
                RefCorner { measure: m.clone(), gap_s }
            })
            .collect();
        let closed = distance(path[0], path[last]) < CLOSED_M;
        Some(Reference { samples, path, closed, map, corners })
    }

    /// How many corners the lap has, and its length on the line (m): for the shell's log.
    pub fn corners(&self) -> usize {
        self.corners.len()
    }

    pub fn length_m(&self) -> f64 {
        self.length()
    }

    /// The lap's length on the line (m).
    fn length(&self) -> f64 {
        (self.path.len() - 1) as f64
    }

    /// Seconds over all judged stretches.
    fn time_s(&self) -> f64 {
        self.corners.iter().map(|c| c.measure.segment_s).sum()
    }

    /// The metre of the line nearest to `pos` among `metres` (wrapping round a closed line), its distance, and the
    /// position along the line to a fraction of a metre.
    fn nearest(&self, pos: [f64; 3], metres: impl Iterator<Item = i64>) -> Option<(f64, f64)> {
        let n = if self.closed { self.path.len() as i64 - 1 } else { self.path.len() as i64 };
        let index = |i: i64| if self.closed { i.rem_euclid(n) as usize } else { i.clamp(0, n - 1) as usize };
        let (i, d) = metres.map(|i| (index(i), distance(pos, self.path[index(i)]))).min_by(|a, b| a.1.total_cmp(&b.1))?;
        let (a, b) = (self.path[index(i as i64 - 1)], self.path[index(i as i64 + 1)]);
        let along = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let norm = (along[0] * along[0] + along[1] * along[1] + along[2] * along[2]).sqrt();
        let p = self.path[i];
        let off = if norm > 0.0 { ((pos[0] - p[0]) * along[0] + (pos[1] - p[1]) * along[1] + (pos[2] - p[2]) * along[2]) / norm } else { 0.0 };
        Some((i as f64 + off.clamp(-0.5, 0.5), d))
    }

    /// The corners as the engineer's corner fit takes them.
    fn detected(&self, mirrored: bool) -> Vec<Detected> {
        let flip = |d: Direction| match (d, mirrored) {
            (Direction::L, true) => Direction::R,
            (Direction::R, true) => Direction::L,
            (d, false) => d,
        };
        self.map.corners.iter().map(|c| Detected { turn_in: c.turn_in_m, turn_out: c.turn_out_m, direction: flip(c.direction), peak: 1.0 / c.radius_m }).collect()
    }

    /// The engineer's corner number of each corner, by the engineer's map of the track: the number of the map's
    /// segment its apex falls in, once the corners are fitted onto the map as the engineer fits a lap (the shift a
    /// Touristenfahrten loop needs included, and the corners' turns read either way: the sign of the world's
    /// handedness is not verified). None when the map fits fewer than half of the corners.
    fn engineer_numbers(&self, map: &Map) -> Option<Vec<u32>> {
        let extra = (self.length() - map.length_m).max(0.0).ceil() as i32;
        let (found, fit) = [false, true]
            .into_iter()
            .map(|mirrored| {
                let found = self.detected(mirrored);
                let fit = corners::fit_within(&found, map, -(extra + 300), 300);
                (found, fit)
            })
            .max_by_key(|(_, fit)| fit.matched)?;
        if !map.usable(&found, &fit) {
            return None;
        }
        Some(
            self.map
                .corners
                .iter()
                .map(|c| {
                    let at = fit.to_map(c.apex_m);
                    let inside = map.corners.iter().find(|m| m.start_m <= at && at < m.end_m);
                    inside.or_else(|| map.corners.iter().min_by(|a, b| (a.apex_m - at).abs().total_cmp(&(b.apex_m - at).abs()))).map_or(0, |m| m.n)
                })
                .collect(),
        )
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    /// t, x, y, z, speed, brake, throttle
    samples: Vec<[f64; 7]>,
}

fn reference_path(dir: &Path, car: &str, preset: Option<&str>, track: &str, layout: &str) -> PathBuf {
    dir.join(DIR).join(format!("{}.json", store::refs_key(car, preset, track, layout)))
}

/// The live reference of a car, preset, track and layout from the engineer's folder (`dir`), when there is one.
pub fn load_reference(dir: &Path, car: &str, preset: Option<&str>, track: &str, layout: &str) -> Option<Reference> {
    let stored: Stored = serde_json::from_slice(&fs::read(reference_path(dir, car, preset, track, layout)).ok()?).ok()?;
    (stored.version == FILE_VERSION).then_some(())?;
    let samples = stored
        .samples
        .iter()
        .map(|x| Sample { t: x[0], pos: [x[1] as f32, x[2] as f32, x[3] as f32], speed: x[4] as f32, brake: x[5] as f32, throttle: x[6] as f32 })
        .collect();
    Reference::from_samples(samples)
}

/// Keeps the live reference of a car, preset, track and layout in the engineer's folder (`dir`); forgetting the
/// engineer's data deletes it with the folder.
pub fn save_reference(dir: &Path, car: &str, preset: Option<&str>, track: &str, layout: &str, reference: &Reference) -> io::Result<()> {
    let path = reference_path(dir, car, preset, track, layout);
    fs::create_dir_all(path.parent().unwrap())?;
    let samples = reference.samples.iter().map(|x| [x.t, x.pos[0] as f64, x.pos[1] as f64, x.pos[2] as f64, x.speed as f64, x.brake as f64, x.throttle as f64]).collect();
    fs::write(path, serde_json::to_vec(&Stored { version: FILE_VERSION, samples }).map_err(io::Error::other)?)
}

/// The lap being driven, as the game counts laps.
#[derive(Debug, Default)]
struct Lap {
    count: i32,
    samples: Vec<Sample>,
    /// It started where a lap starts: at a count, or at the pit exit when every lap starts there
    from_boundary: bool,
    /// A circuit's lap from the pit exit
    out_lap: bool,
    /// Valid, out of the pit lane and live in every sample so far, and the car never moved elsewhere
    clean: bool,
    /// The seconds of each of the reference's stretches as this lap drove them
    times: Vec<Option<f64>>,
}

impl Lap {
    fn new(count: i32, from_boundary: bool, out_lap: bool, corners: usize) -> Lap {
        Lap { count, samples: Vec::new(), from_boundary, out_lap, clean: true, times: vec![None; corners] }
    }
}

/// The car's way along the reference's line since it was last found on it.
#[derive(Debug)]
struct Pass {
    /// Laps the closed line went round since the pass began
    wraps: i64,
    /// Where the car is on the line (m), counting the laps gone round
    s: f64,
    t: f64,
    start: f64,
    points: Vec<Point>,
    /// The lap (of `wraps`) and corner to judge next; None at the end of an open line
    next: Option<(i64, usize)>,
    /// Where (s) a car was within the traffic distance last, and where the lap was last invalid
    traffic_at: f64,
    invalid_at: f64,
}

/// What a judged corner gave.
#[derive(Debug, Clone, PartialEq)]
struct Judged {
    corner: usize,
    dt: f64,
    hints: Vec<Hint>,
    /// Nothing may be said about it (out lap, traffic, invalid, an off)
    silent: bool,
}

/// What a tip says about a corner, with its number rounded as said: km/h whole, metres to 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hint {
    Slower(u64),
    BrakeEarlier(u64),
    BrakeLater(u64),
    ThrottleLater(u64),
}

/// A corner as the radio names it: the platform's name of the engineer's corner, or its number.
#[derive(Debug, Clone, PartialEq)]
enum Label {
    Name(String),
    Number(u32),
}

/// The live tips of one car on one track: [`LiveTips::step`] with every frame.
#[derive(Debug)]
pub struct LiveTips {
    config: Config,
    lang: String,
    reference: Option<Reference>,
    new_reference: bool,
    naming: Option<(Map, BTreeMap<u32, String>)>,
    labels: Vec<Label>,
    lap: Option<Lap>,
    pass: Option<Pass>,
    last: Option<([f64; 3], f64)>,
    in_pit: bool,
    history: VecDeque<Frame>,
    run: Vec<Judged>,
    /// When the last tip was handed out, and about which corner
    last_tip: Option<(f64, usize)>,
    last_praise: f64,
}

impl LiveTips {
    pub fn new(config: Config, lang: &str) -> Self {
        Self {
            config,
            lang: lang.to_string(),
            reference: None,
            new_reference: false,
            naming: None,
            labels: Vec::new(),
            lap: None,
            pass: None,
            last: None,
            in_pit: false,
            history: VecDeque::new(),
            run: Vec::new(),
            last_tip: None,
            last_praise: f64::NEG_INFINITY,
        }
    }

    /// The reference for this car and track, from [`load_reference`] when the shell found one.
    pub fn set_reference(&mut self, reference: Option<Reference>) {
        self.reference = reference;
        self.restart();
    }

    /// The engineer's corner map of this track and layout ([`corners::load`]) and the platform's corner names by
    /// its corner numbers (`cornerNames` of the platform's track data), so the tips name the corners as the
    /// engineer's page does; without them a corner is called by its number on the reference.
    pub fn set_names(&mut self, map: Option<Map>, names: BTreeMap<u32, String>) {
        self.naming = map.map(|map| (map, names));
        self.label();
    }

    pub fn reference(&self) -> Option<&Reference> {
        self.reference.as_ref()
    }

    /// The reference once after a lap became it, for the shell to keep with [`save_reference`].
    pub fn take_new_reference(&mut self) -> Option<&Reference> {
        if std::mem::take(&mut self.new_reference) { self.reference.as_ref() } else { None }
    }

    fn restart(&mut self) {
        self.pass = None;
        self.run.clear();
        let corners = self.reference.as_ref().map_or(0, |r| r.corners.len());
        if let Some(lap) = &mut self.lap {
            lap.times = vec![None; corners];
        }
        self.label();
    }

    fn label(&mut self) {
        let Some(r) = &self.reference else {
            self.labels.clear();
            return;
        };
        let numbers = self.naming.as_ref().and_then(|(map, names)| Some((r.engineer_numbers(map)?, names)));
        self.labels = match numbers {
            Some((numbers, names)) => numbers.into_iter().map(|n| names.get(&n).filter(|name| !unnamed(name)).map_or(Label::Number(n), |name| Label::Name(name.clone()))).collect(),
            None => (1..=r.corners.len() as u32).map(Label::Number).collect(),
        };
    }

    /// One sample (20 a second); answers a tip when one is due.
    pub fn step(&mut self, frame: &Frame) -> Option<Tip> {
        let own = &frame.own;
        let pos = wide(own.pos);
        let pit = own.in_pit_lane || own.in_pit_box;
        let jumped = self.last.is_some_and(|(p, t)| distance(p, pos) > JUMP_M + 2.0 * own.speed as f64 * (frame.t - t).abs());
        self.last = Some((pos, frame.t));
        self.count(frame, pit);
        if !frame.live || pit || jumped {
            self.in_pit = pit;
            self.pass = None;
            self.run.clear();
            self.history.clear();
            if let Some(lap) = &mut self.lap {
                lap.clean = false;
                lap.samples.clear();
            }
            return None;
        }
        if self.in_pit {
            // the pit exit: a lap starts here where every lap does, else this is the out lap
            self.in_pit = false;
            let start = self.config.laps_start_at_pit_exit;
            self.lap = Some(Lap::new(own.lap_count, start, !start, self.reference.as_ref().map_or(0, |r| r.corners.len())));
        }
        let lap = self.lap.as_mut().expect("counted above");
        if lap.clean && lap.samples.len() < MAX_SAMPLES {
            lap.samples.push(Sample { t: frame.t, pos: own.pos, speed: own.speed, brake: own.brake, throttle: own.throttle });
        } else {
            lap.clean = false;
            lap.samples.clear();
        }
        while self.history.front().is_some_and(|f| frame.t - f.t > HISTORY_S) {
            self.history.pop_front();
        }
        self.history.push_back(Frame { others: Vec::new(), ..frame.clone() });
        let mut tip = None;
        for judged in self.follow(frame) {
            match judged {
                None => self.run.clear(),
                Some(judged) => {
                    let corner = judged.corner;
                    self.run.push(judged);
                    let r = self.reference.as_ref().expect("judged against it");
                    if corner + 1 == r.corners.len() || r.corners[corner].gap_s >= self.config.straight_s {
                        tip = tip.or(self.choose(frame.t));
                        self.run.clear();
                    }
                }
            }
        }
        tip
    }

    /// Follows the game's lap count: a lap counted ends the lap being driven, and makes it the reference when it
    /// was a whole valid lap faster over all of the reference's stretches (or there was none yet).
    fn count(&mut self, frame: &Frame, pit: bool) {
        let own = &frame.own;
        let corners = self.reference.as_ref().map_or(0, |r| r.corners.len());
        let Some(lap) = &mut self.lap else {
            // joined midway, or the car stands in the pit lane
            self.lap = Some(Lap::new(own.lap_count, false, false, corners));
            self.in_pit = pit;
            return;
        };
        if own.lap_count == lap.count + 1 {
            // the validity of this sample is the counted lap's (as the lap recorder reads it)
            let done = std::mem::replace(lap, Lap::new(own.lap_count, true, false, corners));
            if done.from_boundary && done.clean && own.lap_valid && frame.live {
                self.consider(done);
            }
        } else if own.lap_count != lap.count {
            // a new session: no lap boundary is known
            *lap = Lap::new(own.lap_count, false, false, corners);
            self.pass = None;
            self.run.clear();
        } else if !own.lap_valid {
            lap.clean = false;
        }
    }

    fn consider(&mut self, done: Lap) {
        let better = match &self.reference {
            None => true,
            Some(r) => done.times.len() == r.corners.len() && done.times.iter().all(Option::is_some) && done.times.iter().flatten().sum::<f64>() < r.time_s(),
        };
        if !better {
            return;
        }
        if let Some(candidate) = Reference::from_samples(done.samples) {
            self.reference = Some(candidate);
            self.new_reference = true;
            self.restart();
            // the car on the new line from the last seconds on, so its next corners are judged whole
            for frame in std::mem::take(&mut self.history) {
                self.follow(&frame);
                self.history.push_back(frame);
            }
            self.run.clear();
        }
    }

    /// Puts the car on the reference's line and judges each corner whose stretch it completed: Some for a corner
    /// judged, None for one it did not drive whole (the run of corners breaks there).
    fn follow(&mut self, frame: &Frame) -> Vec<Option<Judged>> {
        let Some(r) = &self.reference else { return Vec::new() };
        let own = &frame.own;
        let pos = wide(own.pos);
        let v = own.speed as f64;
        let length = r.length();
        // near where the car was, else anywhere on the line
        let near = self.pass.as_ref().and_then(|p| {
            let at = p.s - p.wraps as f64 * length;
            let reach = (v * (frame.t - p.t).abs() * 2.0).ceil() as i64 + 10;
            r.nearest(pos, (at.floor() as i64 - 10)..=(at.ceil() as i64 + reach)).filter(|(_, d)| *d <= ON_LINE_M)
        });
        let found = near.or_else(|| {
            self.pass = None;
            r.nearest(pos, 0..r.path.len() as i64).filter(|(_, d)| *d <= ON_LINE_M)
        });
        let Some((s, _)) = found else {
            self.pass = None;
            self.run.clear();
            return Vec::new();
        };
        let next_corner = |s: f64| r.map.corners.iter().position(|c| c.end_m > s);
        let fresh = |s: f64| Pass {
            wraps: 0,
            s,
            t: frame.t,
            start: s,
            points: Vec::new(),
            next: next_corner(s).map(|c| (0, c)).or(r.closed.then_some((1, 0))),
            traffic_at: f64::NEG_INFINITY,
            invalid_at: f64::NEG_INFINITY,
        };
        let mut pass = match self.pass.take() {
            None => fresh(s),
            Some(mut p) => {
                let before = p.s - p.wraps as f64 * length;
                if r.closed && s - before < -length / 2.0 {
                    p.wraps += 1;
                } else if r.closed && s - before > length / 2.0 {
                    p.wraps -= 1;
                }
                let unwrapped = s + p.wraps as f64 * length;
                let (moved, expected) = (unwrapped - p.s, v * (frame.t - p.t));
                if moved < -1.0 || (moved - expected).abs() > PROGRESS_M + PROGRESS_SHARE * expected {
                    // not the way the car drives along the line (the wrong way, another track, a jump)
                    self.run.clear();
                    fresh(s)
                } else {
                    (p.s, p.t) = (unwrapped.max(p.s), frame.t);
                    p
                }
            }
        };
        let x = Sample { t: frame.t, pos: own.pos, speed: own.speed, brake: own.brake, throttle: own.throttle };
        pass.points.push(point(pass.s, &x));
        if frame.others.iter().any(|c| distance(wide(c.pos), pos) < self.config.traffic_m) {
            pass.traffic_at = pass.s;
        }
        if !own.lap_valid {
            pass.invalid_at = pass.s;
        }
        let out_lap = self.lap.as_ref().is_some_and(|l| l.out_lap);
        let mut judged = Vec::new();
        while let Some((lap_of_pass, c)) = pass.next {
            let base = lap_of_pass as f64 * length;
            let stretch = &r.map.corners[c];
            let (start, end) = (base + stretch.start_m, base + stretch.end_m);
            if pass.s < end + 1.0 {
                break;
            }
            pass.next = if c + 1 < r.corners.len() { Some((lap_of_pass, c + 1)) } else { r.closed.then_some((lap_of_pass + 1, 0)) };
            if pass.start > start {
                judged.push(None);
                continue;
            }
            let from = pass.start.max(start - LOOKBACK_M).ceil();
            let g = grid_of(&pass.points, from as usize, (end + 1.0).floor() as usize);
            let own_measure = measure::corner(&g, &r.map, &Fit { scale: 1.0, offset: from - base, matched: 0 }, c);
            let reference = &r.corners[c].measure;
            let dt = own_measure.segment_s - reference.segment_s;
            if let Some(lap) = &mut self.lap {
                if let Some(time) = lap.times.get_mut(c) {
                    *time = Some(own_measure.segment_s);
                }
            }
            let silent = out_lap || pass.traffic_at >= start || pass.invalid_at >= start || dt > self.config.max_loss_s;
            judged.push(Some(Judged { corner: c, dt, hints: self.hints(&own_measure, reference, dt), silent }));
        }
        // keep what the next corner's brake point may need
        if let Some((lap_of_pass, c)) = pass.next {
            let keep = lap_of_pass as f64 * length + r.map.corners[c].start_m - LOOKBACK_M - 1.0;
            let drop = pass.points.partition_point(|p| p.s < keep).saturating_sub(1);
            pass.points.drain(..drop);
        }
        self.pass = Some(pass);
        judged
    }

    /// The findings of a corner that cost time, at most two, in the order they are said.
    fn hints(&self, own: &CornerMeasure, reference: &CornerMeasure, dt: f64) -> Vec<Hint> {
        if dt < self.config.cost_s {
            return Vec::new();
        }
        let metres = |d: f64| ((d.abs() / 5.0).round() as u64 * 5).max(5);
        let mut hints = Vec::new();
        let slower = reference.min_kmh - own.min_kmh;
        if slower >= self.config.min_speed_kmh {
            hints.push(Hint::Slower(slower.round() as u64));
        }
        if let (Some(a), Some(b)) = (own.point_m, reference.point_m) {
            if a - b <= -self.config.brake_m {
                hints.push(Hint::BrakeEarlier(metres(a - b)));
            } else if a - b >= self.config.brake_m {
                hints.push(Hint::BrakeLater(metres(a - b)));
            }
        }
        if let (Some(a), Some(b)) = (own.pickup_m, reference.pickup_m) {
            if a - b >= self.config.throttle_m {
                hints.push(Hint::ThrottleLater(metres(a - b)));
            }
        }
        hints.truncate(2);
        hints
    }

    /// The tip at the end of a run of corners: the costliest corner with findings, else now and then a corner
    /// clearly better than the reference.
    fn choose(&mut self, t: f64) -> Option<Tip> {
        if self.last_tip.is_some_and(|(at, _)| t - at < self.config.gap_s) {
            return None;
        }
        let repeat = self.last_tip.map(|(_, c)| c);
        let open: Vec<&Judged> = self.run.iter().filter(|j| !j.silent && Some(j.corner) != repeat).collect();
        let loss = open.iter().filter(|j| !j.hints.is_empty()).max_by(|a, b| a.dt.total_cmp(&b.dt));
        let (corner, text) = match loss {
            Some(j) => (j.corner, tip_text(&self.lang, &self.spoken(j.corner), &j.hints)),
            None if t - self.last_praise >= self.config.praise_gap_s => {
                let j = open.iter().filter(|j| j.dt <= -self.config.better_s).min_by(|a, b| a.dt.total_cmp(&b.dt))?;
                self.last_praise = t;
                (j.corner, praise_text(&self.lang, &self.spoken(j.corner)))
            }
            None => return None,
        };
        self.last_tip = Some((t, corner));
        Some(Tip { call: Call { key: KEY, car: None, priority: Priority::Engineer, at: t, expires: t + self.config.expires_s }, text })
    }

    fn spoken(&self, corner: usize) -> String {
        spoken_label(&self.lang, self.labels.get(corner).unwrap_or(&Label::Number(corner as u32 + 1)))
    }
}

/// A section the map's data names only by its number ("T13") gives no name.
fn unnamed(name: &str) -> bool {
    name.strip_prefix('T').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) || name.trim().is_empty()
}

fn number(lang: &str, n: u64) -> String {
    if lang == "de" { de_number(n) } else { en_number(n) }
}

/// A corner's name as the voice says it: digits spelled out ("Hatzenbach zwei"), an unnamed corner by its number
/// ("Kurve zwölf" / "Turn twelve").
fn spoken_label(lang: &str, label: &Label) -> String {
    match label {
        Label::Number(n) if lang == "de" => format!("Kurve {}", de_number(*n as u64)),
        Label::Number(n) => format!("Turn {}", en_number(*n as u64)),
        Label::Name(name) => {
            let mut out = String::new();
            let mut digits = String::new();
            for c in name.chars().chain(std::iter::once(' ')) {
                if c.is_ascii_digit() {
                    digits.push(c);
                    continue;
                }
                if !digits.is_empty() {
                    out.push_str(&number(lang, digits.parse().unwrap_or(0)));
                    digits.clear();
                }
                out.push(c);
            }
            out.trim_end().to_string()
        }
    }
}

/// A finding as the first clause of a tip (compared with the best lap) and as the second.
fn clause(lang: &str, hint: Hint, first: bool) -> String {
    let de = lang == "de";
    let n = |x: u64| number(lang, x);
    match (hint, de, first) {
        (Hint::Slower(k), true, true) => format!("{} Stundenkilometer weniger Kurventempo als in deiner Bestrunde", n(k)),
        (Hint::Slower(k), true, false) => format!("{} Stundenkilometer weniger Kurventempo", n(k)),
        (Hint::BrakeEarlier(m), true, true) => format!("du bremst {} Meter früher als in deiner Bestrunde", n(m)),
        (Hint::BrakeEarlier(m), true, false) => format!("du bremst {} Meter zu früh", n(m)),
        (Hint::BrakeLater(m), true, true) => format!("du bremst {} Meter später als in deiner Bestrunde", n(m)),
        (Hint::BrakeLater(m), true, false) => format!("du bremst {} Meter zu spät", n(m)),
        (Hint::ThrottleLater(m), true, true) => format!("du gehst {} Meter später aufs Gas als in deiner Bestrunde", n(m)),
        (Hint::ThrottleLater(m), true, false) => format!("du gehst {} Meter zu spät aufs Gas", n(m)),
        (Hint::Slower(k), false, true) => format!("{} kilometres an hour less corner speed than your best lap", n(k)),
        (Hint::Slower(k), false, false) => format!("{} kilometres an hour less corner speed", n(k)),
        (Hint::BrakeEarlier(m), false, true) => format!("you're braking {} metres earlier than your best lap", n(m)),
        (Hint::BrakeEarlier(m), false, false) => format!("you're braking {} metres early", n(m)),
        (Hint::BrakeLater(m), false, true) => format!("you're braking {} metres later than your best lap", n(m)),
        (Hint::BrakeLater(m), false, false) => format!("you're braking {} metres late", n(m)),
        (Hint::ThrottleLater(m), false, true) => format!("you're on the throttle {} metres later than your best lap", n(m)),
        (Hint::ThrottleLater(m), false, false) => format!("you're {} metres late on the throttle", n(m)),
    }
}

/// "Hatzenbach: acht Stundenkilometer weniger Kurventempo als in deiner Bestrunde, du bremst fünfzehn Meter zu
/// früh." Any language other than "de" is English.
pub fn tip_text(lang: &str, corner: &str, hints: &[Hint]) -> String {
    let clauses: Vec<String> = hints.iter().enumerate().map(|(i, h)| clause(lang, *h, i == 0)).collect();
    format!("{corner}: {}.", clauses.join(", "))
}

/// "Hatzenbach: besser als deine Bestrunde." / "Hatzenbach: better than your best lap."
pub fn praise_text(lang: &str, corner: &str) -> String {
    if lang == "de" { format!("{corner}: besser als deine Bestrunde.") } else { format!("{corner}: better than your best lap.") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineer::corners::{curvature, detect};
    use crate::engineer::testkit::{self as kit, Change, Piece, STEP};
    use crate::radio::frame::{Car, Own};

    const HZ: f64 = 20.0;
    /// The test track's corners in [`kit::arcs_on`]: the chicane's right and left, R 60, the hairpin R 40, R 150, R 200.
    const R60: usize = 2;
    const HAIRPIN: usize = 3;
    const R150: usize = 4;
    const R200: usize = 5;

    type Lap = (Vec<f64>, Vec<bool>);

    fn track() -> Vec<Piece> {
        kit::closed_track()
    }

    fn length() -> f64 {
        kit::length_of(&track())
    }

    fn arcs() -> Vec<(f64, f64, f64)> {
        kit::arcs_on(&track())
    }

    fn normal() -> Lap {
        kit::flying_lap(&track(), &[])
    }

    fn changed(changes: &[Change]) -> Lap {
        kit::flying_lap(&track(), changes)
    }

    /// Where the car starts braking for arc `c` on a lap (m from the lap's start).
    fn brake_start(lap: &Lap, c: usize) -> f64 {
        let mut i = (arcs()[c].0 / STEP).round() as usize;
        while i > 0 && !lap.1[i] {
            i -= 1;
        }
        while i > 0 && lap.1[i - 1] {
            i -= 1;
        }
        i as f64 * STEP
    }

    /// The hairpin braked 15 m earlier than on `normal` and taken 8 km/h slower.
    fn early_and_slow(c: usize) -> Change {
        Change { corner: c, slower_kmh: 8.0, brake_at: Some(brake_start(&normal(), c) - 15.0) }
    }

    /// The laps driven one after the other from the start line, sampled at 20 Hz from `phase` seconds on, each frame
    /// with the distance driven (m); the game counts a lap at each line.
    fn drive(laps: &[Lap], phase: f64) -> Vec<(f64, Frame)> {
        let track = track();
        let len = length();
        let (mut s, mut t, mut v, mut braking) = (vec![0.0], vec![0.0], vec![laps[0].0[0]], vec![laps[0].1[0]]);
        for (lap_v, lap_b) in laps {
            for i in 1..lap_v.len() {
                let dt = STEP / ((lap_v[i] + lap_v[i - 1]) / 2.0);
                s.push(s.last().unwrap() + STEP);
                t.push(t.last().unwrap() + dt);
                v.push(lap_v[i]);
                *braking.last_mut().unwrap() = lap_b[i - 1];
                braking.push(lap_b[i]);
            }
        }
        let mut out = Vec::new();
        let mut tau = phase;
        while tau < *t.last().unwrap() {
            let k = t.partition_point(|x| *x <= tau) - 1;
            let f = (tau - t[k]) / (t[k + 1] - t[k]);
            let at = s[k] + f * STEP;
            let [x, z] = kit::pose_on(&track, at);
            let throttle = if braking[k] { 0.0 } else if v[k + 1] > v[k] + 1e-9 { 1.0 } else { 0.3 };
            let own = Own {
                id: (0, 1),
                pos: [x as f32, 5.0, z as f32],
                contact: None,
                velocity: [0.0; 3],
                heading: 0.0,
                speed: (v[k] + f * (v[k + 1] - v[k])) as f32,
                yaw_rate: 0.0,
                throttle,
                brake: if braking[k] { 0.9 } else { 0.0 },
                in_pit_lane: false,
                in_pit_box: false,
                pit_exit: false,
                lap_count: (at / len + 1e-9).floor() as i32,
                lap_valid: true,
            };
            out.push((at, Frame { t: tau, live: true, own, others: Vec::new() }));
            tau += 1.0 / HZ;
        }
        out
    }

    /// The tips over the frames, with the distance driven when each was handed out.
    fn tips(live: &mut LiveTips, frames: &[(f64, Frame)]) -> Vec<(f64, Tip)> {
        frames.iter().filter_map(|(s, f)| live.step(f).map(|tip| (*s, tip))).collect()
    }

    fn texts(said: &[(f64, Tip)]) -> Vec<&str> {
        said.iter().map(|(_, t)| t.text.as_str()).collect()
    }

    /// A reference recorded from one lap of `lap`.
    fn reference_of(lap: &Lap) -> Reference {
        recorded(lap, 0.0).0
    }

    /// One lap of `lap` recorded at 20 Hz from `phase` seconds on, and where on the lap its first sample lies (m).
    fn recorded(lap: &Lap, phase: f64) -> (Reference, f64) {
        let frames = drive(std::slice::from_ref(lap), phase);
        let samples = frames.iter().map(|(_, f)| Sample { t: f.t, pos: f.own.pos, speed: f.own.speed, brake: f.own.brake, throttle: f.own.throttle }).collect();
        (Reference::from_samples(samples).unwrap(), frames[0].0)
    }

    #[test]
    fn the_test_track_closes_and_has_six_corners() {
        let len = length();
        let (start, end) = (kit::pose_on(&track(), 0.0), kit::pose_on(&track(), len - 1e-9));
        assert!((start[0] - end[0]).abs() < 1e-6 && (start[1] - end[1]).abs() < 1e-6, "{start:?} {end:?}");
        let r = reference_of(&normal());
        assert!(r.closed && (r.length() - len).abs() < 5.0, "{} against {len}", r.length());
        let found: Vec<(Direction, f64)> = r.map.corners.iter().map(|c| (c.direction, c.radius_m)).collect();
        assert_eq!(found.len(), 6, "{found:?}");
        // right then left in the chicane, then four lefts; radii within 10 %
        let expected = [(Direction::R, 100.0), (Direction::L, 100.0), (Direction::L, 60.0), (Direction::L, 40.0), (Direction::L, 150.0), (Direction::L, 200.0)];
        for ((d, r), (ed, er)) in found.iter().zip(expected) {
            assert!(*d == ed && (r - er).abs() <= 0.1 * er, "{found:?}");
        }
    }

    #[test]
    fn a_corner_braked_early_and_taken_slower_gets_one_tip_after_it() {
        let laps = [normal(), normal(), changed(&[early_and_slow(HAIRPIN)])];
        for (lang, phase) in [("de", 0.013), ("en", 0.037)] {
            let mut live = LiveTips::new(Config::default(), lang);
            let said = tips(&mut live, &drive(&laps, phase));
            assert_eq!(said.len(), 1, "{:?}", texts(&said));
            let (at, tip) = &said[0];
            // after the hairpin's arc, before the next corner's, in the third lap
            let (len, arcs) = (length(), arcs());
            assert!(*at > 2.0 * len + arcs[HAIRPIN].1 && *at < 2.0 * len + arcs[R150].0, "said at {at} m");
            assert_eq!((tip.call.key, tip.call.priority, tip.call.car), (KEY, Priority::Engineer, None));
            assert!((tip.call.expires - tip.call.at - 8.0).abs() < 1e-9);
            // the brake point to within the 20 Hz sampling, rounded to 5 m: 10, 15 or 20 m
            let ok = if lang == "de" {
                ["zehn", "fünfzehn", "zwanzig"].map(|m| format!("Kurve vier: acht Stundenkilometer weniger Kurventempo als in deiner Bestrunde, du bremst {m} Meter zu früh."))
            } else {
                ["ten", "fifteen", "twenty"].map(|m| format!("Turn four: eight kilometres an hour less corner speed than your best lap, you're braking {m} metres early."))
            };
            assert!(ok.contains(&tip.text), "{}", tip.text);
        }
    }

    #[test]
    fn measures_the_brake_point_and_corner_speed_to_the_stated_accuracy() {
        // the live measurement on the reference's line against the truth of the test car, for several sample phases
        let reference = reference_of(&normal());
        let truth = brake_start(&normal(), HAIRPIN);
        let v_brake = normal().0[(truth / STEP) as usize];
        let (mut worst, mut speed): (f64, f64) = (0.0, 0.0);
        for phase in [0.0, 0.011, 0.023, 0.037, 0.049] {
            let (own, start) = recorded(&changed(&[early_and_slow(HAIRPIN)]), phase);
            let (a, b) = (&own.corners[HAIRPIN].measure, &reference.corners[HAIRPIN].measure);
            // on the lap's distance: a lap recorded from a later sample starts that much after the line
            let d = a.point_m.unwrap() + start - b.point_m.unwrap();
            worst = worst.max((d + 15.0).abs());
            speed = speed.max((a.min_kmh - (b.min_kmh - 8.0)).abs());
            assert!((b.point_m.unwrap() - truth).abs() <= v_brake / HZ + 1.0, "{:?} against {truth}", b.point_m);
        }
        assert!(worst <= v_brake / HZ + 1.0 && speed < 0.5, "{worst} m at {v_brake} m/s, {speed} km/h");
        println!("15 m earlier, 8 km/h slower at {v_brake:.1} m/s: measured to {worst:.2} m and {speed:.2} km/h");
        // and live, through the projection: phases of the live lap against the reference's
        for phase in [0.0, 0.011, 0.023, 0.037, 0.049] {
            let mut live = LiveTips::new(Config { gap_s: 0.0, praise_gap_s: 0.0, ..Config::default() }, "en");
            live.set_reference(Some(reference.clone()));
            let said = tips(&mut live, &drive(&[changed(&[early_and_slow(HAIRPIN)])], phase));
            assert_eq!(said.len(), 1, "{:?}", texts(&said));
            assert!(said[0].1.text.contains("eight kilometres an hour"), "{}", said[0].1.text);
        }
    }

    #[test]
    fn a_lap_within_tolerance_says_nothing() {
        let small = changed(&[Change { corner: HAIRPIN, slower_kmh: 2.0, brake_at: Some(brake_start(&normal(), HAIRPIN) - 4.0) }, Change { corner: R150, slower_kmh: 3.0, brake_at: None }]);
        let mut live = LiveTips::new(Config::default(), "de");
        let said = tips(&mut live, &drive(&[normal(), normal(), normal(), small, normal()], 0.021));
        assert_eq!(texts(&said), Vec::<&str>::new());
        assert!(live.reference().is_some());
    }

    #[test]
    fn a_clearly_better_corner_is_praised_now_and_then_and_the_faster_lap_becomes_the_reference() {
        let slow = changed(&[early_and_slow(HAIRPIN), early_and_slow(R200)]);
        let mut live = LiveTips::new(Config::default(), "de");
        let frames = drive(&[normal(), slow.clone(), normal(), normal()], 0.017);
        let mut said = Vec::new();
        let mut new_references = Vec::new();
        for (s, f) in &frames {
            if let Some(tip) = live.step(f) {
                said.push((*s, tip));
            }
            if live.take_new_reference().is_some() {
                new_references.push(*s);
            }
        }
        // both corners are better than the slow reference, but praise comes at most every three minutes
        assert_eq!(texts(&said), ["Kurve vier: besser als deine Bestrunde."]);
        let len = length();
        assert!(said[0].0 > len * 2.0 && said[0].0 < len * 3.0);
        // the slow lap became the reference after the joined lap, the normal one after it; the next normal lap is
        // not faster
        assert_eq!(new_references.len(), 2, "{new_references:?}");
        assert!((new_references[0] - 2.0 * len).abs() < 10.0 && (new_references[1] - 3.0 * len).abs() < 10.0, "{new_references:?}");
        assert!(live.take_new_reference().is_none());
        let reference = live.reference().unwrap().time_s();
        // (a lap recorded live starts at its first sample after the line: up to 0.05 s later at 75 m/s)
        let (normal_s, slow_s) = (reference_of(&normal()).time_s(), reference_of(&slow).time_s());
        assert!((reference - normal_s).abs() <= 0.06 && slow_s - normal_s > 0.5, "{reference} against {normal_s} and {slow_s}");
    }

    #[test]
    fn two_bad_corners_without_a_straight_between_them_get_one_tip_after_the_second() {
        let point = brake_start(&normal(), 0);
        let chicane = changed(&[Change { corner: 0, slower_kmh: 8.0, brake_at: Some(point - 15.0) }, Change { corner: 1, slower_kmh: 10.0, brake_at: None }]);
        let mut live = LiveTips::new(Config::default(), "en");
        let said = tips(&mut live, &drive(&[normal(), normal(), chicane], 0.009));
        assert_eq!(said.len(), 1, "{:?}", texts(&said));
        let r = live.reference().unwrap();
        let (len, (second, next)) = (length(), (&r.map.corners[1], &r.map.corners[2]));
        assert!(r.corners[0].gap_s < 4.0 && r.corners[1].gap_s >= 4.0, "{} {}", r.corners[0].gap_s, r.corners[1].gap_s);
        assert!(said[0].0 >= 2.0 * len + second.end_m && said[0].0 < 2.0 * len + next.turn_in_m, "said at {}", said[0].0);
        assert!(said[0].1.text.starts_with("Turn one: ") || said[0].1.text.starts_with("Turn two: "), "{}", said[0].1.text);
    }

    #[test]
    fn one_tip_in_twenty_seconds_and_never_the_same_corner_twice_in_a_row() {
        let both = changed(&[early_and_slow(R60), early_and_slow(HAIRPIN)]);
        let again = changed(&[early_and_slow(R60)]);
        let hairpin = changed(&[early_and_slow(HAIRPIN)]);
        let mut live = LiveTips::new(Config::default(), "de");
        let said = tips(&mut live, &drive(&[normal(), normal(), both, again, hairpin], 0.031));
        let len = length();
        let laps: Vec<(i64, &str)> = said.iter().map(|(s, t)| ((s / len).floor() as i64, t.text.split(':').next().unwrap())).collect();
        // lap 3: R 60 m, the hairpin comes 16 s later; lap 4: R 60 m again is not said; lap 5: the hairpin
        assert_eq!(laps, [(2, "Kurve drei"), (4, "Kurve vier")], "{:?}", texts(&said));
    }

    /// The frames of a drive where the car leaves the pit lane on the start straight (in the pit lane until
    /// `exit` m) and the game counts each lap `split` m before the line, as on Touristenfahrten when `split` > 0.
    fn from_the_pits(laps: &[Lap], exit: f64, split: f64, entry: Option<f64>) -> Vec<(f64, Frame)> {
        let len = length();
        drive(laps, 0.027)
            .into_iter()
            .map(|(s, mut f)| {
                let lap_s = s.rem_euclid(len);
                f.own.lap_count = ((s + split) / len + 1e-9).floor() as i32;
                f.own.in_pit_lane = s < exit || entry.is_some_and(|e| lap_s >= len - e || lap_s < exit);
                (s, f)
            })
            .collect()
    }

    #[test]
    fn nothing_on_a_circuit_s_out_lap_or_in_the_pit_lane() {
        let bad = changed(&[early_and_slow(HAIRPIN)]);
        let mut live = LiveTips::new(Config::default(), "de");
        live.set_reference(Some(reference_of(&normal())));
        // out of the pits on the start straight: the out lap says nothing, the flying lap after it does
        let said = tips(&mut live, &from_the_pits(&[bad.clone(), bad.clone()], 300.0, 0.0, None));
        let len = length();
        assert_eq!(said.len(), 1, "{:?}", texts(&said));
        assert!(said[0].0 > len && said[0].1.text.starts_with("Kurve vier: "));
        // through the pit lane in the middle of a lap
        let mut live = LiveTips::new(Config::default(), "de");
        live.set_reference(Some(reference_of(&normal())));
        let (a, b) = (arcs()[HAIRPIN].0 - 300.0, arcs()[HAIRPIN].1);
        let frames: Vec<(f64, Frame)> = drive(std::slice::from_ref(&bad), 0.0)
            .into_iter()
            .map(|(s, mut f)| {
                f.own.in_pit_lane = s > a && s < b;
                (s, f)
            })
            .collect();
        assert_eq!(tips(&mut live, &frames).len(), 0);
    }

    #[test]
    fn on_touristenfahrten_the_lap_from_the_pit_exit_is_the_lap() {
        // the parking lot around the line, the counter at the end split 200 m before it: an open reference line
        let bad = changed(&[early_and_slow(HAIRPIN)]);
        let laps = [normal(), bad.clone(), bad];
        let frames = from_the_pits(&laps, 250.0, 200.0, Some(150.0));
        let mut live = LiveTips::new(Config { laps_start_at_pit_exit: true, ..Config::for_layout("Nurburgring", "Touristenfahrten") }, "en");
        let said = tips(&mut live, &frames);
        let r = live.reference().unwrap();
        assert!(!r.closed && (r.length() - (length() - 450.0)).abs() < 10.0, "{}", r.length());
        // the first loop is the reference; the second says the hairpin; the third is the same corner again
        assert_eq!(texts(&said).len(), 1, "{:?}", texts(&said));
        assert!(said[0].1.text.starts_with("Turn four: "), "{}", said[0].1.text);
        // the same drive taken as a circuit: every lap from the pit exit is an out lap
        let mut circuit = LiveTips::new(Config::default(), "en");
        assert_eq!(tips(&mut circuit, &frames).len(), 0);
        assert!(circuit.reference().is_none());
        assert!(Config::for_layout("Nurburgring", "Touristenfahrten").laps_start_at_pit_exit && !Config::for_layout("Nurburgring", "GP").laps_start_at_pit_exit);
    }

    #[test]
    fn nothing_in_traffic_or_where_the_lap_became_invalid() {
        let bad = changed(&[early_and_slow(HAIRPIN)]);
        let (a, b) = (arcs()[HAIRPIN].0 - 100.0, arcs()[HAIRPIN].1);
        for case in ["traffic", "invalid", "none"] {
            let mut live = LiveTips::new(Config::default(), "de");
            live.set_reference(Some(reference_of(&normal())));
            let frames: Vec<(f64, Frame)> = drive(std::slice::from_ref(&bad), 0.0)
                .into_iter()
                .map(|(s, mut f)| {
                    if s > a && s < b {
                        match case {
                            "traffic" => f.others.push(Car { id: (0, 2), pos: [f.own.pos[0] + 12.0, f.own.pos[1], f.own.pos[2]] }),
                            "invalid" => f.own.lap_valid = false,
                            _ => {}
                        }
                    }
                    (s, f)
                })
                .collect();
            assert_eq!(tips(&mut live, &frames).len(), usize::from(case == "none"), "{case}");
        }
    }

    #[test]
    fn robust_without_a_reference_after_a_jump_the_wrong_way_and_on_another_track() {
        let bad = changed(&[early_and_slow(HAIRPIN)]);
        // no reference: the first (joined) lap gives none
        let mut live = LiveTips::new(Config::default(), "de");
        assert_eq!(tips(&mut live, &drive(std::slice::from_ref(&bad), 0.0)).len(), 0);
        assert!(live.reference().is_none());
        // back to the pits in the middle of the hairpin, standing there for half a minute
        let mut live = LiveTips::new(Config::default(), "de");
        live.set_reference(Some(reference_of(&normal())));
        let mid = (arcs()[HAIRPIN].0 + arcs()[HAIRPIN].1) / 2.0;
        let mut frames: Vec<(f64, Frame)> = drive(std::slice::from_ref(&bad), 0.0).into_iter().take_while(|(s, _)| *s < mid).collect();
        let (s, last) = frames.last().unwrap().clone();
        for k in 1..600 {
            let mut f = last.clone();
            f.t += k as f64 / HZ;
            (f.own.pos, f.own.speed, f.own.in_pit_box) = ([-50.0, 5.0, 30.0], 0.0, true);
            frames.push((s, f));
        }
        assert_eq!(tips(&mut live, &frames).len(), 0);
        // the same jump without the pit flags
        let mut live = LiveTips::new(Config::default(), "de");
        live.set_reference(Some(reference_of(&normal())));
        let jumped: Vec<(f64, Frame)> = frames.into_iter().map(|(s, mut f)| {
            f.own.in_pit_box = false;
            (s, f)
        }).collect();
        assert_eq!(tips(&mut live, &jumped).len(), 0);
        // the wrong way round: the frames of a lap backwards
        let mut live = LiveTips::new(Config::default(), "de");
        live.set_reference(Some(reference_of(&normal())));
        let forward = drive(&[bad.clone(), bad.clone()], 0.0);
        let end = forward.last().unwrap().1.t;
        let backward: Vec<(f64, Frame)> = forward.iter().rev().map(|(s, f)| {
            let mut f = f.clone();
            (f.t, f.own.lap_count) = (end - f.t, 0);
            (*s, f)
        }).collect();
        assert_eq!(tips(&mut live, &backward).len(), 0);
        // another track: the same drive 400 m away
        let mut live = LiveTips::new(Config::default(), "de");
        live.set_reference(Some(reference_of(&normal())));
        let elsewhere: Vec<(f64, Frame)> = drive(&[bad.clone(), bad.clone()], 0.0).into_iter().map(|(s, mut f)| {
            f.own.pos[0] += 400.0;
            (s, f)
        }).collect();
        assert_eq!(tips(&mut live, &elsewhere).len(), 0);
        // the reference taken away in the middle of a lap, and frames outside live driving
        let mut live = LiveTips::new(Config::default(), "de");
        live.set_reference(Some(reference_of(&normal())));
        for (k, (_, f)) in drive(std::slice::from_ref(&bad), 0.0).iter().enumerate() {
            if k == 900 {
                live.set_reference(None);
            }
            let mut f = f.clone();
            f.live = k % 7 != 0;
            assert!(live.step(&f).is_none());
        }
    }

    #[test]
    fn the_reference_is_kept_per_car_preset_track_and_layout() {
        let dir = std::env::temp_dir().join(format!("dd-live-tips-{}", std::process::id()));
        assert!(load_reference(&dir, "dd_x", Some("stage1"), "Spa", "GP").is_none());
        let reference = reference_of(&normal());
        save_reference(&dir, "dd_x", Some("stage1"), "Spa", "GP", &reference).unwrap();
        assert!(dir.join("live").join(format!("{}.json", store::refs_key("dd_x", Some("stage1"), "Spa", "GP"))).is_file());
        let back = load_reference(&dir, "dd_x", Some("stage1"), "Spa", "GP").unwrap();
        assert_eq!(back.map, reference.map);
        assert!((back.time_s() - reference.time_s()).abs() < 0.001);
        assert!(load_reference(&dir, "dd_x", Some("stage2"), "Spa", "GP").is_none());
        // a file of another version is no reference
        let path = reference_path(&dir, "dd_x", Some("stage1"), "Spa", "GP");
        fs::write(&path, br#"{"version":2,"samples":[]}"#).unwrap();
        assert!(load_reference(&dir, "dd_x", Some("stage1"), "Spa", "GP").is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The lap as the engineer sees it in a MoTeC file: speed and ROTY every metre (exact here), and its corner map
    /// with the lap starting `shift` m later.
    fn engineer_map(lap: &Lap, shift: f64) -> (Map, Grid) {
        let per_metre = (1.0 / STEP) as usize;
        let n = (lap.0.len() - 1) / per_metre;
        let at = |i: usize| (i * per_metre + (shift / STEP) as usize) % (lap.0.len() - 1);
        let kappa = |s: f64| arcs().iter().find(|(a, b, _)| s >= *a && s < *b).map_or(0.0, |x| x.2);
        let speed: Vec<f64> = (0..=n).map(|i| lap.0[at(i)]).collect();
        let yaw: Vec<f64> = (0..=n).map(|i| lap.0[at(i)] * kappa(at(i) as f64 * STEP)).collect();
        let map = Map::from_lap(&detect(&curvature(&speed, &yaw)), &speed, 1);
        let mut t = vec![0.0; n + 1];
        for i in 1..=n {
            t[i] = t[i - 1] + 2.0 / (speed[i] + speed[i - 1]);
        }
        let brake: Vec<f64> = (0..=n).map(|i| if lap.1[at(i)] { 90.0 } else { 0.0 }).collect();
        let g = Grid { t, throttle: vec![0.0; n + 1], brake, steer: vec![0.0; n + 1], gear: vec![0.0; n + 1], g_lat: vec![0.0; n + 1], yaw, slip: std::array::from_fn(|_| vec![0.0; n + 1]), speed };
        (map, g)
    }

    #[test]
    fn finds_the_corners_and_brake_points_the_engineer_finds_on_the_same_lap() {
        let (map, g) = engineer_map(&normal(), 0.0);
        let live = reference_of(&normal());
        assert_eq!(map.corners.len(), live.map.corners.len());
        let whole = Fit { scale: 1.0, offset: 0.0, matched: 0 };
        let (mut turns, mut points): (f64, f64) = (0.0, 0.0);
        for (c, (e, l)) in map.corners.iter().zip(&live.map.corners).enumerate() {
            assert_eq!(e.direction, l.direction);
            turns = turns.max((e.turn_in_m - l.turn_in_m).abs()).max((e.turn_out_m - l.turn_out_m).abs());
            // the engineer's brake point over the engineer's segment
            let engineer = measure::corner(&g, &map, &whole, c).point_m;
            if let (Some(a), Some(b)) = (engineer, live.corners[c].measure.point_m) {
                let v = g.speed[a as usize];
                assert!((a - b).abs() <= v / HZ + 1.0, "corner {}: {a} against {b} at {v} m/s", c + 1);
                points = points.max((a - b).abs());
            }
        }
        // from 20 Hz positions against ROTY every metre: the turn-ins and -outs within 3 m
        assert!(turns <= 3.0, "{turns} m");
        println!("live against the engineer on the same lap: turn-in/out within {turns:.2} m, brake points within {points:.2} m");
    }

    #[test]
    fn names_the_corners_as_the_engineer_s_page_does() {
        // the engineer's map of a Touristenfahrten-like lap that starts 120 m later, the platform's names by its numbers
        let (map, _) = engineer_map(&normal(), 120.0);
        let names = BTreeMap::from([(4, "Hatzenbach 2".to_string()), (3, "T13".to_string())]);
        let laps = [normal(), normal(), changed(&[early_and_slow(HAIRPIN), early_and_slow(R60)])];
        let mut live = LiveTips::new(Config::default(), "de");
        live.set_names(Some(map.clone()), names.clone());
        let said = tips(&mut live, &drive(&laps, 0.019));
        // R 60 m is the engineer's corner 3, named only by its section's number: "Kurve drei"; the hairpin (16 s
        // later) is not said
        assert_eq!(said.len(), 1, "{:?}", texts(&said));
        assert!(said[0].1.text.starts_with("Kurve drei: "), "{}", said[0].1.text);
        let labels: Vec<String> = (0..6).map(|c| live.spoken(c)).collect();
        assert_eq!(labels, ["Kurve eins", "Kurve zwei", "Kurve drei", "Hatzenbach zwei", "Kurve fünf", "Kurve sechs"]);
        // a world turned the other way round still meets the map
        let mirrored: Vec<Detected> = live.reference().unwrap().detected(true);
        assert!(mirrored.iter().zip(&live.reference().unwrap().map.corners).all(|(m, c)| m.direction != c.direction));
        let mut flipped = map.clone();
        for c in &mut flipped.corners {
            c.direction = if c.direction == Direction::L { Direction::R } else { Direction::L };
        }
        live.set_names(Some(flipped), names);
        assert_eq!(live.spoken(HAIRPIN), "Hatzenbach zwei");
        // a map of another track: the numbers on the reference
        let mut other = map;
        other.corners.retain(|c| c.n <= 2);
        live.set_names(Some(other), BTreeMap::new());
        assert_eq!(live.spoken(HAIRPIN), "Kurve vier");
    }

    #[test]
    fn the_sentences_spell_their_numbers() {
        use Hint::*;
        let cases: [(&[Hint], &str, &str); 5] = [
            (&[Slower(6), BrakeEarlier(10)], "Hatzenbach: sechs Stundenkilometer weniger Kurventempo als in deiner Bestrunde, du bremst zehn Meter zu früh.", "Hatzenbach: six kilometres an hour less corner speed than your best lap, you're braking ten metres early."),
            (&[BrakeLater(15)], "Hatzenbach: du bremst fünfzehn Meter später als in deiner Bestrunde.", "Hatzenbach: you're braking fifteen metres later than your best lap."),
            (&[BrakeEarlier(25), ThrottleLater(20)], "Hatzenbach: du bremst fünfundzwanzig Meter früher als in deiner Bestrunde, du gehst zwanzig Meter zu spät aufs Gas.", "Hatzenbach: you're braking twenty-five metres earlier than your best lap, you're twenty metres late on the throttle."),
            (&[ThrottleLater(10)], "Hatzenbach: du gehst zehn Meter später aufs Gas als in deiner Bestrunde.", "Hatzenbach: you're on the throttle ten metres later than your best lap."),
            (&[Slower(21), BrakeLater(30)], "Hatzenbach: einundzwanzig Stundenkilometer weniger Kurventempo als in deiner Bestrunde, du bremst dreißig Meter zu spät.", "Hatzenbach: twenty-one kilometres an hour less corner speed than your best lap, you're braking thirty metres late."),
        ];
        for (hints, de, en) in cases {
            assert_eq!(tip_text("de", "Hatzenbach", hints), de);
            assert_eq!(tip_text("en", "Hatzenbach", hints), en);
        }
        assert_eq!(praise_text("de", "Kurve zwölf"), "Kurve zwölf: besser als deine Bestrunde.");
        assert_eq!(praise_text("en", "Turn twelve"), "Turn twelve: better than your best lap.");
        assert_eq!(spoken_label("de", &Label::Number(12)), "Kurve zwölf");
        assert_eq!(spoken_label("en", &Label::Number(12)), "Turn twelve");
        assert_eq!(spoken_label("de", &Label::Name("Hatzenbach 12".into())), "Hatzenbach zwölf");
        assert_eq!(spoken_label("en", &Label::Name("Sabine-Schmitz-Kurve".into())), "Sabine-Schmitz-Kurve");
        assert!(unnamed("T13") && unnamed(" ") && !unnamed("Tiergarten") && !unnamed("T"));
        // every clause in both languages, every number a tip can say
        for lang in ["de", "en"] {
            for n in [4, 5, 10, 99, 120] {
                for (a, b) in [(Slower(n), BrakeEarlier(n)), (BrakeLater(n), ThrottleLater(n)), (ThrottleLater(n), Slower(n))] {
                    let text = tip_text(lang, &spoken_label(lang, &Label::Number(n as u32)), &[a, b]);
                    assert!(!text.chars().any(|c| c.is_ascii_digit()) && text.ends_with('.'), "{text}");
                }
            }
        }
    }
}
