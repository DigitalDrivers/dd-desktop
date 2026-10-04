//! Lap summaries of the club's own cars (ids starting with `dd_`) from what Assetto Corsa EVO publishes in its
//! shared memory (`Local\acevo_pmf_physics`, `Local\acevo_pmf_graphics` and `Local\acevo_pmf_static`). The
//! shell samples the pages about ten times a second and hands the bytes here; a lap is summed up when the game
//! counts it. Offsets are those of the official header with `#pragma pack(4)` (page sizes 800, 4900 and 208
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
const TYRE_TEMP_I: usize = 368;
const TYRE_TEMP_M: usize = 384;
const TYRE_TEMP_O: usize = 400;
const BRAKE_BIAS: usize = 564;
const SLIP_RATIO: usize = 640;
const SLIP_ANGLE: usize = 656;
const TC_IN_ACTION: usize = 672;
const ABS_IN_ACTION: usize = 676;
const BRAKE_TORQUE: usize = 716;
// static
const TRACK: usize = 136;
const TRACK_CONFIGURATION: usize = 169;
const IS_ONLINE: usize = 86;
/// `ACEVO_STATUS`: 2 = live driving
const LIVE: i32 = 2;
/// The club's cars; nothing is recorded for any other car.
const CLUB_CAR_PREFIX: &str = "dd_";
/// A lap with fewer samples (ten seconds at 10 Hz) is not summed up.
const MIN_SAMPLES: u32 = 100;
/// Live samples (two seconds at 10 Hz) a counted lap waits for its lap time.
const LAPTIME_WAIT: u32 = 20;
/// A wheel locks under braking, or spins under throttle, beyond this slip ratio (absolute).
const SLIP_LIMIT: f32 = 0.15;

/// Pressure, core and brake temperature of one wheel over a lap.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tyre {
    pub pressure_avg: f32,
    pub pressure_max: f32,
    pub core_temp_avg: f32,
    pub core_temp_max: f32,
    pub brake_temp_max: f32,
    /// Averages of the inner, middle and outer surface temperature
    pub temp_inner: f32,
    pub temp_middle: f32,
    pub temp_outer: f32,
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
    /// Fuel at the first sample minus fuel at the last; negative after refuelling.
    pub fuel_used_l: f32,
    pub air_temp_c: f32,
    pub road_temp_c: f32,
    pub top_speed_kmh: f32,
    /// FL FR RL RR
    pub tyres: [Tyre; 4],
    /// Front, rear
    pub ride_height_min: [f32; 2],
    pub ride_height_avg: [f32; 2],
    /// The brake bias setting, averaged over the braking samples
    pub brake_bias: f32,
    /// The front axle's share of the brake torque, averaged over the braking samples with any torque
    pub brake_front: f32,
    /// Shares of the braking samples with ABS, and of the throttle samples with traction control, working
    pub abs_share: f32,
    pub tc_share: f32,
    /// Front minus rear mean |slip angle| in degrees over the cornering samples: above 0 understeer, below oversteer
    pub balance_deg: f32,
    pub rpm_max: i32,
    /// The gear of the sample with the highest speed
    pub gear_at_top_speed: i32,
    /// Highest |lateral G|, and highest |longitudinal G| under braking
    pub lat_g_max: f32,
    pub brake_g_max: f32,
    #[serde(skip)]
    pub first: FirstSample,
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
    temp_inner: [f32; 4],
    temp_middle: [f32; 4],
    temp_outer: [f32; 4],
    brake_bias: f32,
    slip_ratio: [f32; 4],
    slip_angle: [f32; 4],
    tc: bool,
    abs: bool,
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
    /// None unless the game drives a club car live.
    fn read(graphics: &[u8], physics: &[u8], statics: &[u8]) -> Option<Sample> {
        if graphics.len() < GRAPHICS_SIZE || physics.len() < PHYSICS_SIZE || statics.len() < STATIC_SIZE {
            return None;
        }
        if i32_at(graphics, STATUS) != LIVE {
            return None;
        }
        let car = text_at(graphics, CAR_MODEL, 33);
        if !car.starts_with(CLUB_CAR_PREFIX) {
            return None;
        }
        Some(Sample {
            car,
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
            temp_inner: f32s_at(physics, TYRE_TEMP_I),
            temp_middle: f32s_at(physics, TYRE_TEMP_M),
            temp_outer: f32s_at(physics, TYRE_TEMP_O),
            brake_bias: f32_at(physics, BRAKE_BIAS),
            slip_ratio: f32s_at(physics, SLIP_RATIO),
            slip_angle: f32s_at(physics, SLIP_ANGLE),
            tc: i32_at(physics, TC_IN_ACTION) != 0,
            abs: i32_at(physics, ABS_IN_ACTION) != 0,
            brake_torque: f32s_at(physics, BRAKE_TORQUE),
        })
    }
}

