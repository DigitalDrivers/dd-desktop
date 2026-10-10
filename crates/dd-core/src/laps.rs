//! Lap summaries from what Assetto Corsa EVO publishes in its shared memory (`Local\acevo_pmf_physics`,
//! `Local\acevo_pmf_graphics` and `Local\acevo_pmf_static`). The shell samples the pages about ten times a second
//! and hands the bytes here; a lap is summed up when the game counts it. The pages name the car only by its
//! display name, so the shell keeps a lap only when the game's log says the club's car (id starting with `dd_`)
//! is selected, see [`current_car`]. Offsets are those of the official header with `#pragma pack(4)` (page sizes 800, 4900 and 208
//! bytes). The values are sent as the game publishes them: their units are not documented for EVO.

use serde::Serialize;

use crate::evo_memory::{f32_at, text_at, GRAPHICS_SIZE, STATIC_SIZE};

pub const PHYSICS_SIZE: usize = 800;

// graphics
const STATUS: usize = 4;
const FUEL: usize = 196;
const TOTAL_LAP_COUNT: usize = 2384;
const LAST_LAPTIME_MS: usize = 2396;
const CAR_MODEL: usize = 3086;
const IS_IN_PIT_LANE: usize = 3120;
const IS_VALID_LAP: usize = 3121;
// physics, wheels in the order FL FR RL RR
const SPEED_KMH: usize = 28;
const WHEELS_PRESSURE: usize = 88;
const TYRE_CORE_TEMPERATURE: usize = 152;
const RIDE_HEIGHT: usize = 268;
const AIR_TEMP: usize = 288;
const ROAD_TEMP: usize = 292;
const BRAKE_TEMP: usize = 348;
const GAS: usize = 4;
const BRAKE: usize = 8;
const GEAR: usize = 16;
const RPMS: usize = 20;
/// G: lateral, longitudinal, vertical
const ACC_G: usize = 44;
const SUSPENSION_TRAVEL: usize = 184;
const BRAKE_BIAS: usize = 564;
const SLIP_RATIO: usize = 640;
const SLIP_ANGLE: usize = 656;
const BRAKE_TORQUE: usize = 716;
// static
const TRACK: usize = 136;
const TRACK_CONFIGURATION: usize = 169;
const IS_ONLINE: usize = 86;
/// `ACEVO_STATUS`: 2 = live driving
const LIVE: i32 = 2;
/// The club's cars; the laps of any other car are not kept.
const CLUB_CAR_PREFIX: &str = "dd_";
/// A lap with fewer samples (ten seconds at 10 Hz) is not summed up.
const MIN_SAMPLES: u32 = 100;
/// Live samples (two seconds at 10 Hz) a counted lap waits for its lap time.
const LAPTIME_WAIT: u32 = 20;
/// A wheel locks under braking, or spins under throttle, beyond this slip ratio (absolute).
const SLIP_LIMIT: f32 = 0.15;
/// Samples (one second at 10 Hz) of the moving averages that keep a single kerb strike out of the extremes.
const WINDOW: usize = 10;

/// Pressure, core and brake temperature of one wheel over a lap.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tyre {
    pub pressure_avg: f32,
    pub pressure_max: f32,
    /// The pressure of the lap's last sample off the pit lane. At 10 Hz it moves by 0.007 psi at most from one
    /// sample to the next (three Nordschleife MoTeC logs of 2026-10-09): one sample is the lap's end, a mean of
    /// the last seconds would only lag behind.
    pub pressure_end: f32,
    pub core_temp_avg: f32,
    pub core_temp_max: f32,
    pub brake_temp_max: f32,
    pub travel_max: f32,
    pub travel_avg: f32,
    /// Share of the braking samples with the wheel's |slip ratio| above 0.15
    pub lock_share: f32,
    /// Share of the throttle samples with the wheel's |slip ratio| above 0.15
    pub spin_share: f32,
}

/// The raw wheel values of a lap's first sample, for the shell's log (not sent).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FirstSample {
    pub pressure: [f32; 4],
    pub core_temp: [f32; 4],
    pub ride_height: [f32; 2],
}

/// One finished lap, as the platform takes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LapSummary {
    pub car: String,
    pub track: String,
    pub layout: String,
    pub online: bool,
    pub lap_time_ms: i32,
    pub valid: bool,
    pub pit: bool,
    /// The sum of the decreases between consecutive samples off the pit lane; refuelling does not count.
    pub fuel_used_l: f32,
    pub air_temp_c: f32,
    pub road_temp_c: f32,
    pub top_speed_kmh: f32,
    /// FL FR RL RR
    pub tyres: [Tyre; 4],
    /// Front, rear; the minimum is the lowest one-second average
    pub ride_height_min: [f32; 2],
    pub ride_height_avg: [f32; 2],
    /// The brake bias setting, averaged over the braking samples
    pub brake_bias: f32,
    /// The front axle's share of the brake torque, averaged over the braking samples with any torque
    pub brake_front: f32,
    /// Front minus rear mean |slip angle| in degrees over the cornering samples: above 0 understeer, below oversteer
    pub balance_deg: f32,
    pub rpm_max: i32,
    /// The gear of the sample with the highest speed (1 = first; the pages count 0 = reverse, 1 = neutral)
    pub gear_at_top_speed: i32,
    /// Highest one-second average of |lateral G|, and of |longitudinal G| over ten braking samples in a row
    pub lat_g_max: f32,
    pub brake_g_max: f32,
    /// The car's mechanical preset (stage or variant) as the game's log names it; the shell fills it in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// The version of the car's package the app installed, if the package is still that one; the shell fills it in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_version: Option<u32>,
    /// The setup the driver loaded for the car as the game's log names it, see [`current_setup`]; the shell fills it in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_name: Option<String>,
    #[serde(skip)]
    pub first: FirstSample,
}

/// What the game's log writes when the driver selects a car, followed by the car's id and its preset's path
/// (`content\cars\<car>\presets\<preset>.mechanicalcarpreset`).
const SET_CAR: &str = "onSetPlayerCurrentCarCommand: Set new car ";

/// The car the driver selected last according to the game's log: its id (the folder below `cars`) and its
/// preset's file name without the extension.
pub fn current_car(log: &str) -> Option<(String, String)> {
    log.lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once(SET_CAR)?;
            let (_, path) = rest.trim().split_once(' ')?;
            let parts: Vec<&str> = path.trim().split(['\\', '/']).collect();
            match parts.as_slice() {
                [.., "cars", id, "presets", file] => Some((id.to_string(), file.rsplit_once('.').map_or(*file, |(stem, _)| stem).to_string())),
                _ => None,
            }
        })
        .last()
}

/// What the game's log writes when the driver loads a setup in the setup screen, followed by the setup's name.
const LOAD_SETUP: &str = "] Load preset ";
/// The platform refuses a longer setup name (in UTF-16 units) and with it the whole batch of laps.
const SETUP_NAME_MAX: usize = 128;

