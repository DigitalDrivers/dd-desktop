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

/// Pressure, core and brake temperature of one wheel over a lap.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tyre {
    pub pressure_avg: f32,
    pub pressure_max: f32,
    pub core_temp_avg: f32,
    pub core_temp_max: f32,
    pub brake_temp_max: f32,
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
        };
        lap.add(s);
        lap
    }

    fn add(&mut self, s: &Sample) {
        self.samples += 1;
        self.fuel_last = s.fuel;
        self.valid = s.valid;
        self.pit |= s.pit;
        self.top_speed = self.top_speed.max(s.speed);
        for w in 0..4 {
            self.pressure_sum[w] += s.pressure[w] as f64;
            self.pressure_max[w] = self.pressure_max[w].max(s.pressure[w]);
            self.core_sum[w] += s.core_temp[w] as f64;
            self.core_max[w] = self.core_max[w].max(s.core_temp[w]);
            self.brake_max[w] = self.brake_max[w].max(s.brake_temp[w]);
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
            }),
            ride_height_min: self.ride_min,
            ride_height_avg: [avg(self.ride_sum[0]), avg(self.ride_sum[1])],
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
        // the lap's first sample is the one at the boundary; 98 more, then one with other values: 100
        let mut recorder = past_the_out_lap(&mut pages);
        assert!(pages.drive(&mut recorder, 98).is_empty());
        pages.fuel(36.5);
        pages.f32s(SPEED_KMH, &[251.5]);
        pages.f32s(WHEELS_PRESSURE, &[28.0, 28.5, 27.0, 27.5]);
        pages.f32s(TYRE_CORE_TEMPERATURE, &[90.0, 91.0, 80.0, 81.0]);
        pages.f32s(BRAKE_TEMP, &[600.0, 610.0, 400.0, 410.0]);
        pages.f32s(RIDE_HEIGHT, &[0.05, 0.06]);
        assert!(pages.drive(&mut recorder, 1).is_empty());
        pages.count_lap(512_345);
        let lap = pages.sample(&mut recorder).expect("the lap");

        assert_eq!((lap.car.as_str(), lap.track.as_str(), lap.layout.as_str(), lap.online), ("dd_bmw_m3_e46_gt3", "Nurburgring", "GP", true));
        assert_eq!((lap.lap_time_ms, lap.valid, lap.pit), (512_345, true, false));
        assert_eq!(lap.fuel_used_l, 3.5);
        assert_eq!(lap.top_speed_kmh, 251.5);
        assert_eq!((lap.air_temp_c, lap.road_temp_c), (22.0, 30.0));
        assert_eq!(lap.tyres[0], Tyre { pressure_avg: 26.02, pressure_max: 28.0, core_temp_avg: 80.1, core_temp_max: 90.0, brake_temp_max: 600.0 });
        assert_eq!(lap.tyres[3].brake_temp_max, 410.0);
        assert_eq!(lap.ride_height_min, [0.05, 0.06]);
        assert!((lap.ride_height_avg[0] - 0.0698).abs() < 1e-6);
        assert_eq!(lap.first, FirstSample { pressure: [26.0, 26.5, 25.0, 25.5], core_temp: [80.0, 81.0, 70.0, 71.0], ride_height: [0.07, 0.08] });

        let json = serde_json::to_value(&lap).unwrap();
        assert_eq!(json["lapTimeMs"], 512_345);
        assert_eq!(json["fuelUsedL"], 3.5);
        assert_eq!(json["tyres"].as_array().unwrap().len(), 4);
        assert_eq!(json["tyres"][0]["brakeTempMax"], 600.0);
        assert_eq!(json["rideHeightMin"].as_array().unwrap().len(), 2);
        assert!(json.get("first").is_none());
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["airTempC", "car", "fuelUsedL", "lapTimeMs", "layout", "online", "pit", "rideHeightAvg", "rideHeightMin", "roadTempC", "topSpeedKmh", "track", "tyres", "valid"]);
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
