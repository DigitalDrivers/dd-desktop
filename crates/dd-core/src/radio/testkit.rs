//! Test data for the radio: the own car drives a straight or an arc, other cars sit at chosen offsets in its frame
//! (or at chosen places in the world), and the shell's samples come out as frames, 20 a second by default, with the
//! position jitter and the graphics page's lag of a real session when asked for.
//!
//! The test world is right-handed with Y up: right of a forward direction (x, z) is (-z, x), what the spotter assumes
//! by default when it has no contact points. The contact points the frames carry follow the same world.

use super::frame::{Car, CarId, Frame, Own};
use super::spotter::{Config, Event, Spotter};

pub const OWN: CarId = (0, 1000);
const WHEELBASE: f32 = 2.7;
const TRACK_WIDTH: f32 = 1.6;
/// The height of the test track
const GROUND: f32 = 5.0;

/// Where the own car is at a time: what the frames are made from.
#[derive(Debug, Clone, Copy)]
pub struct Truth {
    pub pos: [f32; 3],
    /// Unit vector in the x/z plane
    pub fwd: [f32; 2],
    pub speed: f32,
    pub yaw_rate: f32,
}

impl Truth {
    pub fn right(&self) -> [f32; 2] {
        [-self.fwd[1], self.fwd[0]]
    }

    /// The point `long` m ahead of the car, `lat` m to its right and `up` m above it.
    pub fn at(&self, long: f32, lat: f32, up: f32) -> [f32; 3] {
        let r = self.right();
        [self.pos[0] + long * self.fwd[0] + lat * r[0], self.pos[1] + up, self.pos[2] + long * self.fwd[1] + lat * r[1]]
    }
}

/// A straight at `speed` m/s; the heading is an angle in the x/z plane, so the tests are not aligned with an axis.
pub fn straight(speed: f32, heading_deg: f32) -> impl Fn(f64) -> Truth {
    let h = heading_deg.to_radians();
    let fwd = [h.cos(), h.sin()];
    move |t| Truth { pos: [120.0 + fwd[0] * speed * t as f32, GROUND, -40.0 + fwd[1] * speed * t as f32], fwd, speed, yaw_rate: 0.0 }
}

/// A circle of `radius` m around the origin at `speed` m/s, turning right or left.
pub fn arc(speed: f32, radius: f32, right_turn: bool) -> impl Fn(f64) -> Truth {
    let s = if right_turn { 1.0 } else { -1.0 };
    let omega = speed / radius;
    move |t| {
        let a = s * omega * t as f32;
        Truth { pos: [radius * a.cos(), GROUND, radius * a.sin()], fwd: [-s * a.sin(), s * a.cos()], speed, yaw_rate: s * omega }
    }
}

/// Which of the pose inputs the frames carry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Inputs {
    /// Contact points and velocity
    Contact,
    /// Velocity only, the contact points are zero
    Velocity,
    /// Neither: only the positions
    Positions,
}

type CarAt = Box<dyn Fn(f64, &Truth) -> Option<[f32; 3]>>;

/// A drive, sampled into frames.
pub struct Drive {
    pub hz: f64,
    pub seconds: f64,
    own: Box<dyn Fn(f64) -> Truth>,
    inputs: Inputs,
    jitter_m: f32,
    lag_s: f64,
    pit_lane: Box<dyn Fn(f64) -> bool>,
    cars: Vec<(CarId, CarAt)>,
}

impl Drive {
    pub fn new(own: impl Fn(f64) -> Truth + 'static) -> Self {
        Self { hz: 20.0, seconds: 10.0, own: Box::new(own), inputs: Inputs::Contact, jitter_m: 0.0, lag_s: 0.0, pit_lane: Box::new(|_| false), cars: Vec::new() }
    }

    pub fn seconds(mut self, seconds: f64) -> Self {
        self.seconds = seconds;
        self
    }

    pub fn inputs(mut self, inputs: Inputs) -> Self {
        self.inputs = inputs;
        self
    }