/// The setup the driver loaded last according to the game's log, if that was after the car was last selected:
/// the game logs the car again when a session starts, and nothing says a setup loaded before still applies.
/// None when the driver drives the game's default setup or one the game picked by itself, or the name is too long.
pub fn current_setup(log: &str) -> Option<String> {
    let mut setup = None;
    for line in log.lines() {
        if line.contains(SET_CAR) {
            setup = None;
        } else if let Some((_, name)) = line.split_once(LOAD_SETUP) {
            setup = Some(name.trim()).filter(|name| !name.is_empty() && name.encode_utf16().count() <= SETUP_NAME_MAX);
        }
    }
    setup.map(str::to_string)
}

/// Whether a car id is one of the club's cars.
pub fn is_club_car(id: &str) -> bool {
    id.starts_with(CLUB_CAR_PREFIX)
}

/// The fields of one sample of the three pages.
struct Sample {
    car: String,
    track: String,
    layout: String,
    online: bool,
    lap_count: i32,
    last_laptime_ms: i32,
    fuel: f32,
    pit: bool,
    valid: bool,
    speed: f32,
    pressure: [f32; 4],
    core_temp: [f32; 4],
    brake_temp: [f32; 4],
    ride_height: [f32; 2],
    air_temp: f32,
    road_temp: f32,
    gas: f32,
    brake: f32,
    gear: i32,
    rpm: i32,
    acc_g: [f32; 3],
    travel: [f32; 4],
    brake_bias: f32,
    slip_ratio: [f32; 4],
    slip_angle: [f32; 4],
    brake_torque: [f32; 4],
}

impl Sample {
    /// Braking: brake above 0.1 at more than 30 km/h.
    fn braking(&self) -> bool {
        self.brake > 0.1 && self.speed > 30.0
    }

    /// Throttle: gas above 0.5, brake below 0.05, at more than 30 km/h.
    fn throttle(&self) -> bool {
        self.gas > 0.5 && self.speed > 30.0 && self.brake < 0.05
    }

    /// Cornering: |lateral G| above 0.8 at more than 60 km/h.
    fn cornering(&self) -> bool {
        self.speed > 60.0 && self.acc_g[0].abs() > 0.8
    }
}

fn i32_at(page: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(page[at..at + 4].try_into().unwrap())
}

fn f32s_at<const N: usize>(page: &[u8], at: usize) -> [f32; N] {
    std::array::from_fn(|i| f32_at(page, at + i * 4))
}

impl Sample {
    /// None unless the game drives live.
    fn read(graphics: &[u8], physics: &[u8], statics: &[u8]) -> Option<Sample> {
        if graphics.len() < GRAPHICS_SIZE || physics.len() < PHYSICS_SIZE || statics.len() < STATIC_SIZE {
            return None;
        }
        if i32_at(graphics, STATUS) != LIVE {
            return None;
        }
        Some(Sample {
            car: text_at(graphics, CAR_MODEL, 33),
            track: text_at(statics, TRACK, 33),
            layout: text_at(statics, TRACK_CONFIGURATION, 33),
            online: statics[IS_ONLINE] != 0,
            lap_count: i32_at(graphics, TOTAL_LAP_COUNT),
            last_laptime_ms: i32_at(graphics, LAST_LAPTIME_MS),
            fuel: f32_at(graphics, FUEL),
            pit: graphics[IS_IN_PIT_LANE] != 0,
            valid: graphics[IS_VALID_LAP] != 0,
            speed: f32_at(physics, SPEED_KMH),
            pressure: f32s_at(physics, WHEELS_PRESSURE),
            core_temp: f32s_at(physics, TYRE_CORE_TEMPERATURE),
            brake_temp: f32s_at(physics, BRAKE_TEMP),
            ride_height: f32s_at(physics, RIDE_HEIGHT),
            air_temp: f32_at(physics, AIR_TEMP),
            road_temp: f32_at(physics, ROAD_TEMP),
            gas: f32_at(physics, GAS),
            brake: f32_at(physics, BRAKE),
            gear: i32_at(physics, GEAR),
            rpm: i32_at(physics, RPMS),
            acc_g: f32s_at(physics, ACC_G),
            travel: f32s_at(physics, SUSPENSION_TRAVEL),
            brake_bias: f32_at(physics, BRAKE_BIAS),
            slip_ratio: f32s_at(physics, SLIP_RATIO),
            slip_angle: f32s_at(physics, SLIP_ANGLE),
            brake_torque: f32s_at(physics, BRAKE_TORQUE),
        })
    }
}

/// The average of the last `WINDOW` values, kept in a ring with a running sum.
#[derive(Default)]
struct Window {
    values: [f32; WINDOW],
    len: usize,
    next: usize,
    sum: f64,
}

impl Window {
    /// Adds a value; the average once the window is full.
    fn push(&mut self, value: f32) -> Option<f32> {
        if self.len == WINDOW {
            self.sum -= self.values[self.next] as f64;
        } else {
            self.len += 1;
        }
        self.values[self.next] = value;
        self.sum += value as f64;
        self.next = (self.next + 1) % WINDOW;
        (self.len == WINDOW).then(|| (self.sum / WINDOW as f64) as f32)
    }
}

/// The running sums of the lap being driven, over its samples off the pit lane.
struct Lap {
    samples: u32,
    first: FirstSample,
    online: bool,
    fuel_last: Option<f32>,
    fuel_used: f64,
    valid: bool,
    pit: bool,
    top_speed: f32,
    pressure_sum: [f64; 4],
    pressure_max: [f32; 4],
    pressure_end: [f32; 4],
    core_sum: [f64; 4],
    core_max: [f32; 4],
    brake_max: [f32; 4],
    ride_min: [f32; 2],
    ride_sum: [f64; 2],
    air_sum: f64,
    road_sum: f64,
    gear_at_top: i32,
    rpm_max: i32,
    lat_g: Window,
    lat_g_max: f32,
    ride: [Window; 2],
    travel_sum: [f64; 4],
    travel_max: [f32; 4],
    braking: u32,
    locked: [u32; 4],
    bias_sum: f64,
    /// Over the braking samples in a row; emptied when braking stops
    brake_g: Window,
    brake_g_max: f32,
    /// Braking samples with any brake torque, and the sum of their front shares
    torque_samples: u32,
    front_sum: f64,
    throttle: u32,
    spinning: [u32; 4],
    cornering: u32,
    balance_sum: f64,
}

/// `sum / count`: a share or an average over some of the samples; 0 when there are none.
fn per(sum: f64, count: u32) -> f32 {
    if count == 0 { 0.0 } else { (sum / count as f64) as f32 }
}