/// The running sums of the lap being driven.
struct Lap {
    samples: u32,
    first: FirstSample,
    online: bool,
    fuel_first: f32,
    fuel_last: f32,
    valid: bool,
    pit: bool,
    top_speed: f32,
    pressure_sum: [f64; 4],
    pressure_max: [f32; 4],
    core_sum: [f64; 4],
    core_max: [f32; 4],
    brake_max: [f32; 4],
    ride_min: [f32; 2],
    ride_sum: [f64; 2],
    air_sum: f64,
    road_sum: f64,
    gear_at_top: i32,
    rpm_max: i32,
    lat_g_max: f32,
    temp_sum: [[f64; 4]; 3],
    travel_sum: [f64; 4],
    travel_max: [f32; 4],
    braking: u32,
    locked: [u32; 4],
    abs: u32,
    bias_sum: f64,
    brake_g_max: f32,
    /// Braking samples with any brake torque, and the sum of their front shares
    torque_samples: u32,
    front_sum: f64,
    throttle: u32,
    spinning: [u32; 4],
    tc: u32,
    cornering: u32,
    balance_sum: f64,
}

/// `sum / count`: a share or an average over some of the samples; 0 when there are none.
fn per(sum: f64, count: u32) -> f32 {
    if count == 0 { 0.0 } else { (sum / count as f64) as f32 }
}

impl Lap {
    fn start(s: &Sample) -> Lap {
        let mut lap = Lap {
            samples: 0,
            first: FirstSample { pressure: s.pressure, core_temp: s.core_temp, ride_height: s.ride_height },
            online: s.online,
            fuel_first: s.fuel,
            fuel_last: s.fuel,
            valid: s.valid,
            pit: false,
            top_speed: f32::NEG_INFINITY,
            pressure_sum: [0.0; 4],
            pressure_max: [f32::NEG_INFINITY; 4],
            core_sum: [0.0; 4],
            core_max: [f32::NEG_INFINITY; 4],
            brake_max: [f32::NEG_INFINITY; 4],
            ride_min: [f32::INFINITY; 2],
            ride_sum: [0.0; 2],
            air_sum: 0.0,
            road_sum: 0.0,
            gear_at_top: 0,
            rpm_max: i32::MIN,
            lat_g_max: 0.0,
            temp_sum: [[0.0; 4]; 3],
            travel_sum: [0.0; 4],
            travel_max: [f32::NEG_INFINITY; 4],
            braking: 0,
            locked: [0; 4],
            abs: 0,
            bias_sum: 0.0,
            brake_g_max: 0.0,
            torque_samples: 0,
            front_sum: 0.0,
            throttle: 0,
            spinning: [0; 4],
            tc: 0,
            cornering: 0,
            balance_sum: 0.0,
        };
        lap.add(s);
        lap
    }