    /// Adds up to `m` metres to each other car's x and z in every sample.
    pub fn jitter(mut self, m: f32) -> Self {
        self.jitter_m = m;
        self
    }

    /// The graphics page (all positions) shows the cars `s` seconds late; the physics page is current.
    pub fn lag(mut self, s: f64) -> Self {
        self.lag_s = s;
        self
    }

    pub fn pit_lane(mut self, when: impl Fn(f64) -> bool + 'static) -> Self {
        self.pit_lane = Box::new(when);
        self
    }

    /// A car at (longitudinal, lateral, height) metres in the own car's frame (ahead, right, up); None while it is
    /// not in the session.
    pub fn beside(self, id: u64, offset: impl Fn(f64) -> Option<(f32, f32, f32)> + 'static) -> Self {
        self.car(id, move |t, own| offset(t).map(|(long, lat, up)| own.at(long, lat, up)))
    }

    /// A car anywhere in the world.
    pub fn car(mut self, id: u64, at: impl Fn(f64, &Truth) -> Option<[f32; 3]> + 'static) -> Self {
        self.cars.push(((0, id), Box::new(at)));
        self
    }

    pub fn frames(&self) -> Vec<Frame> {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
        };
        let samples = (self.seconds * self.hz).round() as usize;
        (0..=samples)
            .map(|k| {
                let t = k as f64 / self.hz;
                let now = (self.own)(t);
                let seen = (self.own)(t - self.lag_s);
                let r = now.right();
                let wheel = |long: f32, lat: f32| [now.pos[0] + long * now.fwd[0] + lat * r[0], now.pos[1], now.pos[2] + long * now.fwd[1] + lat * r[1]];
                let (b, w) = (WHEELBASE / 2.0, TRACK_WIDTH / 2.0);
                let others = self
                    .cars
                    .iter()
                    .filter_map(|(id, at)| {
                        let pos = at(t - self.lag_s, &seen)?;
                        Some(Car { id: *id, pos: [pos[0] + self.jitter_m * noise(), pos[1], pos[2] + self.jitter_m * noise()] })
                    })
                    .collect();
                Frame {
                    t,
                    live: true,
                    own: Own {
                        id: OWN,
                        pos: seen.pos,
                        contact: (self.inputs == Inputs::Contact).then(|| [wheel(b, -w), wheel(b, w), wheel(-b, -w), wheel(-b, w)]),
                        velocity: if self.inputs == Inputs::Positions { [0.0; 3] } else { [now.fwd[0] * now.speed, 0.0, now.fwd[1] * now.speed] },
                        heading: 0.0,
                        speed: now.speed,
                        yaw_rate: now.yaw_rate,
                        throttle: 1.0,
                        brake: 0.0,
                        in_pit_lane: (self.pit_lane)(t),
                        in_pit_box: false,
                        lap_count: 0,
                        lap_valid: true,
                    },
                    others,
                }
            })
            .collect()
    }
}

/// The spotter's events over the frames, with the time of each.
pub fn run_with(spotter: &mut Spotter, frames: &[Frame]) -> Vec<(f64, Event)> {
    let mut out = Vec::new();
    for frame in frames {
        out.extend(spotter.step(frame).into_iter().map(|event| (frame.t, event)));
    }
    out
}

pub fn run(frames: &[Frame]) -> Vec<(f64, Event)> {
    run_with(&mut Spotter::new(Config::default()), frames)
}

/// Only the events, without their times.
pub fn events(timeline: &[(f64, Event)]) -> Vec<Event> {
    timeline.iter().map(|(_, e)| *e).collect()
}

/// A value moving linearly between keyframes (time, value), held before the first and after the last.
pub fn keys(t: f64, keys: &[(f64, f32)]) -> f32 {
    let (first, last) = (keys[0], keys[keys.len() - 1]);
    if t <= first.0 {
        return first.1;
    }
    keys.windows(2).find(|w| t < w[1].0).map_or(last.1, |w| w[0].1 + (w[1].1 - w[0].1) * ((t - w[0].0) / (w[1].0 - w[0].0)) as f32)
}
