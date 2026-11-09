//! The spotter: from the own car's pose and the other cars' positions (all EVO publishes about them) to the calls a
//! spotter makes, "car left", "three wide", "clear", "closing fast" ... The design is research
//! 2026-11-09-boxenfunk/A-evo-spotter-data.md §2.2 and §3; every threshold is a [`Config`] field with the research's
//! value as its default, to be tuned on recordings.
//!
//! Geometry runs in the x/z plane with Y up (the live map draws the track that way). Other cars are tracked by their
//! id, never by their slot, and their velocities come from their positions over a sliding window: the pages carry
//! nothing else about them. The own car's velocity for the relative speeds comes from its positions on the same page
//! the same way, so a late graphics page shifts both alike and the difference stays right.
//!
//! `car_coordinates` mark the same point on every car (where on the car is not known yet), so the offset between two
//! cars of one size needs no shift; the box assumes the other car has the own car's size (a single-make server most of
//! the time).

use std::collections::{HashMap, HashSet, VecDeque};

use super::frame::{CarId, Frame};

/// Where the own car's forward and right directions come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoseSource {
    /// The wheels' contact points: forward = mid(FL, FR) - mid(RL, RR), right = FR - FL. Same world frame as the
    /// positions, so no convention is assumed.
    Contact,
    /// The direction of the world velocity, above [`Config::pose_min_speed_ms`]
    Velocity,
    /// The direction the own car moved over [`Config::pose_delta_window_s`]
    PositionDelta,
}

/// The research's order. The physics page's `heading` stays out until a recording shows its zero and sign.
pub const POSE_SOURCES: &[PoseSource] = &[PoseSource::Contact, PoseSource::Velocity, PoseSource::PositionDelta];

/// The spotter's thresholds (research §2.2, §3), metres, seconds and m/s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// The pose sources, the first sane one wins
    pub pose_sources: &'static [PoseSource],
    /// Which way "right" lies of a forward direction (x, z) when it comes from the velocity or the position delta
    /// (contact points give it directly): true = (-z, x), right = forward × up in a right-handed world with Y up;
    /// false = (z, -x), a left-handed world. An assumption until a recording compares it with the contact points (the
    /// synthetic selftest recording's contact points agree with true).
    pub right_handed: bool,
    /// Contact points are sane with a wheelbase and a track width in these ranges
    pub wheelbase_m: (f32, f32),
    pub track_width_m: (f32, f32),
    /// The velocity and the position delta give a direction only above this speed
    pub pose_min_speed_ms: f32,
    pub pose_delta_window_s: f64,
    /// The cars' size (the own car's; the others are assumed alike)
    pub car_length_m: f32,
    pub car_width_m: f32,
    /// A new overlap: |long| below one length ahead, one length plus this behind
    pub behind_extra_m: f32,
    /// An overlap ends at |long| of one length plus this, held for the clear delay
    pub clear_extra_m: f32,
    /// Cars further to the side are not alongside
    pub lateral_max_m: f32,
    /// Closer to the side than this share of the width (contact, or the same lane) a car still counts but keeps the
    /// side it had
    pub inner_lateral_share: f32,
    /// Cars higher or lower than this (bridges, pit buildings, start areas on two levels) are ignored
    pub height_max_m: f32,
    pub overlap_delay_s: f64,
    pub clear_delay_s: f64,
    /// A side announced clear is not announced again before this
    pub bounce_s: f64,
    pub still_there_every_s: f64,
    pub still_there_max: u32,
    /// The spotter is silent at or below this own speed
    pub min_speed_ms: f32,
    /// A car closing faster than this does not become an overlap ("closing fast" warns of it instead)
    pub max_relative_ms: f32,
    /// Cars slower than this for longer than `parked_after_s` are parked and never alongside
    pub parked_speed_ms: f32,
    pub parked_after_s: f64,
    /// A jump of more than max(this, `teleport_speed_ms` × dt) is a teleport
    pub teleport_min_m: f32,
    /// 1.5 × 90 m/s
    pub teleport_speed_ms: f32,
    /// Silence after the own car teleported (return to pits, a session restart)
    pub mute_after_teleport_s: f64,
    /// Velocities from positions over at least this window ...
    pub velocity_window_s: f64,
    /// ... with this many samples averaged at each end
    pub velocity_samples: usize,
    /// "Closing fast": a car behind, within this time gap, closing faster than this, reaching the own car within
    /// this time; once per car per `closing_every_s`
    pub closing_gap_s: f32,
    pub closing_min_ms: f32,
    pub closing_reach_s: f32,
    pub closing_every_s: f64,
    /// ... and not while the own car brakes harder than this (pedal 0 to 1): a car that brakes later behind it
    /// closes fast but stays behind. Such a car uses up its call all the same, else it would come at the brake's
    /// release.
    pub closing_max_brake: f32,
    /// "Close behind": within this time gap for `close_hold_s`; once per car per `close_every_s`, again after
    /// `close_repeat_s` while the pressure holds
    pub close_gap_s: f32,
    pub close_hold_s: f64,
    pub close_every_s: f64,
    pub close_repeat_s: f64,
    /// "Close behind" and "slow car ahead" look this far to the side (in line)
    pub in_line_lateral_m: f32,
    /// Slipstream both ways: time gap in this range, this close to the line, above this own speed, on a straight
    /// (|yaw rate| below `straight_yaw_rate`), held `slip_hold_s`; once per car per straight
    pub slip_gap_s: (f32, f32),
    pub slip_lateral_m: f32,
    pub slip_min_speed_ms: f32,
    pub slip_hold_s: f64,
    pub straight_yaw_rate: f32,
    /// A straight ends once |yaw rate| stayed at or above `straight_yaw_rate` this long (a corner)
    pub corner_min_s: f64,
    /// "Slow car ahead": within this distance ahead in the own frame (no reference path yet: reliable on straights
    /// only), slower than this share of the own speed or than `slow_speed_ms`, and at least `slow_closing_ms` slower
    /// than the own car; once per car per `slow_every_s`
    pub slow_range_m: f32,
    pub slow_share: f32,
    pub slow_speed_ms: f32,
    pub slow_closing_ms: f32,
    pub slow_every_s: f64,
    /// At the pit exit a car outside the pit mask, moving the own car's way, faster than the own car, that closes on
    /// it from at most this far and reaches it within this time, is traffic; once an exit. Distances in the x/z
    /// plane: no pose needed, and the track may lie on another level there (8 m below the exit road at Brands Hatch).
    /// A car driving the other way (on the straight beside the exit road at Brands Hatch) or standing is no traffic.
    pub pit_exit_range_m: f32,
    pub pit_exit_reach_s: f32,
    /// The exit is clear, said once at its first sample, when no car moving the own car's way (faster or not) is
    /// nearer than `pit_exit_near_m` or closes on the own car from at most `pit_exit_clear_range_m` within
    /// `pit_exit_clear_reach_s`: the warning's 4 s plus the 4.3 s the own car was at the exit at Brands Hatch, so a
    /// car that would come into the warning's window while the own car is still at the exit holds the clear back. A
    /// car that comes later is still warned of.
    pub pit_exit_near_m: f32,
    pub pit_exit_clear_range_m: f32,
    pub pit_exit_clear_reach_s: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pose_sources: POSE_SOURCES,
            right_handed: true,
            wheelbase_m: (1.5, 4.0),
            track_width_m: (1.0, 2.5),
            pose_min_speed_ms: 5.0,
            pose_delta_window_s: 0.1,
            car_length_m: 4.7,
            car_width_m: 2.0,
            behind_extra_m: 0.4,
            clear_extra_m: 0.5,
            lateral_max_m: 6.0,
            inner_lateral_share: 0.8,
            height_max_m: 3.0,
            overlap_delay_s: 0.05,
            clear_delay_s: 0.15,
            bounce_s: 1.5,
            still_there_every_s: 3.0,
            still_there_max: 2,
            min_speed_ms: 10.0,
            max_relative_ms: 12.0,
            parked_speed_ms: 3.0,
            parked_after_s: 2.0,
            teleport_min_m: 10.0,
            teleport_speed_ms: 1.5 * 90.0,
            mute_after_teleport_s: 2.0,
            velocity_window_s: 0.2,
            velocity_samples: 3,
            closing_gap_s: 2.5,
            closing_min_ms: 5.0,
            closing_reach_s: 3.0,
            closing_every_s: 30.0,
            closing_max_brake: 0.1,
            close_gap_s: 0.6,
            close_hold_s: 3.0,
            close_every_s: 60.0,
            close_repeat_s: 30.0,
            in_line_lateral_m: 4.0,
            slip_gap_s: (0.2, 0.6),
            slip_lateral_m: 1.2,
            slip_min_speed_ms: 150.0 / 3.6,
            slip_hold_s: 1.5,
            straight_yaw_rate: 0.05,
            corner_min_s: 1.0,
            slow_range_m: 150.0,
            slow_share: 0.4,
            slow_speed_ms: 15.0,
            slow_closing_ms: 5.0,
            slow_every_s: 20.0,
            pit_exit_range_m: 200.0,
            pit_exit_reach_s: 4.0,
            pit_exit_near_m: 50.0,
            pit_exit_clear_range_m: 400.0,
            pit_exit_clear_reach_s: 8.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// What the spotter calls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    CarLeft { car: CarId },
    CarRight { car: CarId },
    /// Cars on both sides: "in the middle"
    ThreeWide,
    /// Two cars beside each other on the right: the driver is the left one of three
    ThreeWideOnLeft,
    /// Two cars beside each other on the left: the driver is the right one of three
    ThreeWideOnRight,
    StillThere { side: Side },
    ClearLeft,
    ClearRight,
    ClearAll,
    ClosingFast { car: CarId, gap_s: f32 },
    CloseBehind { car: CarId, gap_s: f32 },
    /// A car sits in the driver's slipstream
    SlipstreamBehind { car: CarId },
    /// The driver sits in another car's slipstream
    InTow { car: CarId },
    SlowCarAhead { car: CarId },
    /// At the pit exit: a faster car closing from behind on the track
    PitExitTraffic { car: CarId },
    /// At the pit exit: no car comes
    PitExitClear,
}