impl Lap {
    /// A lap starting at this sample; `pit` when it is already known to include the pit lane.
    fn start(s: &Sample, pit: bool) -> Lap {
        let mut lap = Lap {
            samples: 0,
            first: FirstSample::default(),
            online: s.online,
            fuel_last: None,
            fuel_used: 0.0,
            valid: s.valid,
            pit,
            top_speed: f32::NEG_INFINITY,
            pressure_sum: [0.0; 4],
            pressure_max: [f32::NEG_INFINITY; 4],
            pressure_end: [0.0; 4],
            core_sum: [0.0; 4],
            core_max: [f32::NEG_INFINITY; 4],
            brake_max: [f32::NEG_INFINITY; 4],
            ride_min: [f32::INFINITY; 2],
            ride_sum: [0.0; 2],
            air_sum: 0.0,
            road_sum: 0.0,
            gear_at_top: 0,
            rpm_max: i32::MIN,
            lat_g: Window::default(),
            lat_g_max: 0.0,
            ride: Default::default(),
            travel_sum: [0.0; 4],
            travel_max: [f32::NEG_INFINITY; 4],
            braking: 0,
            locked: [0; 4],
            bias_sum: 0.0,
            brake_g: Window::default(),
            brake_g_max: 0.0,
            torque_samples: 0,
            front_sum: 0.0,
            throttle: 0,
            spinning: [0; 4],
            cornering: 0,
            balance_sum: 0.0,
        };
        lap.take(s);
        lap
    }

    /// A sample in the pit lane only marks the lap (and breaks the moving averages); any other is summed up.
    fn take(&mut self, s: &Sample) {
        self.valid = s.valid;
        if s.pit {
            self.pit = true;
            self.lat_g = Window::default();
            self.ride = Default::default();
            self.brake_g = Window::default();
            self.fuel_last = None;
        } else {
            self.add(s);
        }
    }

    fn add(&mut self, s: &Sample) {
        if self.samples == 0 {
            self.first = FirstSample { pressure: s.pressure, core_temp: s.core_temp, ride_height: s.ride_height };
        }
        self.samples += 1;
        if let Some(last) = self.fuel_last {
            self.fuel_used += (last - s.fuel).max(0.0) as f64;
        }
        self.fuel_last = Some(s.fuel);
        if s.speed > self.top_speed {
            self.top_speed = s.speed;
            self.gear_at_top = s.gear;
        }
        self.rpm_max = self.rpm_max.max(s.rpm);
        if let Some(average) = self.lat_g.push(s.acc_g[0].abs()) {
            self.lat_g_max = self.lat_g_max.max(average);
        }
        let (braking, throttle) = (s.braking(), s.throttle());
        if braking {
            self.braking += 1;
            self.bias_sum += s.brake_bias as f64;
            if let Some(average) = self.brake_g.push(s.acc_g[1].abs()) {
                self.brake_g_max = self.brake_g_max.max(average);
            }
            let torque: f32 = s.brake_torque.iter().sum();
            if torque > 0.0 {
                self.torque_samples += 1;
                self.front_sum += ((s.brake_torque[0] + s.brake_torque[1]) / torque) as f64;
            }
        } else {
            self.brake_g = Window::default();
        }
        if throttle {
            self.throttle += 1;
        }
        if s.cornering() {
            self.cornering += 1;
            let mean = |a: f32, b: f32| (a.abs() + b.abs()) / 2.0;
            let front_minus_rear = mean(s.slip_angle[0], s.slip_angle[1]) - mean(s.slip_angle[2], s.slip_angle[3]);
            self.balance_sum += front_minus_rear.to_degrees() as f64;
        }
        self.pressure_end = s.pressure;
        for w in 0..4 {
            self.pressure_sum[w] += s.pressure[w] as f64;
            self.pressure_max[w] = self.pressure_max[w].max(s.pressure[w]);
            self.core_sum[w] += s.core_temp[w] as f64;
            self.core_max[w] = self.core_max[w].max(s.core_temp[w]);
            self.brake_max[w] = self.brake_max[w].max(s.brake_temp[w]);
            self.travel_sum[w] += s.travel[w] as f64;
            self.travel_max[w] = self.travel_max[w].max(s.travel[w]);
            let slipping = s.slip_ratio[w].abs() > SLIP_LIMIT;
            self.locked[w] += (braking && slipping) as u32;
            self.spinning[w] += (throttle && slipping) as u32;
        }
        for axle in 0..2 {
            if let Some(average) = self.ride[axle].push(s.ride_height[axle]) {
                self.ride_min[axle] = self.ride_min[axle].min(average);
            }
            self.ride_sum[axle] += s.ride_height[axle] as f64;
        }
        self.air_sum += s.air_temp as f64;
        self.road_sum += s.road_temp as f64;
    }

    fn summary(self, car: &str, track: &str, layout: &str, lap_time_ms: i32) -> LapSummary {
        let n = self.samples as f64;
        let avg = |sum: f64| (sum / n) as f32;
        LapSummary {
            car: car.to_string(),
            track: track.to_string(),
            layout: layout.to_string(),
            online: self.online,
            lap_time_ms,
            valid: self.valid,
            pit: self.pit,
            fuel_used_l: self.fuel_used as f32,
            air_temp_c: avg(self.air_sum),
            road_temp_c: avg(self.road_sum),
            top_speed_kmh: self.top_speed,
            tyres: std::array::from_fn(|w| Tyre {
                pressure_avg: avg(self.pressure_sum[w]),
                pressure_max: self.pressure_max[w],
                pressure_end: self.pressure_end[w],
                core_temp_avg: avg(self.core_sum[w]),
                core_temp_max: self.core_max[w],
                brake_temp_max: self.brake_max[w],
                travel_max: self.travel_max[w],
                travel_avg: avg(self.travel_sum[w]),
                lock_share: per(self.locked[w] as f64, self.braking),
                spin_share: per(self.spinning[w] as f64, self.throttle),
            }),
            ride_height_min: self.ride_min,
            ride_height_avg: [avg(self.ride_sum[0]), avg(self.ride_sum[1])],
            brake_bias: per(self.bias_sum, self.braking),
            brake_front: per(self.front_sum, self.torque_samples),
            balance_deg: per(self.balance_sum, self.cornering),
            rpm_max: self.rpm_max,
            gear_at_top_speed: self.gear_at_top - 1,
            lat_g_max: self.lat_g_max,
            brake_g_max: self.brake_g_max,
            preset: None,
            package_version: None,
            setup_name: None,
            first: self.first,
        }
    }
}

/// A lap the game has counted, waiting for its lap time: the game may publish it a few samples after the count.
struct Pending {
    lap: Lap,
    /// Whether it is complete enough to keep (it started at a boundary and has enough samples)
    keep: bool,
    /// Live samples since the boundary
    waited: u32,
}

/// The car and track being driven, and the lap since the game last counted one.
struct Stint {
    car: String,
    track: String,
    layout: String,
    lap_count: i32,
    /// Whether the lap being driven started at a counted boundary or at the pit exit (not one joined midway)
    from_boundary: bool,
    /// Whether the last sample was in the pit lane, and whether the game counted a lap since the car entered it
    in_pit: bool,
    counted_in_pit: bool,
    lap: Lap,
    pending: Option<Pending>,
    /// `last_laptime_ms` as it was when the previous lap was settled (or the stint started): a different value
    /// is the new lap's time.
    known_laptime: i32,
}