    fn add(&mut self, s: &Sample) {
        self.samples += 1;
        self.fuel_last = s.fuel;
        self.valid = s.valid;
        self.pit |= s.pit;
        if s.speed > self.top_speed {
            self.top_speed = s.speed;
            self.gear_at_top = s.gear;
        }
        self.rpm_max = self.rpm_max.max(s.rpm);
        self.lat_g_max = self.lat_g_max.max(s.acc_g[0].abs());
        let (braking, throttle) = (s.braking(), s.throttle());
        if braking {
            self.braking += 1;
            self.abs += s.abs as u32;
            self.bias_sum += s.brake_bias as f64;
            self.brake_g_max = self.brake_g_max.max(s.acc_g[1].abs());
            let torque: f32 = s.brake_torque.iter().sum();
            if torque > 0.0 {
                self.torque_samples += 1;
                self.front_sum += ((s.brake_torque[0] + s.brake_torque[1]) / torque) as f64;
            }
        }
        if throttle {
            self.throttle += 1;
            self.tc += s.tc as u32;
        }
        if s.cornering() {
            self.cornering += 1;
            let mean = |a: f32, b: f32| (a.abs() + b.abs()) / 2.0;
            let front_minus_rear = mean(s.slip_angle[0], s.slip_angle[1]) - mean(s.slip_angle[2], s.slip_angle[3]);
            self.balance_sum += front_minus_rear.to_degrees() as f64;
        }
        for w in 0..4 {
            self.pressure_sum[w] += s.pressure[w] as f64;
            self.pressure_max[w] = self.pressure_max[w].max(s.pressure[w]);
            self.core_sum[w] += s.core_temp[w] as f64;
            self.core_max[w] = self.core_max[w].max(s.core_temp[w]);
            self.brake_max[w] = self.brake_max[w].max(s.brake_temp[w]);
            self.temp_sum[0][w] += s.temp_inner[w] as f64;
            self.temp_sum[1][w] += s.temp_middle[w] as f64;
            self.temp_sum[2][w] += s.temp_outer[w] as f64;
            self.travel_sum[w] += s.travel[w] as f64;
            self.travel_max[w] = self.travel_max[w].max(s.travel[w]);
            let slipping = s.slip_ratio[w].abs() > SLIP_LIMIT;
            self.locked[w] += (braking && slipping) as u32;
            self.spinning[w] += (throttle && slipping) as u32;
        }
        for axle in 0..2 {
            self.ride_min[axle] = self.ride_min[axle].min(s.ride_height[axle]);
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
            fuel_used_l: self.fuel_first - self.fuel_last,
            air_temp_c: avg(self.air_sum),
            road_temp_c: avg(self.road_sum),
            top_speed_kmh: self.top_speed,
            tyres: std::array::from_fn(|w| Tyre {
                pressure_avg: avg(self.pressure_sum[w]),
                pressure_max: self.pressure_max[w],
                core_temp_avg: avg(self.core_sum[w]),
                core_temp_max: self.core_max[w],
                brake_temp_max: self.brake_max[w],
                temp_inner: avg(self.temp_sum[0][w]),
                temp_middle: avg(self.temp_sum[1][w]),
                temp_outer: avg(self.temp_sum[2][w]),
                travel_max: self.travel_max[w],
                travel_avg: avg(self.travel_sum[w]),
                lock_share: per(self.locked[w] as f64, self.braking),
                spin_share: per(self.spinning[w] as f64, self.throttle),
            }),
            ride_height_min: self.ride_min,
            ride_height_avg: [avg(self.ride_sum[0]), avg(self.ride_sum[1])],
            brake_bias: per(self.bias_sum, self.braking),
            brake_front: per(self.front_sum, self.torque_samples),
            abs_share: per(self.abs as f64, self.braking),
            tc_share: per(self.tc as f64, self.throttle),
            balance_deg: per(self.balance_sum, self.cornering),
            rpm_max: self.rpm_max,
            gear_at_top_speed: self.gear_at_top,
            lat_g_max: self.lat_g_max,
            brake_g_max: self.brake_g_max,
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
    /// Whether the lap being driven started at a counted boundary (not the out lap, nor one joined midway)
    from_boundary: bool,
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
            lap: Lap::start(s),
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

/// Turns samples of the pages into finished laps. Samples outside live driving of a club car are ignored; a
/// change of car or track, or a lap counter that jumps or goes back (a new session), starts over and forgets a
/// lap still waiting for its time.
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
        match s.lap_count - stint.lap_count {
            0 => {
                stint.lap.add(&s);
                if let Some(pending) = &mut stint.pending {
                    pending.waited += 1;
                }
                stint.settle(&s, false)
            }
            1 => {
                // A lap still waiting means the new one is shorter than the wait and will not be kept.
                let earlier = stint.settle(&s, true);
                let lap = std::mem::replace(&mut stint.lap, Lap::start(&s));
                let keep = stint.from_boundary && lap.samples >= MIN_SAMPLES;
                stint.pending = Some(Pending { lap, keep, waited: 0 });
                stint.from_boundary = true;
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
            pages.car("dd_bmw_m3_e46_gt3");
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
        pages.f32s(TYRE_TEMP_I, &[90.0, 91.0, 92.0, 93.0]);
        pages.f32s(TYRE_TEMP_M, &[85.0, 85.0, 85.0, 85.0]);
        pages.f32s(TYRE_TEMP_O, &[80.0, 80.0, 80.0, 80.0]);
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
        pages.physics_i32(GEAR, 6);
        pages.physics_i32(RPMS, 7800);
        assert!(pages.drive(&mut recorder, 1).is_empty());
        pages.count_lap(512_345);
        let lap = pages.sample(&mut recorder).expect("the lap");

        assert_eq!((lap.car.as_str(), lap.track.as_str(), lap.layout.as_str(), lap.online), ("dd_bmw_m3_e46_gt3", "Nurburgring", "GP", true));
        assert_eq!((lap.lap_time_ms, lap.valid, lap.pit), (512_345, true, false));
        assert_eq!(lap.fuel_used_l, 3.5);
        assert_eq!(lap.top_speed_kmh, 251.5);
        assert_eq!((lap.air_temp_c, lap.road_temp_c), (22.0, 30.0));
        let fl = &lap.tyres[0];
        assert_eq!((fl.pressure_avg, fl.pressure_max, fl.core_temp_avg, fl.core_temp_max, fl.brake_temp_max), (26.02, 28.0, 80.1, 90.0, 600.0));
        assert_eq!((fl.temp_inner, fl.temp_middle, fl.temp_outer, lap.tyres[3].temp_inner), (90.0, 85.0, 80.0, 93.0));
        assert_eq!((fl.travel_max, lap.tyres[1].travel_max), (0.05, 0.03));
        assert!((fl.travel_avg - 0.0302).abs() < 1e-6);
        assert_eq!((lap.rpm_max, lap.gear_at_top_speed), (7800, 6));
        // no braking, throttle or cornering samples in this lap: every share and average over them is 0
        assert!(lap.tyres.iter().all(|t| t.lock_share == 0.0 && t.spin_share == 0.0));
        assert_eq!((lap.brake_bias, lap.brake_front, lap.abs_share, lap.tc_share, lap.balance_deg), (0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!((lap.lat_g_max, lap.brake_g_max), (0.0, 0.0));
        assert_eq!(lap.tyres[3].brake_temp_max, 410.0);
        assert_eq!(lap.ride_height_min, [0.05, 0.06]);
        assert!((lap.ride_height_avg[0] - 0.0698).abs() < 1e-6);
        assert_eq!(lap.first, FirstSample { pressure: [26.0, 26.5, 25.0, 25.5], core_temp: [80.0, 81.0, 70.0, 71.0], ride_height: [0.07, 0.08] });

        let json = serde_json::to_value(&lap).unwrap();
        assert_eq!(json["lapTimeMs"], 512_345);
        assert_eq!(json["fuelUsedL"], 3.5);
        assert_eq!(json["tyres"].as_array().unwrap().len(), 4);
        assert_eq!(json["tyres"][0]["brakeTempMax"], 600.0);
        let mut tyre_keys: Vec<_> = json["tyres"][0].as_object().unwrap().keys().cloned().collect();
        tyre_keys.sort();
        assert_eq!(tyre_keys, ["brakeTempMax", "coreTempAvg", "coreTempMax", "lockShare", "pressureAvg", "pressureMax", "spinShare", "tempInner", "tempMiddle", "tempOuter", "travelAvg", "travelMax"]);
        assert_eq!(json["gearAtTopSpeed"], 6);
        assert_eq!(json["rideHeightMin"].as_array().unwrap().len(), 2);
        assert!(json.get("first").is_none());
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, [
            "absShare", "airTempC", "balanceDeg", "brakeBias", "brakeFront", "brakeGMax", "car", "fuelUsedL", "gearAtTopSpeed", "lapTimeMs", "latGMax",
            "layout", "online", "pit", "rideHeightAvg", "rideHeightMin", "roadTempC", "rpmMax", "tcShare", "topSpeedKmh", "track", "tyres", "valid",
        ]);
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn sums_up_braking_throttle_and_cornering() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages); // the boundary sample stands still: none of the three

        // braking at 150 km/h: half with all four torques (80 % front), ABS and the front left locking at -1.5 G,
        // half without torque at -1.2 G
        pages.f32s(SPEED_KMH, &[150.0]);
        pages.f32s(BRAKE, &[0.8]);
        pages.f32s(BRAKE_BIAS, &[0.56]);
        pages.f32s(BRAKE_TORQUE, &[1000.0, 1000.0, 250.0, 250.0]);
        pages.f32s(SLIP_RATIO, &[-0.2, 0.1, 0.0, 0.0]);
        pages.physics_i32(ABS_IN_ACTION, 1);
        pages.f32s(ACC_G, &[0.0, -1.5, 1.0]);
        pages.drive(&mut recorder, 20);
        pages.f32s(BRAKE_TORQUE, &[0.0; 4]);
        pages.f32s(SLIP_RATIO, &[0.0; 4]);
        pages.physics_i32(ABS_IN_ACTION, 0);
        pages.f32s(ACC_G, &[0.0, -1.2, 1.0]);
        pages.drive(&mut recorder, 20);

        // full throttle in 5th: half with the rear wheels spinning and traction control on; the top speed in 6th
        pages.f32s(BRAKE, &[0.0]);
        pages.f32s(GAS, &[1.0]);
        pages.f32s(SPEED_KMH, &[200.0]);
        pages.f32s(ACC_G, &[0.0, 0.5, 1.0]);
        pages.physics_i32(GEAR, 5);
        pages.physics_i32(RPMS, 7000);
        pages.f32s(SLIP_RATIO, &[0.0, 0.0, 0.3, -0.3]);
        pages.physics_i32(TC_IN_ACTION, 1);
        pages.drive(&mut recorder, 15);
        pages.f32s(SLIP_RATIO, &[0.0; 4]);
        pages.physics_i32(TC_IN_ACTION, 0);
        pages.drive(&mut recorder, 14);
        pages.f32s(SPEED_KMH, &[260.0]);
        pages.physics_i32(GEAR, 6);
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
        assert_eq!(lap.abs_share, 0.5);
        assert_eq!((lap.tyres[0].lock_share, lap.tyres[1].lock_share, lap.tyres[2].lock_share), (0.5, 0.0, 0.0));
        assert_eq!(lap.brake_g_max, 1.5);
        assert_eq!(lap.tc_share, 0.5);
        assert_eq!((lap.tyres[0].spin_share, lap.tyres[2].spin_share, lap.tyres[3].spin_share), (0.0, 0.5, 0.5));
        assert_eq!((lap.top_speed_kmh, lap.gear_at_top_speed, lap.rpm_max), (260.0, 6, 8200));
        assert_eq!(lap.lat_g_max, 1.6);
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
        pages.graphics[IS_IN_PIT_LANE] = 1;
        pages.graphics[IS_VALID_LAP] = 0;
        assert!(pages.drive(&mut recorder, 2).is_empty());
        pages.i32(LAST_LAPTIME_MS, 512_000);
        let lap = pages.sample(&mut recorder).expect("the lap, three samples late");
        assert_eq!((lap.lap_time_ms, lap.valid, lap.pit), (512_000, true, false));

        // the samples while waiting belong to the next lap, which then settles as usual
        pages.graphics[IS_IN_PIT_LANE] = 0;
        pages.drive(&mut recorder, 96);
        pages.count_lap(514_000);
        let lap = pages.sample(&mut recorder).unwrap();
        assert_eq!((lap.lap_time_ms, lap.valid, lap.pit), (514_000, false, true));
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
    fn marks_a_lap_through_the_pits_and_an_invalid_lap() {
        let mut pages = Pages::new();
        let mut recorder = past_the_out_lap(&mut pages);
        pages.graphics[IS_IN_PIT_LANE] = 1;
        pages.drive(&mut recorder, 10);
        pages.graphics[IS_IN_PIT_LANE] = 0;
        pages.drive(&mut recorder, 100);
        pages.count_lap(530_000);
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
    fn ignores_cars_that_are_not_the_club_s() {
        let mut pages = Pages::new();
        pages.car("bmw_m3_e46_csl");
        let mut recorder = LapRecorder::new();
        for lap in 0..3 {
            assert!(pages.drive(&mut recorder, 150).is_empty());
            pages.count_lap(500_000 + lap);
        }
        assert!(recorder.stint.is_none());
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
        pages.car("dd_e46clubsport");
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