/// Where the own car points: unit vectors in the x/z plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub forward: [f32; 2],
    pub right: [f32; 2],
    pub source: PoseSource,
}

/// Edge of a pit mask cell. Part of a stored mask's meaning, so not a setting.
pub const PIT_CELL_M: f32 = 4.0;

/// The ground the own car covered in the pit lane, in cells of [`PIT_CELL_M`]: cars inside are ignored. Learned while
/// driving; the shell can keep it per track and layout as the plain list of [`PitMask::cells`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PitMask {
    cells: HashSet<(i32, i32)>,
}

impl PitMask {
    pub fn from_cells(cells: &[[i32; 2]]) -> Self {
        Self { cells: cells.iter().map(|c| (c[0], c[1])).collect() }
    }

    /// The cells as (x, z) indices (cell = floor(coordinate / 4 m)), sorted.
    pub fn cells(&self) -> Vec<[i32; 2]> {
        let mut cells: Vec<[i32; 2]> = self.cells.iter().map(|c| [c.0, c.1]).collect();
        cells.sort_unstable();
        cells
    }

    fn cell(pos: [f32; 3]) -> (i32, i32) {
        ((pos[0] / PIT_CELL_M).floor() as i32, (pos[2] / PIT_CELL_M).floor() as i32)
    }

    fn add(&mut self, pos: [f32; 3]) {
        self.cells.insert(Self::cell(pos));
    }

    pub fn contains(&self, pos: [f32; 3]) -> bool {
        self.cells.contains(&Self::cell(pos))
    }
}

/// A frame further apart than this from the one before starts afresh: the windows would span a pause.
const MAX_GAP_S: f64 = 1.0;
/// How far back a car's positions are kept
const KEEP_S: f64 = 1.0;
/// Slack for times that are sums of float steps
const EPS: f64 = 1e-6;

fn held(since: f64, now: f64, wait: f64) -> bool {
    now - since >= wait - EPS
}