impl Stint {
    fn start(s: &Sample) -> Stint {
        Stint {
            car: s.car.clone(),
            track: s.track.clone(),
            layout: s.layout.clone(),
            lap_count: s.lap_count,
            from_boundary: false,
            in_pit: s.pit,
            counted_in_pit: false,
            lap: Lap::start(s, false),
            pending: None,
            known_laptime: s.last_laptime_ms,
        }
    }

    /// Settles the pending lap once its time has arrived, after `LAPTIME_WAIT` samples, or when `now`: with the
    /// lap time of this sample, dropped if there is none.
    fn settle(&mut self, s: &Sample, now: bool) -> Option<LapSummary> {
        let pending = self.pending.as_ref()?;
        let arrived = s.last_laptime_ms > 0 && s.last_laptime_ms != self.known_laptime;
        if !(arrived || now || pending.waited >= LAPTIME_WAIT) {
            return None;
        }
        let pending = self.pending.take().unwrap();
        self.known_laptime = s.last_laptime_ms;
        (pending.keep && s.last_laptime_ms > 0).then(|| pending.lap.summary(&self.car, &self.track, &self.layout, s.last_laptime_ms))
    }
}

/// Turns samples of the pages into finished laps, of any car (by its display name). Samples outside live driving
/// are ignored; a change of car or track, or a lap counter that jumps or goes back (a new session), starts over
/// and forgets a lap still waiting for its time. The lap the app joined midway is dropped.
///
/// Samples in the pit lane are not summed up; they only mark the lap `pit` (an in-lap, when the game counts the
/// lap after the car entered the pit lane). Leaving the pit lane starts a fresh lap that is kept when the game
/// counts it, as on Touristenfahrten, where every stint starts in the box and the counter ticks at the end split
/// before the pit lane. That lap is marked `pit` too if the game counted a lap while the car was in the pit lane
/// (its time then includes the pit lane).
///
/// A lap ends at the sample where the game counts it; `valid` and `pit` are those of that moment. Its lap time
/// is the first `last_laptime_ms` that is positive and differs from the one the previous lap was settled with,
/// so a lap whose time arrives in the same sample as the count is returned by that very sample, and one whose
/// time arrives later by the sample that brings it. After `LAPTIME_WAIT` samples without a new time, the value
/// then published is taken (the same time to the millisecond twice in a row); none at all drops the lap.
#[derive(Default)]
pub struct LapRecorder {
    stint: Option<Stint>,
}

impl LapRecorder {
    pub fn new() -> LapRecorder {
        LapRecorder::default()
    }

    /// Takes one sample; returns a lap the game has counted, once it is settled and complete enough to keep.
    pub fn sample(&mut self, graphics: &[u8], physics: &[u8], statics: &[u8]) -> Option<LapSummary> {
        let s = Sample::read(graphics, physics, statics)?;
        let stint = match &mut self.stint {
            Some(stint) if stint.car == s.car && stint.track == s.track && stint.layout == s.layout => stint,
            _ => {
                self.stint = Some(Stint::start(&s));
                return None;
            }
        };
        if s.pit && !stint.in_pit {
            stint.counted_in_pit = false;
        }
        let pit_exit = stint.in_pit && !s.pit;
        stint.in_pit = s.pit;
        match s.lap_count - stint.lap_count {
            0 => {
                if pit_exit {
                    stint.lap = Lap::start(&s, stint.counted_in_pit);
                    stint.from_boundary = true;
                } else {
                    stint.lap.take(&s);
                }
                if let Some(pending) = &mut stint.pending {
                    pending.waited += 1;
                }
                stint.settle(&s, false)
            }
            1 => {
                // A lap still waiting means the new one is shorter than the wait and will not be kept.
                let earlier = stint.settle(&s, true);
                let lap = std::mem::replace(&mut stint.lap, Lap::start(&s, false));
                let keep = stint.from_boundary && lap.samples >= MIN_SAMPLES;
                stint.pending = Some(Pending { lap, keep, waited: 0 });
                stint.from_boundary = true;
                stint.counted_in_pit |= s.pit;
                stint.lap_count = s.lap_count;
                earlier.or_else(|| stint.settle(&s, false))
            }
            _ => {
                self.stint = Some(Stint::start(&s));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pages {
        graphics: Vec<u8>,
        physics: Vec<u8>,
        statics: Vec<u8>,
    }

    impl Pages {
        fn new() -> Pages {
            let mut pages = Pages { graphics: vec![0u8; GRAPHICS_SIZE], physics: vec![0u8; PHYSICS_SIZE], statics: vec![0u8; STATIC_SIZE] };
            pages.graphics[STATUS..STATUS + 4].copy_from_slice(&LIVE.to_le_bytes());
            pages.car("BMW M3 E46 GT3");
            pages.statics[TRACK..TRACK + 11].copy_from_slice(b"Nurburgring");
            pages.statics[TRACK_CONFIGURATION..TRACK_CONFIGURATION + 2].copy_from_slice(b"GP");
            pages.statics[IS_ONLINE] = 1;
            pages.graphics[IS_VALID_LAP] = 1;
            pages.f32s(RIDE_HEIGHT, &[0.07, 0.08]);
            pages
        }

        fn car(&mut self, id: &str) {
            self.graphics[CAR_MODEL..CAR_MODEL + 33].fill(0);
            self.graphics[CAR_MODEL..CAR_MODEL + id.len()].copy_from_slice(id.as_bytes());
        }

        fn i32(&mut self, at: usize, value: i32) {
            self.graphics[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }

        fn physics_i32(&mut self, at: usize, value: i32) {
            self.physics[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }

        fn fuel(&mut self, liters: f32) {
            self.graphics[FUEL..FUEL + 4].copy_from_slice(&liters.to_le_bytes());
        }

        fn f32s(&mut self, at: usize, values: &[f32]) {
            for (i, v) in values.iter().enumerate() {
                self.physics[at + i * 4..at + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        }

        /// The game counts a lap with this time.
        fn count_lap(&mut self, lap_time_ms: i32) {
            self.count_only();
            self.i32(LAST_LAPTIME_MS, lap_time_ms);
        }

        /// The game counts a lap but has not published its time yet.
        fn count_only(&mut self) {
            let count = i32_at(&self.graphics, TOTAL_LAP_COUNT);
            self.i32(TOTAL_LAP_COUNT, count + 1);
        }

        fn sample(&self, recorder: &mut LapRecorder) -> Option<LapSummary> {
            recorder.sample(&self.graphics, &self.physics, &self.statics)
        }

        /// `n` samples; the laps they finish.
        fn drive(&self, recorder: &mut LapRecorder, n: usize) -> Vec<LapSummary> {
            (0..n).filter_map(|_| self.sample(recorder)).collect()
        }
    }

    /// A recorder that has seen the out lap end, so the next lap counts.
    fn past_the_out_lap(pages: &mut Pages) -> LapRecorder {
        let mut recorder = LapRecorder::new();
        assert!(pages.drive(&mut recorder, 30).is_empty());
        pages.count_lap(600_000);
        assert!(pages.sample(&mut recorder).is_none(), "the out lap is dropped");
        recorder
    }

    #[test]
    fn sums_up_a_lap_when_the_game_counts_it() {
        let mut pages = Pages::new();
        pages.fuel(40.0);
        pages.f32s(SPEED_KMH, &[120.0]);
        pages.f32s(WHEELS_PRESSURE, &[26.0, 26.5, 25.0, 25.5]);
        pages.f32s(TYRE_CORE_TEMPERATURE, &[80.0, 81.0, 70.0, 71.0]);
        pages.f32s(BRAKE_TEMP, &[300.0, 310.0, 200.0, 210.0]);
        pages.f32s(AIR_TEMP, &[22.0, 30.0]);
        pages.f32s(SUSPENSION_TRAVEL, &[0.03, 0.03, 0.04, 0.04]);
        pages.physics_i32(GEAR, 4);
        pages.physics_i32(RPMS, 6000);
        // the lap's first sample is the one at the boundary; 98 more, then one with other values: 100
        let mut recorder = past_the_out_lap(&mut pages);
        assert!(pages.drive(&mut recorder, 98).is_empty());
        pages.fuel(36.5);
        pages.f32s(SPEED_KMH, &[251.5]);
        pages.f32s(WHEELS_PRESSURE, &[28.0, 28.5, 27.0, 27.5]);
        pages.f32s(TYRE_CORE_TEMPERATURE, &[90.0, 91.0, 80.0, 81.0]);
        pages.f32s(BRAKE_TEMP, &[600.0, 610.0, 400.0, 410.0]);
        pages.f32s(RIDE_HEIGHT, &[0.05, 0.06]);
        pages.f32s(SUSPENSION_TRAVEL, &[0.05, 0.03, 0.04, 0.04]);
        pages.physics_i32(GEAR, 7); // 6th: the pages count 0 = reverse, 1 = neutral
        pages.physics_i32(RPMS, 7800);
        assert!(pages.drive(&mut recorder, 1).is_empty());
        pages.count_lap(512_345);
        let lap = pages.sample(&mut recorder).expect("the lap");

        assert_eq!((lap.car.as_str(), lap.track.as_str(), lap.layout.as_str(), lap.online), ("BMW M3 E46 GT3", "Nurburgring", "GP", true));
        assert_eq!((lap.lap_time_ms, lap.valid, lap.pit), (512_345, true, false));
        assert_eq!(lap.fuel_used_l, 3.5);
        assert_eq!(lap.top_speed_kmh, 251.5);
        assert_eq!((lap.air_temp_c, lap.road_temp_c), (22.0, 30.0));
        let fl = &lap.tyres[0];
        assert_eq!((fl.pressure_avg, fl.pressure_max, fl.core_temp_avg, fl.core_temp_max, fl.brake_temp_max), (26.02, 28.0, 80.1, 90.0, 600.0));
        assert_eq!((fl.travel_max, lap.tyres[1].travel_max), (0.05, 0.03));
        assert!((fl.travel_avg - 0.0302).abs() < 1e-6);
        assert_eq!((lap.rpm_max, lap.gear_at_top_speed), (7800, 6));
        // no braking, throttle or cornering samples in this lap: every share and average over them is 0
        assert!(lap.tyres.iter().all(|t| t.lock_share == 0.0 && t.spin_share == 0.0));
        assert_eq!((lap.brake_bias, lap.brake_front, lap.balance_deg), (0.0, 0.0, 0.0));
        assert_eq!((lap.lat_g_max, lap.brake_g_max), (0.0, 0.0));
        assert_eq!(lap.tyres[3].brake_temp_max, 410.0);
        // the pressures of the lap's last sample
        assert_eq!(lap.tyres.each_ref().map(|t| t.pressure_end), [28.0, 28.5, 27.0, 27.5]);
        // one second averages: the last one has nine samples at 0.07 / 0.08 and one at 0.05 / 0.06
        assert!(close(lap.ride_height_min[0], 0.068) && close(lap.ride_height_min[1], 0.078), "{:?}", lap.ride_height_min);
        assert!((lap.ride_height_avg[0] - 0.0698).abs() < 1e-6);
        assert_eq!(lap.first, FirstSample { pressure: [26.0, 26.5, 25.0, 25.5], core_temp: [80.0, 81.0, 70.0, 71.0], ride_height: [0.07, 0.08] });

        let json = serde_json::to_value(&lap).unwrap();
        assert_eq!(json["lapTimeMs"], 512_345);
        assert_eq!(json["fuelUsedL"], 3.5);
        assert_eq!(json["tyres"].as_array().unwrap().len(), 4);
        assert_eq!(json["tyres"][0]["brakeTempMax"], 600.0);
        let mut tyre_keys: Vec<_> = json["tyres"][0].as_object().unwrap().keys().cloned().collect();
        tyre_keys.sort();
        // tempInner/Middle/Outer are gone: the graphics page's tread temperatures spread by 1 °C at most
        assert_eq!(tyre_keys, ["brakeTempMax", "coreTempAvg", "coreTempMax", "lockShare", "pressureAvg", "pressureEnd", "pressureMax", "spinShare", "travelAvg", "travelMax"]);
        assert_eq!(json["gearAtTopSpeed"], 6);
        assert_eq!(json["rideHeightMin"].as_array().unwrap().len(), 2);
        assert!(json.get("first").is_none());
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        // absShare and tcShare are gone: the game leaves those fields at 0
        assert_eq!(keys, [
            "airTempC", "balanceDeg", "brakeBias", "brakeFront", "brakeGMax", "car", "fuelUsedL", "gearAtTopSpeed", "lapTimeMs", "latGMax", "layout",
            "online", "pit", "rideHeightAvg", "rideHeightMin", "roadTempC", "rpmMax", "topSpeedKmh", "track", "tyres", "valid",
        ]);
    }

    const SELECT_RENNSPORT_UNL3: &str = "[2026-10-04 20:05:21.996] [gameplay] [info] ACEVO-2629 onSetPlayerCurrentCarCommand: Set new car 4422f4ed-904e-8717-71fa-e328d8e09daa content\\cars\\dd_bmw_m3_e46_rennsport\\presets\\preset_dd_bmw_m3_e46_rennsport_unl3.mechanicalcarpreset";

    #[test]
    fn finds_the_selected_car_and_its_preset_in_the_game_s_log() {
        let log = format!("[2026-10-04 20:05:20.001] [core] [info] something else\n{SELECT_RENNSPORT_UNL3}\n[2026-10-04 20:05:22.000] [gameplay] [info] driving\n");
        let expected = Some(("dd_bmw_m3_e46_rennsport".to_string(), "preset_dd_bmw_m3_e46_rennsport_unl3".to_string()));
        assert_eq!(current_car(&log), expected);
        assert_eq!(current_car(&log.replace('\n', "\r\n")), expected, "CRLF");
        assert_eq!(current_car("[2026-10-04 20:05:20.001] [core] [info] started\n"), None);
    }

    #[test]
    fn takes_the_last_selection_of_any_car() {
        let unl1 = SELECT_RENNSPORT_UNL3.replace("unl3", "unl1");
        let kunos = SELECT_RENNSPORT_UNL3.replace("dd_bmw_m3_e46_rennsport", "bmw_m2_coupe").replace("_unl3", "_stock");
        let log = [SELECT_RENNSPORT_UNL3, &unl1].join("\n");
        assert_eq!(current_car(&log).unwrap().1, "preset_dd_bmw_m3_e46_rennsport_unl1");
        let log = [SELECT_RENNSPORT_UNL3, &kunos].join("\n");
        assert_eq!(current_car(&log), Some(("bmw_m2_coupe".to_string(), "preset_bmw_m2_coupe_stock".to_string())));
        let log = [&kunos, SELECT_RENNSPORT_UNL3].join("\n");
        assert_eq!(current_car(&log).unwrap().0, "dd_bmw_m3_e46_rennsport");
    }

    const LOAD_GT3_QUALIFYING: &str = "[2026-10-08 23:42:34.726] [gameface] [info] Load preset DD Nordschleife GT3 Nordschleife Qualifying v4 ";

    #[test]
    fn finds_the_setup_loaded_for_the_selected_car_in_the_game_s_log() {
        // As on 2026-10-08: the car is selected (twice, the second time as the session starts), then the setup is loaded.
        let gt3 = SELECT_RENNSPORT_UNL3.replace("rennsport", "gt3");
        let dynamic = "[2026-10-08 23:41:42.515] [physics] [info] Loading DynamicTrack preset: content\\tracks\\nurburgring/dynamic_track/Touristenfahrten.dynamictrackpresetcompressed";
        let log = [gt3.as_str(), &gt3, dynamic, "[2026-10-08 23:42:34.726] [gameface] [warning] CarSetupRequestPresetList ", LOAD_GT3_QUALIFYING, "[2026-10-08 23:42:34.726] [gameface] [warning] CarSetupRequestLoadPreset "].join("\r\n");
        assert_eq!(current_setup(&log).as_deref(), Some("DD Nordschleife GT3 Nordschleife Qualifying v4"));
        // the last one loaded counts
        let stage3 = LOAD_GT3_QUALIFYING.replace("GT3 Nordschleife Qualifying v4", "S54 Stage 3 Hotlap v1");
        assert_eq!(current_setup(&[log.as_str(), &stage3].join("\n")).as_deref(), Some("DD Nordschleife S54 Stage 3 Hotlap v1"));
        // a setup loaded before the car was selected (again) is not known to be on it
        assert_eq!(current_setup(&[LOAD_GT3_QUALIFYING, &gt3].join("\n")), None);
        assert_eq!(current_setup(&[log.as_str(), SELECT_RENNSPORT_UNL3].join("\n")), None);
        // none loaded, or an empty name
        assert_eq!(current_setup(&[gt3.as_str(), dynamic].join("\n")), None);
        assert_eq!(current_setup(&[gt3.as_str(), "[2026-10-08 23:42:34.726] [gameface] [info] Load preset  "].join("\n")), None);
        // a name the platform takes, and one too long for it
        let load = |name: &str| [gt3.as_str(), &format!("[2026-10-08 23:42:34.726] [gameface] [info] Load preset {name}")].join("\n");
        assert_eq!(current_setup(&load(&"ä".repeat(128))), Some("ä".repeat(128)));
        assert_eq!(current_setup(&load(&"ä".repeat(129))), None);
    }

    #[test]
    fn sends_the_package_version_and_the_setup_only_when_known() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 120);
        pages.count_lap(512_000);
        let mut lap = pages.sample(&mut recorder).unwrap();
        let json = serde_json::to_value(&lap).unwrap();
        assert!(json.get("packageVersion").is_none() && json.get("setupName").is_none());
        lap.package_version = Some(47);
        lap.setup_name = Some("DD Nordschleife GT3 Nordschleife Qualifying v4".into());
        let json = serde_json::to_value(&lap).unwrap();
        assert_eq!((json["packageVersion"].as_u64(), json["setupName"].as_str()), (Some(47), Some("DD Nordschleife GT3 Nordschleife Qualifying v4")));
    }

    #[test]
    fn knows_the_club_s_cars_by_their_id() {
        assert!(is_club_car("dd_bmw_m3_e46_gt3"));
        assert!(!is_club_car("bmw_m2_coupe"));
        assert!(!is_club_car("BMW M2 Coupe"));
    }

    #[test]
    fn sends_the_preset_only_when_there_is_one() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 120);
        pages.count_lap(512_000);
        let mut lap = pages.sample(&mut recorder).unwrap();
        assert!(serde_json::to_value(&lap).unwrap().get("preset").is_none());
        lap.preset = Some("preset_dd_bmw_m3_e46_gt3_unl3".into());
        assert_eq!(serde_json::to_value(&lap).unwrap()["preset"], "preset_dd_bmw_m3_e46_gt3_unl3");
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn sums_up_braking_throttle_and_cornering() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages); // the boundary sample stands still: none of the three

        // braking at 150 km/h: half with all four torques (80 % front) and the front left locking at -1.5 G,
        // half without torque at -1.2 G
        pages.f32s(SPEED_KMH, &[150.0]);
        pages.f32s(BRAKE, &[0.8]);
        pages.f32s(BRAKE_BIAS, &[0.56]);
        pages.f32s(BRAKE_TORQUE, &[1000.0, 1000.0, 250.0, 250.0]);
        pages.f32s(SLIP_RATIO, &[-0.2, 0.1, 0.0, 0.0]);
        pages.f32s(ACC_G, &[0.0, -1.5, 1.0]);
        pages.drive(&mut recorder, 20);
        pages.f32s(BRAKE_TORQUE, &[0.0; 4]);
        pages.f32s(SLIP_RATIO, &[0.0; 4]);
        pages.f32s(ACC_G, &[0.0, -1.2, 1.0]);
        pages.drive(&mut recorder, 20);

        // full throttle in 5th: half with the rear wheels spinning; the top speed in 6th
        pages.f32s(BRAKE, &[0.0]);
        pages.f32s(GAS, &[1.0]);
        pages.f32s(SPEED_KMH, &[200.0]);
        pages.f32s(ACC_G, &[0.0, 0.5, 1.0]);
        pages.physics_i32(GEAR, 6);
        pages.physics_i32(RPMS, 7000);
        pages.f32s(SLIP_RATIO, &[0.0, 0.0, 0.3, -0.3]);
        pages.drive(&mut recorder, 15);
        pages.f32s(SLIP_RATIO, &[0.0; 4]);
        pages.drive(&mut recorder, 14);
        pages.f32s(SPEED_KMH, &[260.0]);
        pages.physics_i32(GEAR, 7);
        pages.physics_i32(RPMS, 8200);
        pages.drive(&mut recorder, 1);

        // cornering at 120 km/h, part throttle: half pushing the front (0.1 vs 0.05 rad), half the rear (0.02 vs 0.08)
        pages.f32s(GAS, &[0.3]);
        pages.f32s(SPEED_KMH, &[120.0]);
        pages.physics_i32(GEAR, 4);
        pages.physics_i32(RPMS, 6000);
        pages.f32s(ACC_G, &[1.2, 0.0, 1.0]);
        pages.f32s(SLIP_ANGLE, &[0.1, -0.1, 0.05, -0.05]);
        pages.drive(&mut recorder, 10);
        pages.f32s(ACC_G, &[-1.6, 0.0, 1.0]);
        pages.f32s(SLIP_ANGLE, &[0.02, -0.02, 0.08, -0.08]);
        pages.drive(&mut recorder, 10);

        // slow and straight (none of the three) up to 111 samples
        pages.f32s(SPEED_KMH, &[20.0]);
        pages.f32s(ACC_G, &[0.0, 0.0, 1.0]);
        pages.drive(&mut recorder, 20);
        pages.count_lap(520_000);
        let lap = pages.sample(&mut recorder).expect("the lap");

        assert!(close(lap.brake_bias, 0.56));
        assert!(close(lap.brake_front, 0.8), "only the braking samples with torque");
        assert_eq!((lap.tyres[0].lock_share, lap.tyres[1].lock_share, lap.tyres[2].lock_share), (0.5, 0.0, 0.0));
        assert_eq!(lap.brake_g_max, 1.5, "ten braking samples in a row at 1.5");
        assert_eq!((lap.tyres[0].spin_share, lap.tyres[2].spin_share, lap.tyres[3].spin_share), (0.0, 0.5, 0.5));
        assert_eq!((lap.top_speed_kmh, lap.gear_at_top_speed, lap.rpm_max), (260.0, 6, 8200));
        assert_eq!(lap.lat_g_max, 1.6, "ten samples in a row at 1.6");
        // (+0.05 rad + -0.06 rad) / 2 = -0.005 rad: slightly oversteering
        assert!(close(lap.balance_deg, -0.005f32.to_degrees()), "{}", lap.balance_deg);
    }

    #[test]
    fn balance_is_positive_when_the_front_slides_more() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.f32s(SPEED_KMH, &[100.0]);
        pages.f32s(ACC_G, &[-0.9, 0.0, 1.0]);
        pages.f32s(SLIP_ANGLE, &[-0.1, -0.1, -0.04, -0.04]);
        pages.drive(&mut recorder, 50);
        pages.f32s(SPEED_KMH, &[59.0]); // too slow to count as cornering
        pages.f32s(SLIP_ANGLE, &[0.0, 0.0, 0.5, 0.5]);
        pages.drive(&mut recorder, 60);
        pages.count_lap(520_000);
        let lap = pages.sample(&mut recorder).unwrap();
        assert!(close(lap.balance_deg, 0.06f32.to_degrees()), "{}", lap.balance_deg);
    }

    #[test]
    fn returns_a_lap_whose_time_arrives_with_the_count_at_that_sample() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 120);
        pages.count_lap(512_000);
        assert_eq!(pages.sample(&mut recorder).unwrap().lap_time_ms, 512_000);
        assert!(pages.drive(&mut recorder, 30).is_empty());
    }

    #[test]
    fn waits_for_a_lap_time_that_arrives_late_and_keeps_the_boundary_s_flags() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 120);
        pages.count_only(); // still the out lap's 600_000
        assert!(pages.sample(&mut recorder).is_none());
        pages.graphics[IS_VALID_LAP] = 0;
        assert!(pages.drive(&mut recorder, 2).is_empty());
        pages.i32(LAST_LAPTIME_MS, 512_000);
        let lap = pages.sample(&mut recorder).expect("the lap, three samples late");
        assert_eq!((lap.lap_time_ms, lap.valid, lap.pit), (512_000, true, false));

        // the samples while waiting belong to the next lap, which then settles as usual
        pages.drive(&mut recorder, 96);
        pages.count_lap(514_000);
        let lap = pages.sample(&mut recorder).unwrap();
        assert_eq!((lap.lap_time_ms, lap.valid, lap.pit), (514_000, false, false));
    }

    #[test]
    fn takes_the_published_time_after_two_seconds_without_a_new_one() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 120);
        pages.count_lap(600_000); // the same time as the lap before, to the millisecond
        assert!(pages.sample(&mut recorder).is_none());
        assert!(pages.drive(&mut recorder, 19).is_empty());
        assert_eq!(pages.sample(&mut recorder).unwrap().lap_time_ms, 600_000);
    }

    #[test]
    fn drops_the_lap_the_app_joined_midway() {
        let mut pages = Pages::new();
        let mut recorder = LapRecorder::new();
        assert!(pages.drive(&mut recorder, 500).is_empty());
        pages.count_lap(500_000);
        assert!(pages.sample(&mut recorder).is_none());
        assert!(pages.drive(&mut recorder, 150).is_empty());
        pages.count_lap(510_000);
        assert_eq!(pages.sample(&mut recorder).unwrap().lap_time_ms, 510_000);
    }

    #[test]
    fn marks_an_in_lap_and_an_invalid_lap() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.f32s(WHEELS_PRESSURE, &[27.4, 27.5, 26.8, 26.9]);
        pages.drive(&mut recorder, 110);
        // the game counts the lap after the car entered the pit lane
        pages.graphics[IS_IN_PIT_LANE] = 1;
        pages.f32s(WHEELS_PRESSURE, &[26.0, 26.0, 26.0, 26.0]);
        pages.drive(&mut recorder, 10);
        pages.count_lap(530_000);
        let lap = pages.sample(&mut recorder).unwrap();
        assert!(lap.pit && lap.valid);
        // the pressures at the lap's end are those of its last sample off the pit lane
        assert_eq!(lap.tyres.each_ref().map(|t| t.pressure_end), [27.4, 27.5, 26.8, 26.9]);
        // leaving the pit lane starts the next lap, still marked: its time includes the pit lane
        pages.drive(&mut recorder, 10);
        pages.graphics[IS_IN_PIT_LANE] = 0;
        pages.drive(&mut recorder, 110);
        pages.count_lap(540_000);
        let lap = pages.sample(&mut recorder).unwrap();
        assert!(lap.pit && lap.valid);

        pages.drive(&mut recorder, 50);
        pages.graphics[IS_VALID_LAP] = 0;
        pages.drive(&mut recorder, 60);
        pages.count_lap(520_000);
        pages.graphics[IS_VALID_LAP] = 1; // the new lap starts valid; the finished one was not
        let lap = pages.sample(&mut recorder).unwrap();
        assert!(!lap.pit && !lap.valid);
    }

    #[test]
    fn keeps_the_first_lap_after_leaving_the_box() {
        // Touristenfahrten: the stint starts in the box, refuelled there; the counter ticks at the end split
        let mut pages = Pages::new();
        pages.graphics[IS_IN_PIT_LANE] = 1;
        pages.fuel(10.0);
        let mut recorder = LapRecorder::new();
        pages.drive(&mut recorder, 20);
        pages.fuel(60.0);
        pages.f32s(SPEED_KMH, &[300.0]); // in the pit lane: not summed up
        pages.drive(&mut recorder, 20);
        pages.graphics[IS_IN_PIT_LANE] = 0;
        pages.f32s(SPEED_KMH, &[200.0]);
        pages.drive(&mut recorder, 60);
        pages.fuel(59.0);
        pages.drive(&mut recorder, 60);
        pages.count_lap(375_780);
        let lap = pages.sample(&mut recorder).expect("the first timed lap");
        assert_eq!((lap.lap_time_ms, lap.pit, lap.top_speed_kmh, lap.fuel_used_l), (375_780, false, 200.0, 1.0));

        // the next one ends at the end split, then the car goes into the pit lane: the bit before it is dropped,
        // and leaving the pit lane starts a fresh lap again
        pages.drive(&mut recorder, 50);
        pages.graphics[IS_IN_PIT_LANE] = 1;
        pages.drive(&mut recorder, 300);
        pages.fuel(61.0);
        pages.graphics[IS_IN_PIT_LANE] = 0;
        pages.drive(&mut recorder, 100);
        pages.count_lap(368_283);
        let lap = pages.sample(&mut recorder).expect("the lap after the stop");
        assert_eq!((lap.lap_time_ms, lap.pit, lap.fuel_used_l), (368_283, false, 0.0));
    }

    #[test]
    fn counts_only_the_fuel_burnt() {
        let mut pages = Pages::new();
        pages.fuel(40.0);
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 40);
        pages.fuel(39.0);
        pages.drive(&mut recorder, 30);
        pages.fuel(60.0); // refuelled without the pit lane (a reset to the box, say)
        pages.drive(&mut recorder, 30);
        pages.fuel(59.5);
        pages.drive(&mut recorder, 10);
        pages.count_lap(500_000);
        assert_eq!(pages.sample(&mut recorder).unwrap().fuel_used_l, 1.5);
    }

    #[test]
    fn keeps_a_kerb_strike_out_of_the_extremes() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.f32s(SPEED_KMH, &[150.0]);
        pages.f32s(ACC_G, &[1.0, 0.0, 1.0]);
        pages.drive(&mut recorder, 50);
        pages.f32s(ACC_G, &[12.3, -6.5, 1.0]);
        pages.f32s(RIDE_HEIGHT, &[-0.016, -0.022]);
        pages.f32s(BRAKE, &[0.5]);
        pages.drive(&mut recorder, 1);
        pages.f32s(ACC_G, &[1.0, 0.0, 1.0]);
        pages.f32s(RIDE_HEIGHT, &[0.07, 0.08]);
        pages.f32s(BRAKE, &[0.0]);
        pages.drive(&mut recorder, 20);
        // short stops (under a second) count for nothing; ten braking samples in a row at 1.4 G do
        pages.f32s(BRAKE, &[0.9]);
        pages.f32s(ACC_G, &[0.0, -3.0, 1.0]);
        for _ in 0..3 {
            pages.drive(&mut recorder, 5);
            pages.f32s(BRAKE, &[0.0]);
            pages.drive(&mut recorder, 1);
            pages.f32s(BRAKE, &[0.9]);
        }
        pages.f32s(ACC_G, &[0.0, -1.4, 1.0]);
        pages.drive(&mut recorder, 10);
        pages.f32s(BRAKE, &[0.0]);
        pages.f32s(ACC_G, &[0.0, 0.0, 1.0]);
        pages.drive(&mut recorder, 20);
        pages.count_lap(500_000);
        let lap = pages.sample(&mut recorder).unwrap();
        assert!(close(lap.lat_g_max, (9.0 + 12.3) / 10.0), "{}", lap.lat_g_max);
        assert!(close(lap.brake_g_max, 1.4), "{}", lap.brake_g_max);
        assert!(close(lap.ride_height_min[0], (9.0 * 0.07 - 0.016) / 10.0), "{:?}", lap.ride_height_min);
        assert!(close(lap.ride_height_min[1], (9.0 * 0.08 - 0.022) / 10.0), "{:?}", lap.ride_height_min);
    }

    #[test]
    fn starts_over_when_the_counter_jumps_or_goes_back() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 150);
        pages.i32(TOTAL_LAP_COUNT, 3);
        pages.i32(LAST_LAPTIME_MS, 500_000);
        assert!(pages.sample(&mut recorder).is_none(), "jumped by 2");
        pages.drive(&mut recorder, 150);
        pages.count_lap(500_000);
        assert!(pages.sample(&mut recorder).is_none(), "the first lap after the jump has no boundary");
        pages.drive(&mut recorder, 150);
        pages.i32(TOTAL_LAP_COUNT, 0);
        assert!(pages.sample(&mut recorder).is_none(), "a new session");
        pages.drive(&mut recorder, 150);
        pages.count_lap(500_000);
        assert!(pages.sample(&mut recorder).is_none(), "out lap of the new session");
        pages.drive(&mut recorder, 150);
        pages.count_lap(505_000);
        assert_eq!(pages.sample(&mut recorder).unwrap().lap_time_ms, 505_000);
    }

    #[test]
    fn records_any_car_by_the_name_the_pages_give() {
        let mut pages = Pages::new();
        pages.car("BMW M2 Coupe");
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 150);
        pages.count_lap(500_000);
        assert_eq!(pages.sample(&mut recorder).unwrap().car, "BMW M2 Coupe");
    }

    #[test]
    fn ignores_samples_while_paused_or_out_of_live_driving() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 100);
        pages.i32(STATUS, 3);
        pages.f32s(SPEED_KMH, &[999.0]);
        pages.drive(&mut recorder, 300);
        pages.i32(STATUS, LIVE);
        pages.f32s(SPEED_KMH, &[200.0]);
        pages.count_lap(515_000);
        let lap = pages.sample(&mut recorder).unwrap();
        assert_eq!(lap.top_speed_kmh, 0.0, "the paused samples and the boundary sample are not part of the lap");
        assert!(pages.sample(&mut recorder).is_none());
        assert!(recorder.sample(&pages.graphics[..100], &pages.physics, &pages.statics).is_none());
    }

    #[test]
    fn starts_over_on_another_car_or_track() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 150);
        pages.car("BMW M3 E46 Clubsport");
        pages.drive(&mut recorder, 150);
        pages.count_lap(500_000);
        assert!(pages.sample(&mut recorder).is_none(), "another car: its first lap has no boundary");
        pages.drive(&mut recorder, 150);
        pages.statics[TRACK_CONFIGURATION..TRACK_CONFIGURATION + 2].copy_from_slice(b"NS");
        pages.drive(&mut recorder, 150);
        pages.count_lap(500_000);
        assert!(pages.sample(&mut recorder).is_none(), "another layout");
    }

    #[test]
    fn drops_a_lap_with_too_few_samples_or_without_a_time() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.drive(&mut recorder, 98);
        pages.count_lap(500_000);
        assert!(pages.sample(&mut recorder).is_none(), "99 samples");
        pages.drive(&mut recorder, 150);
        pages.count_lap(0);
        assert!(pages.sample(&mut recorder).is_none(), "no lap time");
        pages.drive(&mut recorder, 99);
        pages.count_lap(500_000);
        assert!(pages.sample(&mut recorder).is_some(), "100 samples");
    }
}