fn due(last: Option<f64>, now: f64, every: f64) -> bool {
    last.is_none_or(|last| held(last, now, every))
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn unit(v: [f32; 2]) -> Option<[f32; 2]> {
    let len = v[0].hypot(v[1]);
    (len > 1e-3).then(|| [v[0] / len, v[1] / len])
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    t: f64,
    pos: [f32; 3],
}

/// A car's recent positions.
#[derive(Debug, Default)]
struct History(VecDeque<Sample>);

impl History {
    fn push(&mut self, t: f64, pos: [f32; 3]) {
        self.0.push_back(Sample { t, pos });
        while self.0.front().is_some_and(|s| s.t < t - KEEP_S) {
            self.0.pop_front();
        }
    }

    fn last(&self) -> Option<Sample> {
        self.0.back().copied()
    }

    /// x/z velocity: the mean of the last `k` positions against the mean of the `k` ending `window` earlier.
    fn velocity(&self, window: f64, k: usize) -> Option<[f32; 2]> {
        let h = &self.0;
        let n = h.len();
        let now = h.back()?.t;
        let end = h.iter().rposition(|s| s.t <= now - window + EPS)?;
        if k == 0 || n < k || end + 1 < k {
            return None;
        }
        let mean = |from: usize| {
            let (mut t, mut x, mut z) = (0.0, 0.0, 0.0);
            for s in h.range(from..from + k) {
                (t, x, z) = (t + s.t, x + s.pos[0], z + s.pos[2]);
            }
            (t / k as f64, x / k as f32, z / k as f32)
        };
        let ((t1, x1, z1), (t0, x0, z0)) = (mean(n - k), mean(end + 1 - k));
        let dt = (t1 - t0) as f32;
        (dt > 0.0).then(|| [(x1 - x0) / dt, (z1 - z0) / dt])
    }

    /// The x/z displacement since the newest sample at least `window` old, and the time it took.
    fn displacement(&self, window: f64) -> Option<([f32; 2], f32)> {
        let last = self.last()?;
        let old = self.0.iter().rev().find(|s| s.t <= last.t - window + EPS)?;
        Some(([last.pos[0] - old.pos[0], last.pos[2] - old.pos[2]], (last.t - old.t) as f32))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Beside {
    No,
    Entering { since: f64 },
    Yes { side: Side },
    Leaving { side: Side, since: f64 },
}

#[derive(Debug)]
struct Track {
    history: History,
    slow_since: Option<f64>,
    beside: Beside,
    /// Own-frame offset at the last frame: ahead, right
    long: f32,
    lat: f32,
}

impl Track {
    fn new() -> Self {
        Self { history: History::default(), slow_since: None, beside: Beside::No, long: f32::NAN, lat: f32::NAN }
    }
}

/// The rate limits and hold timers of one car's calls; they outlive the car's track, which a teleport resets.
#[derive(Debug, Default)]
struct Limits {
    closing_fast: Option<f64>,
    close_since: Option<f64>,
    close_last: Option<f64>,
    slip_since: Option<f64>,
    slip_straight: Option<u32>,
    tow_since: Option<f64>,
    tow_straight: Option<u32>,
    slow_ahead: Option<f64>,
}

impl Limits {
    fn hold_off(&mut self) {
        (self.close_since, self.slip_since, self.tow_since) = (None, None, None);
    }
}

fn index(side: Side) -> usize {
    match side {
        Side::Left => 0,
        Side::Right => 1,
    }
}

/// The spotter, stepped with every frame the shell reads (20 a second).
#[derive(Debug)]
pub struct Spotter {
    config: Config,
    mask: PitMask,
    last_t: Option<f64>,
    mute_until: f64,
    own: History,
    pose: Option<Pose>,
    tracks: HashMap<CarId, Track>,
    limits: HashMap<CarId, Limits>,
    /// Cars per side (0, 1 or 2) as last announced: left, right
    said: [u8; 2],
    cleared_at: [f64; 2],
    /// Per side: the time of the last call about it, and how many "still there" followed
    still: [(f64, u32); 2],
    /// Counts the straights: the index moves on with each corner
    straight: u32,
    turning_since: Option<f64>,
    corner_counted: bool,
    /// The traffic at this pit exit was called
    pit_exit_said: bool,
    /// This pit exit was judged clear or not
    pit_exit_judged: bool,
}

impl Spotter {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            mask: PitMask::default(),
            last_t: None,
            mute_until: f64::NEG_INFINITY,
            own: History::default(),
            pose: None,
            tracks: HashMap::new(),
            limits: HashMap::new(),
            said: [0; 2],
            cleared_at: [f64::NEG_INFINITY; 2],
            still: [(f64::NEG_INFINITY, 0); 2],
            straight: 0,
            turning_since: None,
            corner_counted: false,
            pit_exit_said: false,
            pit_exit_judged: false,
        }
    }

    /// Starts afresh; only the learned pit mask stays.
    fn reset(&mut self) {
        let mask = std::mem::take(&mut self.mask);
        *self = Self { mask, ..Self::new(self.config) };
    }

    /// Forgets the overlaps without calling "clear": the spotter is silent (pit lane, slow, muted).
    fn silence(&mut self) {
        for track in self.tracks.values_mut() {
            track.beside = Beside::No;
        }
        for limits in self.limits.values_mut() {
            limits.hold_off();
        }
        self.said = [0; 2];
        self.cleared_at = [f64::NEG_INFINITY; 2];
        self.still = [(f64::NEG_INFINITY, 0); 2];
    }

    /// Whether a car is alongside as last announced: the queue holds back what is not urgent.
    pub fn overlap(&self) -> bool {
        self.said != [0, 0]
    }

    /// The own pose of the last frame, and which source gave it.
    pub fn pose(&self) -> Option<Pose> {
        self.pose
    }

    /// Where a car was at the last frame in the own car's frame: metres ahead (behind < 0) and to the right (left < 0).
    pub fn offset(&self, car: CarId) -> Option<(f32, f32)> {
        self.tracks.get(&car).filter(|t| t.long.is_finite()).map(|t| (t.long, t.lat))
    }

    pub fn pit_mask(&self) -> &PitMask {
        &self.mask
    }

    pub fn set_pit_mask(&mut self, mask: PitMask) {
        self.mask = mask;
    }

    fn jumped(&self, last: Sample, t: f64, pos: [f32; 3]) -> bool {
        let d = ((pos[0] - last.pos[0]).powi(2) + (pos[1] - last.pos[1]).powi(2) + (pos[2] - last.pos[2]).powi(2)).sqrt();
        d > self.config.teleport_min_m.max(self.config.teleport_speed_ms * (t - last.t) as f32)
    }

    fn velocity(&self, history: &History) -> Option<[f32; 2]> {
        history.velocity(self.config.velocity_window_s, self.config.velocity_samples)
    }

    fn right_of(&self, forward: [f32; 2]) -> [f32; 2] {
        if self.config.right_handed {
            [-forward[1], forward[0]]
        } else {
            [forward[1], -forward[0]]
        }
    }

    fn find_pose(&self, frame: &Frame) -> Option<Pose> {
        let c = &self.config;
        let from_forward = |v: [f32; 2], source| unit(v).map(|forward| Pose { forward, right: self.right_of(forward), source });
        c.pose_sources.iter().find_map(|source| match source {
            PoseSource::Contact => {
                let [fl, fr, rl, rr] = frame.own.contact?;
                let forward: [f32; 3] = std::array::from_fn(|i| (fl[i] + fr[i] - rl[i] - rr[i]) / 2.0);
                let right: [f32; 3] = std::array::from_fn(|i| fr[i] - fl[i]);
                let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                let (wheelbase, track) = (len(forward), len(right));
                if !(c.wheelbase_m.0..=c.wheelbase_m.1).contains(&wheelbase) || !(c.track_width_m.0..=c.track_width_m.1).contains(&track) {
                    return None;
                }
                Some(Pose { forward: unit([forward[0], forward[2]])?, right: unit([right[0], right[2]])?, source: PoseSource::Contact })
            }
            PoseSource::Velocity => {
                let v = [frame.own.velocity[0], frame.own.velocity[2]];
                (v[0].hypot(v[1]) > c.pose_min_speed_ms).then(|| from_forward(v, PoseSource::Velocity)).flatten()
            }
            PoseSource::PositionDelta => {
                let (d, dt) = self.own.displacement(c.pose_delta_window_s)?;
                (dt > 0.0 && d[0].hypot(d[1]) / dt > c.pose_min_speed_ms).then(|| from_forward(d, PoseSource::PositionDelta)).flatten()
            }
        })
    }

    /// One frame in, the calls it makes out.
    pub fn step(&mut self, frame: &Frame) -> Vec<Event> {
        let c = self.config;
        let t = frame.t;
        if !frame.live {
            self.reset();
            return Vec::new();
        }
        if let Some(last) = self.last_t {
            if t <= last {
                return Vec::new();
            }
            if t - last > MAX_GAP_S {
                self.reset();
            }
        }
        if let Some(last) = self.own.last() {
            if self.jumped(last, t, frame.own.pos) {
                self.reset();
                self.mute_until = t + c.mute_after_teleport_s;
            }
        }
        self.last_t = Some(t);
        self.own.push(t, frame.own.pos);
        if frame.own.in_pit_lane {
            self.mask.add(frame.own.pos);
        }
        self.track(frame);
        if frame.own.yaw_rate.abs() < c.straight_yaw_rate {
            (self.turning_since, self.corner_counted) = (None, false);
        } else if !self.corner_counted && held(*self.turning_since.get_or_insert(t), t, c.corner_min_s) {
            self.straight += 1;
            self.corner_counted = true;
        }

        self.pose = self.find_pose(frame);
        let own_v = self.velocity(&self.own);
        let exit = self.pit_exit(frame, own_v);
        let quiet = t < self.mute_until || frame.own.in_pit_lane || frame.own.in_pit_box || frame.own.speed <= c.min_speed_ms;
        let (Some(pose), Some(own_v), false) = (self.pose, own_v, quiet) else {
            self.silence();
            for track in self.tracks.values_mut() {
                (track.long, track.lat) = (f32::NAN, f32::NAN);
            }
            return exit.into_iter().collect();
        };

        let mut calls = Vec::new();
        for car in &frame.others {
            let v = self.tracks.get(&car.id).and_then(|track| self.velocity(&track.history));
            let masked = self.mask.contains(car.pos);
            let track = self.tracks.get_mut(&car.id).expect("tracked above");
            let d = [car.pos[0] - frame.own.pos[0], car.pos[2] - frame.own.pos[2]];
            let (long, lat, dy) = (dot(d, pose.forward), dot(d, pose.right), car.pos[1] - frame.own.pos[1]);
            (track.long, track.lat) = (long, lat);
            let parked = track.slow_since.is_some_and(|since| t - since > c.parked_after_s);
            let level = dy.abs() < c.height_max_m;
            let rel = v.map(|v| [v[0] - own_v[0], v[1] - own_v[1]]);
            let side_of = |lat: f32| if lat < 0.0 { Side::Left } else { Side::Right };

            let enter = level
                && !parked
                && !masked
                && rel.is_some_and(|r| r[0].hypot(r[1]) < c.max_relative_ms)
                && lat.abs() <= c.lateral_max_m
                && long < c.car_length_m
                && long > -(c.car_length_m + c.behind_extra_m);
            let stay = level && lat.abs() <= c.lateral_max_m && long.abs() < c.car_length_m + c.clear_extra_m;
            track.beside = match track.beside {
                Beside::No | Beside::Entering { .. } if !enter => Beside::No,
                Beside::No if c.overlap_delay_s > EPS => Beside::Entering { since: t },
                Beside::No => Beside::Yes { side: side_of(lat) },
                Beside::Entering { since } if held(since, t, c.overlap_delay_s) => Beside::Yes { side: side_of(lat) },
                Beside::Entering { since } => Beside::Entering { since },
                Beside::Yes { side } | Beside::Leaving { side, .. } if stay => {
                    // inside the inner band the car keeps its side
                    Beside::Yes { side: if lat.abs() >= c.inner_lateral_share * c.car_width_m { side_of(lat) } else { side } }
                }
                Beside::Yes { side } => Beside::Leaving { side, since: t },
                Beside::Leaving { since, .. } if held(since, t, c.clear_delay_s) => Beside::No,
                leaving @ Beside::Leaving { .. } => leaving,
            };

            // the calls of §3, for cars with a velocity on the own level and outside the pit mask
            let (Some(v), Some(rel), true, false) = (v, rel, level, masked) else {
                self.limits.entry(car.id).or_default().hold_off();
                continue;
            };
            let limits = self.limits.entry(car.id).or_default();
            let speed = frame.own.speed;
            let closing = dot(rel, pose.forward);
            let on_straight = frame.own.yaw_rate.abs() < c.straight_yaw_rate;
            let behind = -long / speed;
            let ahead = long / speed;
            if long < -(c.car_length_m + c.behind_extra_m)
                && lat.abs() <= c.lateral_max_m
                && behind < c.closing_gap_s
                && closing > c.closing_min_ms
                && (-long - c.car_length_m) / closing < c.closing_reach_s
                && due(limits.closing_fast, t, c.closing_every_s)
            {
                limits.closing_fast = Some(t);
                if frame.own.brake <= c.closing_max_brake {
                    calls.push(Event::ClosingFast { car: car.id, gap_s: behind });
                }
            }
            if long < -c.car_length_m && lat.abs() <= c.in_line_lateral_m && behind < c.close_gap_s {
                let since = *limits.close_since.get_or_insert(t);
                let pressed = limits.close_last.is_some_and(|last| since <= last);
                if held(since, t, c.close_hold_s) && due(limits.close_last, t, if pressed { c.close_repeat_s } else { c.close_every_s }) {
                    limits.close_last = Some(t);
                    calls.push(Event::CloseBehind { car: car.id, gap_s: behind });
                }
            } else {
                limits.close_since = None;
            }
            let slip = |gap: f32| on_straight && speed > c.slip_min_speed_ms && lat.abs() < c.slip_lateral_m && (c.slip_gap_s.0..=c.slip_gap_s.1).contains(&gap);
            if long < -c.car_length_m && slip(behind) {
                let since = *limits.slip_since.get_or_insert(t);
                if held(since, t, c.slip_hold_s) && limits.slip_straight != Some(self.straight) {
                    limits.slip_straight = Some(self.straight);
                    calls.push(Event::SlipstreamBehind { car: car.id });
                }
            } else {
                limits.slip_since = None;
            }
            if long > c.car_length_m && slip(ahead) {
                let since = *limits.tow_since.get_or_insert(t);
                if held(since, t, c.slip_hold_s) && limits.tow_straight != Some(self.straight) {
                    limits.tow_straight = Some(self.straight);
                    calls.push(Event::InTow { car: car.id });
                }
            } else {
                limits.tow_since = None;
            }
            let other = v[0].hypot(v[1]);
            if long > c.car_length_m
                && long <= c.slow_range_m
                && lat.abs() <= c.in_line_lateral_m
                && (other < c.slow_share * speed || other < c.slow_speed_ms)
                && speed - other > c.slow_closing_ms
                && due(limits.slow_ahead, t, c.slow_every_s)
            {
                limits.slow_ahead = Some(t);
                calls.push(Event::SlowCarAhead { car: car.id });
            }
        }

        let mut events = self.announce(t);
        events.extend(calls);
        events.extend(exit);
        events
    }

    /// While the own car is at the pit exit: the faster car closing from behind soonest, once an exit; at the first
    /// sample of the exit without such a car, "clear" when no car is near or comes within the clear window.
    fn pit_exit(&mut self, frame: &Frame, own_v: Option<[f32; 2]>) -> Option<Event> {
        if !frame.own.pit_exit {
            (self.pit_exit_said, self.pit_exit_judged) = (false, false);
            return None;
        }
        let (c, own_v) = (self.config, own_v?);
        if self.pit_exit_said {
            return None;
        }
        let own_speed = own_v[0].hypot(own_v[1]);
        // the cars outside the pit mask moving the own car's way: id, distance, speed, time to reach the own car
        let cars: Vec<(CarId, f32, f32, f32)> = frame
            .others
            .iter()
            .filter(|car| !self.mask.contains(car.pos))
            .filter_map(|car| {
                let v = self.velocity(&self.tracks.get(&car.id)?.history)?;
                let speed = v[0].hypot(v[1]);
                if speed <= c.parked_speed_ms || dot(v, own_v) <= 0.0 {
                    return None;
                }
                let d = [car.pos[0] - frame.own.pos[0], car.pos[2] - frame.own.pos[2]];
                let dist = d[0].hypot(d[1]);
                let closing = -dot([v[0] - own_v[0], v[1] - own_v[1]], d) / dist;
                Some((car.id, dist, speed, if closing > 0.0 { dist / closing } else { f32::INFINITY }))
            })
            .collect();
        let traffic = cars
            .iter()
            .filter(|(_, dist, speed, reach)| *speed > own_speed && *dist <= c.pit_exit_range_m && *reach <= c.pit_exit_reach_s)
            .min_by(|a, b| a.3.total_cmp(&b.3));
        if let Some((car, ..)) = traffic {
            self.pit_exit_said = true;
            return Some(Event::PitExitTraffic { car: *car });
        }
        if std::mem::replace(&mut self.pit_exit_judged, true) {
            return None;
        }
        let near = cars.iter().any(|(_, dist, _, reach)| *dist <= c.pit_exit_near_m || (*dist <= c.pit_exit_clear_range_m && *reach <= c.pit_exit_clear_reach_s));
        (!near).then_some(Event::PitExitClear)
    }

    /// Follows every car by its id: a new id, or a teleport, starts its history afresh; an id that is gone is dropped.
    fn track(&mut self, frame: &Frame) {
        self.tracks.retain(|id, _| frame.others.iter().any(|car| car.id == *id));
        for car in &frame.others {
            let jumped = self.tracks.get(&car.id).and_then(|track| track.history.last()).is_some_and(|last| self.jumped(last, frame.t, car.pos));
            let (window, k, slow) = (self.config.velocity_window_s, self.config.velocity_samples, self.config.parked_speed_ms);
            let track = self.tracks.entry(car.id).or_insert_with(Track::new);
            if jumped {
                *track = Track::new();
            }
            track.history.push(frame.t, car.pos);
            match track.history.velocity(window, k) {
                Some(v) if v[0].hypot(v[1]) < slow => {
                    track.slow_since.get_or_insert(frame.t);
                }
                Some(_) => track.slow_since = None,
                None => {}
            }
        }
    }

    /// The cars alongside on each side (0, 1 or 2: two only when they are side by side, more than a car width apart),
    /// with the one nearest along the car, turned into calls by the state machine of research §2.1 step 6.
    fn announce(&mut self, t: f64) -> Vec<Event> {
        let c = self.config;
        let mut near: [Vec<(CarId, f32, f32)>; 2] = [Vec::new(), Vec::new()];
        for (id, track) in &self.tracks {
            if let Beside::Yes { side } | Beside::Leaving { side, .. } = track.beside {
                near[index(side)].push((*id, track.long.abs(), track.lat.abs()));
            }
        }
        let count = |cars: &[(CarId, f32, f32)]| -> u8 {
            let wide = cars.iter().any(|a| cars.iter().any(|b| (a.2 - b.2).abs() > c.car_width_m));
            match cars.len() {
                0 => 0,
                _ if wide => 2,
                _ => 1,
            }
        };
        let nearest = |cars: &[(CarId, f32, f32)]| cars.iter().min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0))).map(|car| car.0);
        let (l, r) = (count(&near[0]), count(&near[1]));
        let [pl, pr] = self.said;

        let mut events = Vec::new();
        let (clear_l, clear_r) = (l == 0 && pl > 0, r == 0 && pr > 0);
        match (clear_l, clear_r) {
            (true, true) => events.push(Event::ClearAll),
            (true, false) => events.push(Event::ClearLeft),
            (false, true) => events.push(Event::ClearRight),
            (false, false) => {}
        }
        for (cleared, i) in [(clear_l, 0), (clear_r, 1)] {
            if cleared {
                self.cleared_at[i] = t;
            }
        }
        let (pl, pr) = (if clear_l { 0 } else { pl }, if clear_r { 0 } else { pr });
        // a side that grows waits out the bounce guard after its last "clear"
        let guarded = |i: usize| t - self.cleared_at[i] < c.bounce_s - EPS;
        let nl = if l > pl && guarded(0) { pl } else { l };
        let nr = if r > pr && guarded(1) { pr } else { r };
        let mut called = [false; 2];
        if nl > 0 && nr > 0 {
            if !(pl > 0 && pr > 0) {
                events.push(Event::ThreeWide);
                called = [true, true];
            }
        } else if nl > 0 {
            if nl == 2 && pl < 2 {
                events.push(Event::ThreeWideOnRight);
                called[0] = true;
            } else if nl == 1 && pl == 0 {
                events.push(Event::CarLeft { car: nearest(&near[0]).expect("a car on the left") });
                called[0] = true;
            }
        } else if nr > 0 {
            if nr == 2 && pr < 2 {
                events.push(Event::ThreeWideOnLeft);
                called[1] = true;
            } else if nr == 1 && pr == 0 {
                events.push(Event::CarRight { car: nearest(&near[1]).expect("a car on the right") });
                called[1] = true;
            }
        }
        self.said = [nl, nr];
        for (still, called) in self.still.iter_mut().zip(called) {
            if called {
                *still = (t, 0);
            }
        }
        // "still there" every few seconds, at most twice; one call covers both sides
        let mut stilled = false;
        for (i, side) in [(0, Side::Left), (1, Side::Right)] {
            let (last, n) = self.still[i];
            if self.said[i] > 0 && n < c.still_there_max && held(last, t, c.still_there_every_s) {
                if !stilled {
                    events.push(Event::StillThere { side });
                }
                stilled = true;
                self.still[i] = (t, n + 1);
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{arc, events, keys, run, run_with, straight, Drive, Inputs, OWN};
    use super::*;

    const A: CarId = (0, 1);
    const B: CarId = (0, 2);

    fn sign(side: Side) -> f32 {
        if side == Side::Left { -1.0 } else { 1.0 }
    }

    fn car_on(side: Side, car: CarId) -> Event {
        if side == Side::Left { Event::CarLeft { car } } else { Event::CarRight { car } }
    }

    fn clear(side: Side) -> Event {
        if side == Side::Left { Event::ClearLeft } else { Event::ClearRight }
    }

    /// A car comes up from 12 m behind to level with the driver, 3 m to the side (3.7 m/s faster), and drops back
    /// (ending before it would be "close behind" for 3 s).
    fn alongside(side: Side) -> Drive {
        let s = sign(side);
        Drive::new(straight(40.0, 30.0)).seconds(6.0).beside(1, move |t| Some((keys(t, &[(0.0, -12.0), (3.0, -0.9), (6.0, -12.0)]), 3.0 * s, 0.0)))
    }

    #[test]
    fn a_car_alongside_and_dropping_back_is_called_once_and_cleared_once_on_either_side() {
        for side in [Side::Left, Side::Right] {
            let timeline = run(&alongside(side).frames());
            assert_eq!(events(&timeline), vec![car_on(side, A), clear(side)], "{side:?}");
            // in the box from long -5.1 (t 1.86 s: the sample at 1.9 plus the 50 ms overlap delay), out of it from
            // long -5.2 (t 4.16 s: the sample at 4.2 plus the 150 ms clear delay)
            assert!((timeline[0].0 - 1.95).abs() < 0.01, "{timeline:?}");
            assert!((timeline[1].0 - 4.35).abs() < 0.01, "{timeline:?}");
        }
    }

    #[test]
    fn the_same_calls_with_jitter_and_a_graphics_page_a_frame_late() {
        for side in [Side::Left, Side::Right] {
            let timeline = run(&alongside(side).jitter(0.05).lag(1.0 / 60.0).frames());
            assert_eq!(events(&timeline), vec![car_on(side, A), clear(side)], "{side:?}");
        }
    }

    #[test]
    fn all_three_pose_sources_give_the_same_sides_on_a_straight_and_in_a_corner() {
        for side in [Side::Left, Side::Right] {
            let s = sign(side);
            for inputs in [Inputs::Contact, Inputs::Velocity, Inputs::Positions] {
                for right_turn in [true, false] {
                    let drive = Drive::new(arc(30.0, 150.0, right_turn)).seconds(6.0).inputs(inputs).beside(1, move |t| Some((if t < 3.0 { -8.0 + 2.5 * t as f32 } else { -0.5 }, 3.0 * s, 0.0)));
                    let mut spotter = Spotter::new(Config::default());
                    let timeline = run_with(&mut spotter, &drive.frames());
                    assert_eq!(events(&timeline)[0], car_on(side, A), "{side:?} {inputs:?} right turn {right_turn}");
                    let source = match inputs {
                        Inputs::Contact => PoseSource::Contact,
                        Inputs::Velocity => PoseSource::Velocity,
                        Inputs::Positions => PoseSource::PositionDelta,
                    };
                    assert_eq!(spotter.pose().unwrap().source, source);
                }
            }
        }
    }

    #[test]
    fn the_handedness_flag_flips_the_fallback_sides_but_not_the_contact_points() {
        let drive = |inputs| Drive::new(straight(40.0, 200.0)).seconds(4.0).inputs(inputs).beside(1, |_| Some((0.0, 3.0, 0.0)));
        let left_handed = Config { right_handed: false, ..Config::default() };
        let first = |inputs| events(&run_with(&mut Spotter::new(left_handed), &drive(inputs).frames()))[0];
        assert_eq!(first(Inputs::Contact), Event::CarRight { car: A });
        assert_eq!(first(Inputs::Velocity), Event::CarLeft { car: A });
        assert_eq!(first(Inputs::Positions), Event::CarLeft { car: A });
    }

    #[test]
    fn contact_points_that_are_not_sane_fall_back_to_the_velocity() {
        let mut frames = Drive::new(straight(40.0, 10.0)).seconds(3.0).beside(1, |_| Some((0.0, -3.0, 0.0))).frames();
        for frame in &mut frames {
            let contact = frame.own.contact.as_mut().unwrap();
            // a wheelbase of 27 m
            contact[0] = [contact[0][0] * 10.0, contact[0][1], contact[0][2] * 10.0];
        }
        let mut spotter = Spotter::new(Config::default());
        assert_eq!(events(&run_with(&mut spotter, &frames)), vec![Event::CarLeft { car: A }]);
        assert_eq!(spotter.pose().unwrap().source, PoseSource::Velocity);
    }

    #[test]
    fn cars_on_both_sides_are_three_wide_and_clear_all_round_together() {
        let back = |t| keys(t, &[(4.0, 0.0), (7.0, -12.0)]);
        let drive = Drive::new(straight(40.0, 75.0)).seconds(8.0).beside(1, move |t| Some((back(t), -3.0, 0.0))).beside(2, move |t| Some((back(t), 2.8, 0.0)));
        let timeline = run(&drive.frames());
        assert_eq!(events(&timeline), vec![Event::ThreeWide, Event::StillThere { side: Side::Left }, Event::ClearAll], "{timeline:?}");
    }

    #[test]
    fn a_car_joining_on_the_other_side_makes_three_wide_and_leaving_clears_only_its_side() {
        let drive = Drive::new(straight(40.0, 75.0))
            .seconds(11.0)
            .beside(1, |t| Some((keys(t, &[(8.0, 0.0), (10.0, -10.0)]), -3.0, 0.0)))
            .beside(2, |t| Some((keys(t, &[(0.0, -9.0), (2.0, 0.0), (2.5, 0.0), (4.5, -40.0)]), 3.0, 0.0)));
        let timeline = run(&drive.frames());
        assert_eq!(
            events(&timeline),
            vec![Event::CarLeft { car: A }, Event::ThreeWide, Event::ClearRight, Event::StillThere { side: Side::Left }, Event::StillThere { side: Side::Left }, Event::ClearLeft],
            "{timeline:?}"
        );
    }

    #[test]
    fn two_cars_side_by_side_on_one_side_say_where_the_driver_is_but_nose_to_tail_is_one_car() {
        for side in [Side::Left, Side::Right] {
            let s = sign(side);
            let wide = Drive::new(straight(40.0, 0.0)).seconds(2.0).beside(1, move |_| Some((0.5, 2.5 * s, 0.0))).beside(2, move |_| Some((-0.5, 5.0 * s, 0.0)));
            let expected = if side == Side::Left { Event::ThreeWideOnRight } else { Event::ThreeWideOnLeft };
            assert_eq!(events(&run(&wide.frames())), vec![expected], "{side:?}");
            let nose_to_tail = Drive::new(straight(40.0, 0.0)).seconds(2.0).beside(1, move |_| Some((2.0, 3.0 * s, 0.0))).beside(2, move |_| Some((-3.5, 3.4 * s, 0.0)));
            assert_eq!(events(&run(&nose_to_tail.frames())), vec![car_on(side, A)], "{side:?}");
        }
    }

    #[test]
    fn still_there_every_three_seconds_at_most_twice() {
        let drive = Drive::new(straight(40.0, 0.0)).seconds(15.0).beside(1, |_| Some((0.0, 3.0, 0.0)));
        let timeline = run(&drive.frames());
        let times: Vec<f64> = timeline.iter().map(|(t, _)| (t * 100.0).round() / 100.0).collect();
        // the car has a velocity after 0.3 s, plus the overlap delay
        assert_eq!(events(&timeline), vec![Event::CarRight { car: A }, Event::StillThere { side: Side::Right }, Event::StillThere { side: Side::Right }]);
        assert_eq!(times, vec![0.35, 3.35, 6.35]);
    }

    #[test]
    fn flicker_at_the_edge_of_the_box_does_not_spam() {
        // inside the hysteresis: between one length and one length plus the clear margin
        let small = Drive::new(straight(40.0, 0.0)).seconds(10.0).beside(1, |t| Some((-4.5 - 0.5 * (t as f32 * 7.0).sin().abs(), -3.0, 0.0)));
        assert_eq!(events(&run(&small.frames())), vec![Event::CarLeft { car: A }, Event::StillThere { side: Side::Left }, Event::StillThere { side: Side::Left }]);
        // out of the box for 100 ms at a time: shorter than the clear delay
        let quick = Drive::new(straight(40.0, 0.0)).seconds(10.0).beside(1, |t| Some((if (t * 10.0).round() as i64 % 4 == 0 { -6.0 } else { -3.0 }, -3.0, 0.0)));
        assert_eq!(events(&run(&quick.frames())).first(), Some(&Event::CarLeft { car: A }));
        assert!(!events(&run(&quick.frames())).contains(&Event::ClearLeft));
        // in and out every 0.5 s: the bounce guard holds a side's next call until 1.5 s after its "clear"
        let slow = Drive::new(straight(40.0, 0.0)).seconds(12.0).beside(1, |t| Some((if (t * 2.0) as i64 % 2 == 0 { -3.0 } else { -6.5 }, -3.0, 0.0)));
        let timeline = run(&slow.frames());
        assert!(timeline.iter().filter(|(_, e)| *e == Event::ClearLeft).count() >= 2, "it does bounce: {timeline:?}");
        for pair in timeline.windows(2) {
            if pair[0].1 == Event::ClearLeft {
                assert_eq!(pair[1].1, Event::CarLeft { car: A });
                assert!(pair[1].0 - pair[0].0 >= 1.5 - 1e-6, "{timeline:?}");
            }
        }
    }

    #[test]
    fn a_car_on_a_bridge_above_is_never_alongside() {
        let drive = Drive::new(straight(40.0, 0.0)).seconds(5.0).beside(1, |t| Some((-10.0 + 4.0 * t as f32, 2.0, 5.0)));
        assert_eq!(events(&run(&drive.frames())), vec![]);
    }

    #[test]
    fn parked_cars_beside_the_track_are_never_alongside() {
        // Touristenfahrten: cars standing 4.5 to 5.5 m beside the road while the driver rolls past just above the speed
        // gate, slower than the relative speed gate
        let mut drive = Drive::new(straight(11.0, 0.0)).seconds(20.0);
        for (i, along) in [60.0f32, 66.0, 72.0, 150.0].into_iter().enumerate() {
            let lat = if i % 2 == 0 { 4.5 } else { -5.5 };
            drive = drive.car(10 + i as u64, move |_, _| Some(straight(0.0, 0.0)(0.0).at(along, lat, 0.0)));
        }
        assert_eq!(events(&run(&drive.frames())), vec![]);
    }

    #[test]
    fn silent_in_the_pit_lane_and_cars_in_the_learned_pit_mask_are_ignored() {
        let pit = |speed: f32| move |t: f64| straight(speed, 0.0)(t).at(0.0, 0.0, 0.0);
        // the own car drives down the pit lane with a car alongside: silent, and it learns the lane
        let lane = Drive::new(straight(20.0, 0.0)).seconds(10.0).pit_lane(|_| true).beside(1, |_| Some((0.0, -3.0, 0.0)));
        let mut spotter = Spotter::new(Config::default());
        assert_eq!(events(&run_with(&mut spotter, &lane.frames())), vec![]);
        let cells = spotter.pit_mask().cells();
        assert!(cells.len() >= 40, "200 m of lane in 4 m cells: {}", cells.len());

        // later on the track 5 m beside the lane, a car drives down the lane alongside
        let track = || {
            Drive::new(move |t| {
                let mut truth = straight(21.0, 0.0)(t);
                truth.pos = truth.at(-2.0, 5.0, 0.0);
                truth
            })
            .seconds(2.5)
            .car(2, move |t, _| Some(pit(21.0)(t)))
        };
        let mut fresh = Spotter::new(Config::default());
        fresh.set_pit_mask(PitMask::from_cells(&cells));
        assert_eq!(events(&run_with(&mut fresh, &track().frames())), vec![], "the stored mask, read back");
        assert_eq!(events(&run(&track().frames())), vec![Event::CarLeft { car: B }], "without the mask");
    }

    #[test]
    fn at_the_pit_exit_a_faster_car_closing_from_behind_is_called_once() {
        // A2 573.6 s: car_location 3 (the pit exit) at 56 km/h; the car 84 m away at 134 km/h closing at 38 m/s there
        // drove the other way (see below), these cars come from behind
        let exit = |behind: f32, lat: f32| {
            Drive::new(straight(16.0, 0.0)).seconds(8.0).pit_lane(|_| true).pit_exit(|t| t >= 5.0).beside(1, move |t| Some((behind + 24.0 * (t as f32 - 5.0), lat, 0.0)))
        };
        // the track runs 8 m beside the pit lane: a car on it is outside the pit mask
        let timeline = run(&exit(-80.0, 8.0).frames());
        assert_eq!(events(&timeline), vec![Event::PitExitTraffic { car: A }], "80 m behind, 24 m/s faster: 3.3 s");
        assert!((timeline[0].0 - 5.0).abs() < 1e-9, "at the first sample of the exit");
        // 120 m behind is 5 s away: called once it is 4 s away, while the own car is still at the exit
        let timeline = run(&exit(-120.0, 8.0).frames());
        assert_eq!(events(&timeline), vec![Event::PitExitTraffic { car: A }]);
        assert!((timeline[0].0 - 6.0).abs() < 0.11, "{}", timeline[0].0);
        assert_eq!(events(&run(&exit(-250.0, 8.0).frames())), vec![Event::PitExitClear], "10.4 s away: clear, still 7.4 s away when the exit ends");
        // a slower car ahead the own car catches is no traffic from behind
        let ahead = Drive::new(straight(16.0, 0.0)).seconds(8.0).pit_lane(|_| true).pit_exit(|t| t >= 5.0).beside(1, |t| Some((40.0 - 8.0 * t as f32, 8.0, 0.0)));
        assert_eq!(events(&run(&ahead.frames())), vec![]);
        // not at the exit yet: the spotter is silent in the pit lane
        let lane = Drive::new(straight(16.0, 0.0)).seconds(8.0).pit_lane(|_| true).beside(1, |t| Some((-80.0 + 24.0 * (t as f32 - 5.0), 8.0, 0.0)));
        assert_eq!(events(&run(&lane.frames())), vec![]);
        // a car in the pit lane behind (in the learned pit mask) is not traffic, one beside the lane is
        let two = Drive::new(straight(16.0, 0.0))
            .seconds(8.0)
            .pit_lane(|_| true)
            .pit_exit(|t| t >= 5.0)
            .beside(1, |t| Some((-40.0 + 24.0 * (t as f32 - 5.0), 0.0, 0.0)))
            .beside(2, |t| Some((-60.0 + 24.0 * (t as f32 - 5.0), 8.0, 0.0)));
        assert_eq!(events(&run(&two.frames())), vec![Event::PitExitTraffic { car: B }]);
    }

    #[test]
    fn the_pit_exit_is_clear_when_no_car_comes_within_eight_seconds_and_a_car_coming_later_is_still_called() {
        let lane = || Drive::new(straight(16.0, 0.0)).pit_lane(|_| true);
        // nothing on the track: clear at the first sample of the exit
        let timeline = run(&lane().seconds(8.0).pit_exit(|t| t >= 5.0).frames());
        assert_eq!(events(&timeline), vec![Event::PitExitClear]);
        assert!((timeline[0].0 - 5.0).abs() < 1e-9);
        // 240 m behind, 24 m/s faster: 10 s away, clear; the warning still comes once it is 4 s away
        let far = lane().seconds(12.0).pit_exit(|t| t >= 5.0).beside(1, |t| Some((-240.0 + 24.0 * (t as f32 - 5.0), 8.0, 0.0)));
        let timeline = run(&far.frames());
        assert_eq!(events(&timeline), vec![Event::PitExitClear, Event::PitExitTraffic { car: A }]);
        assert!((timeline[1].0 - 11.0).abs() < 0.11, "{}", timeline[1].0);
        // a car that joins the session after the clear
        let late = lane().seconds(8.0).pit_exit(|t| t >= 5.0).beside(1, |t| (t >= 6.0).then(|| (-100.0 + 24.0 * (t as f32 - 6.0), 8.0, 0.0)));
        assert_eq!(events(&run(&late.frames())), vec![Event::PitExitClear, Event::PitExitTraffic { car: A }]);
        // a car standing beside the exit (a pit box) is no traffic
        let parked = lane().seconds(8.0).pit_exit(|t| t >= 5.0).beside(1, |t| Some((100.0 - 16.0 * t as f32, 8.0, 0.0)));
        assert_eq!(events(&run(&parked.frames())), vec![Event::PitExitClear]);
    }

    #[test]
    fn no_pit_exit_clear_while_a_car_comes_within_eight_seconds_or_is_near() {
        let exit = |car: Box<dyn Fn(f64) -> Option<(f32, f32, f32)>>| {
            Drive::new(straight(16.0, 0.0)).seconds(5.5).pit_lane(|_| true).pit_exit(|t| t >= 5.0).beside(1, car)
        };
        // 150 m behind, 24 m/s faster: 6.3 s away, outside the warning's 4 s while the exit lasts
        assert_eq!(events(&run(&exit(Box::new(|t| Some((-150.0 + 24.0 * (t as f32 - 5.0), 8.0, 0.0)))).frames())), vec![]);
        // on the track beside the exit at the own speed: closes on nothing, but is there
        assert_eq!(events(&run(&exit(Box::new(|_| Some((-10.0, 8.0, 0.0)))).frames())), vec![]);
        // a slower car 30 m ahead on the track
        assert_eq!(events(&run(&exit(Box::new(|t| Some((30.0 - 4.0 * (t as f32 - 5.0), 8.0, 0.0)))).frames())), vec![]);
    }

    #[test]
    fn a_car_the_other_way_on_a_track_beside_the_exit_is_no_traffic() {
        // A2 573.6 s at Brands Hatch: the car called "coming" drove the other way on the straight beside the exit
        // road (heading -78° against the own car's +113°), 9 m below, 61 m away when they passed; the cars on the
        // own way were all ahead and slower
        let oncoming = Drive::new(straight(16.0, 0.0))
            .seconds(8.0)
            .pit_lane(|_| true)
            .pit_exit(|t| t >= 5.0)
            .beside(1, |t| Some((70.0 - 54.0 * (t as f32 - 5.0), -45.0, -8.0)));
        assert_eq!(events(&run(&oncoming.frames())), vec![Event::PitExitClear]);
    }

    #[test]
    fn a_teleport_of_the_own_car_resets_everything_and_mutes_two_seconds() {
        let drive = Drive::new(|t| {
            let mut truth = straight(40.0, 0.0)(t);
            if t >= 5.0 {
                truth.pos = truth.at(0.0, 800.0, 0.0);
            }
            truth
        })
        .seconds(10.0)
        .beside(1, |t| (t < 5.0).then_some((0.0, -3.0, 0.0)))
        .beside(2, |t| (t >= 5.0).then_some((0.5, 3.0, 0.0)));
        let timeline = run(&drive.frames());
        assert_eq!(events(&timeline), vec![Event::CarLeft { car: A }, Event::StillThere { side: Side::Left }, Event::CarRight { car: B }], "no clear for the car left behind");
        assert!(timeline[2].0 >= 7.0 - 1e-6 && timeline[2].0 < 7.1, "{timeline:?}");
    }

    #[test]
    fn a_car_that_teleports_in_beside_is_called_only_once_it_has_a_velocity() {
        // joins at 5 s right beside the driver at the same speed
        let joins = Drive::new(straight(40.0, 0.0)).seconds(8.0).beside(1, |t| (t >= 5.0).then_some((0.0, -3.0, 0.0)));
        let timeline = run(&joins.frames());
        assert_eq!(events(&timeline), vec![Event::CarLeft { car: A }]);
        assert!(timeline[0].0 >= 5.3 - 1e-6, "{timeline:?}");
        // the same id jumps from far away to beside the driver and stands there (a respawn): never alongside
        let respawn = Drive::new(straight(40.0, 0.0)).seconds(8.0).car(1, |t, own| Some(if t < 5.0 { own.at(-300.0, 0.0, 0.0) } else { straight(0.0, 0.0)(0.0).at(200.0, -3.0, 0.0) }));
        assert_eq!(events(&run(&respawn.frames())), vec![]);
    }

    #[test]
    fn a_car_blasting_past_is_closing_fast_before_it_arrives_but_never_alongside() {
        for side in [Side::Left, Side::Right] {
            let s = sign(side);
            let drive = Drive::new(straight(40.0, 120.0)).seconds(8.0).beside(1, move |t| Some((-80.0 + 20.0 * t as f32, 3.0 * s, 0.0)));
            let timeline = run(&drive.frames());
            assert_eq!(events(&timeline).len(), 1, "{timeline:?}");
            let (t, Event::ClosingFast { car, gap_s }) = timeline[0] else { panic!("{timeline:?}") };
            assert_eq!(car, A);
            // it reaches the own car's tail (long -4.7) at 3.77 s; the warning comes 3 s before that at the latest
            assert!(t < 3.77 - 2.0, "{t}");
            assert!(gap_s > 0.0 && gap_s < 2.5, "{gap_s}");
        }
    }

    #[test]
    fn a_car_closing_while_the_driver_brakes_is_not_called_and_not_after_the_brake_either() {
        // the recording of 2026-11-09: 17 of 20 "closing fast" came in braking zones, from cars that braked later
        // and stayed 9 to 26 m behind; with only the brake gate they came at the brake's release instead
        let mut frames = Drive::new(straight(40.0, 120.0)).seconds(8.0).beside(1, |t| Some((-80.0 + 20.0 * t as f32, 3.0, 0.0))).frames();
        for frame in frames.iter_mut().filter(|f| f.t < 2.0) {
            frame.own.brake = 1.0;
        }
        let timeline = run(&frames);
        assert!(!events(&timeline).iter().any(|e| matches!(e, Event::ClosingFast { .. })), "{timeline:?}");
    }

    #[test]
    fn a_car_close_behind_is_called_after_three_seconds_and_again_after_thirty_of_pressure() {
        let drive = Drive::new(straight(40.0, 0.0)).seconds(40.0).beside(1, |_| Some((-16.0, 0.3, 0.0)));
        let timeline = run(&drive.frames());
        let close: Vec<(f64, Event)> = timeline.iter().copied().filter(|(_, e)| matches!(e, Event::CloseBehind { .. })).collect();
        assert_eq!(close.len(), 2, "{timeline:?}");
        assert!((close[0].0 - 3.3).abs() < 0.06 && (close[1].0 - 33.3).abs() < 0.06, "{close:?}");
        assert!(matches!(close[0].1, Event::CloseBehind { car: A, gap_s } if (gap_s - 0.4).abs() < 1e-3), "{close:?}");
    }

    #[test]
    fn slipstream_both_ways_once_per_straight_and_not_in_a_corner() {
        let behind = Drive::new(straight(50.0, 0.0)).seconds(10.0).beside(1, |_| Some((-15.0, 0.3, 0.0)));
        let timeline = run(&behind.frames());
        let slip: Vec<&(f64, Event)> = timeline.iter().filter(|(_, e)| matches!(e, Event::SlipstreamBehind { .. })).collect();
        assert_eq!(slip.len(), 1, "{timeline:?}");
        assert!((slip[0].0 - 1.8).abs() < 0.06 && slip[0].1 == Event::SlipstreamBehind { car: A }, "{slip:?}");

        let ahead = Drive::new(straight(50.0, 0.0)).seconds(10.0).beside(1, |_| Some((15.0, -0.3, 0.0)));
        assert_eq!(events(&run(&ahead.frames())), vec![Event::InTow { car: A }]);

        let corner = Drive::new(arc(50.0, 400.0, true)).seconds(10.0).beside(1, |_| Some((15.0, -0.3, 0.0)));
        assert_eq!(events(&run(&corner.frames())), vec![], "0.125 rad/s is no straight");
    }

    #[test]
    fn a_slow_car_ahead_is_called_once_within_150_m() {
        // the driver at 40 m/s comes up to a car at 10 m/s, 300 m ahead
        let drive = Drive::new(straight(40.0, 45.0)).seconds(9.0).car(1, |t, _| Some(straight(10.0, 45.0)(t).at(300.0, 1.0, 0.0)));
        let timeline = run(&drive.frames());
        assert_eq!(events(&timeline).first(), Some(&Event::SlowCarAhead { car: A }), "{timeline:?}");
        assert_eq!(timeline.iter().filter(|(_, e)| matches!(e, Event::SlowCarAhead { .. })).count(), 1);
        // 300 - 30 t = 150 at t 5 s
        assert!((timeline[0].0 - 5.0).abs() < 0.06, "{timeline:?}");
    }

    #[test]
    fn silent_below_the_speed_gate_and_outside_a_live_session() {
        let slow = Drive::new(straight(9.0, 0.0)).seconds(5.0).beside(1, |_| Some((0.0, 3.0, 0.0)));
        assert_eq!(events(&run(&slow.frames())), vec![]);
        let mut frames = Drive::new(straight(40.0, 0.0)).seconds(5.0).beside(1, |_| Some((0.0, 3.0, 0.0))).frames();
        for frame in &mut frames {
            frame.live = false;
        }
        assert_eq!(events(&run(&frames)), vec![]);
    }

    #[test]
    fn the_own_car_is_not_in_its_own_list_and_its_offsets_are_readable() {
        let drive = Drive::new(straight(40.0, 0.0)).seconds(1.0).beside(1, |_| Some((1.5, -3.0, 0.0)));
        let mut spotter = Spotter::new(Config::default());
        run_with(&mut spotter, &drive.frames());
        let (long, lat) = spotter.offset(A).unwrap();
        assert!((long - 1.5).abs() < 1e-3 && (lat + 3.0).abs() < 1e-3, "{long} {lat}");
        assert_eq!(spotter.offset(OWN), None);
        assert!(spotter.overlap());
    }
}
