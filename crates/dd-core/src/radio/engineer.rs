//! The race engineer on the team radio: what the game publishes about the driver's own car, turned into the calls
//! the radio speaks (research 2026-11-09-boxenfunk/A-evo-spotter-data.md §4 and §5): fuel for so many laps, fuel
//! low and box this lap, the lap time or a new personal best at the line, an invalid lap, brakes very hot, tyres in
//! the window.
//!
//! It reads the pages itself rather than a [`super::frame::Frame`]: a frame needs the own car's slot in
//! `car_coordinates`, which the engineer does not, so it keeps talking when the slots are empty. Only fields seen
//! working in the game are read ([`Sample`]); the pit limiter, `best_laptime_ms`, the game's own fuel averages and the
//! session's time and laps left join that one struct once a recording has confirmed them.
//!
//! Laps are the app's lap list's: [`LapRecorder`] decides when a lap ends, its time, whether it was valid, whether
//! it went through the pit lane, and the fuel it burnt (refuelling and the pit lane do not count). A lap through
//! the pit lane (an in-lap, or an out-lap whose time includes the pit lane) gets no call; on Touristenfahrten every
//! stint's lap starts at the pit exit and the game counts it before the pit lane, so those laps count like any.
//!
//! The rules (thresholds in [`Config`]):
//! - **Fuel for X laps** at the line after a lap not through the pit lane, X = fuel now ÷ the mean use of the last
//!   up to three such laps. Said after the first such lap (and the first after refuelling), then every third lap,
//!   or earlier when X is half a lap or more off what the last call let the driver expect (X then minus the laps
//!   since): it burns about one lap a lap, so a plain "X changed by half a lap" would be every lap. A lap that
//!   burnt (almost) nothing does not count: the session has fuel use off.
//! - **Fuel low** once when X drops below 2 laps, **box this lap** once below 1.2, each with the fuel sentence
//!   after it; refuelling (fuel up by more than a litre) arms both again. Judged at the line only.
//! - **Lap time** at the line after a valid lap; **new personal best** when it beats the best known lap (the one
//!   the shell gives, or this session's best). Without a best given, the first valid lap only sets it.
//! - **Lap invalid** once a lap when `is_valid_lap` goes from 1 to 0, not in the pit lane, not at the sample that
//!   counts a lap, and not in the first seconds after the pit exit (what the flag does there is not known).
//! - **Brakes very hot** when any brake is above the coach's 800 °C for 2 s; again only after all cooled below the
//!   coach's 700 °C and at most once every three minutes.
//! - **Tyres in the window** once a stint (the stint starts at the pit exit) when all four core temperatures are
//!   in the window after one of them was seen below it: warm tyres after a short stop get no call.
//! - Nothing is said while the car is in the pit lane, except the calls of a lap that ended before it.
//!
//! Every call has [`Priority::Engineer`] and expires after [`EXPIRES_S`]. The timing between calls of one key lives
//! here, so the queue needs no cooldowns for them.

use std::collections::VecDeque;

use super::phrases;
use super::queue::{Call, Priority};
use crate::engineer::measure::{HOT, VERY_HOT};
use crate::evo_memory::{f32_at, GRAPHICS_SIZE, STATIC_SIZE};
use crate::laps::{self, i32_at, f32s_at, LapRecorder, LapSummary, PHYSICS_SIZE};

/// An engineer's call is still worth saying this long (research §3: 20 to 30 s).
pub const EXPIRES_S: f64 = 25.0;
/// The fuel per lap is the mean of this many laps.
const FUEL_LAPS: usize = 3;
/// A lap that burnt less (litres) says fuel use is off in this session.
const NO_FUEL_USE_L: f32 = 0.1;
/// The lap recorder takes the static page for the car's track and whether it is online, which the calls do not
/// need: an empty page.
const NO_STATICS: [u8; STATIC_SIZE] = [0; STATIC_SIZE];

/// The settings' call groups, so the shell can leave out what the driver switched off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Fuel,
    /// Lap times, personal bests, invalid laps
    Laps,
    /// Brakes, tyres
    Car,
}

/// What the engineer wants said.
#[derive(Debug, Clone, PartialEq)]
pub struct Say {
    pub call: Call,
    /// A sentence built now, to be spoken live; None: the library's wording of `call.key`
    pub text: Option<String>,
    pub group: Group,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The fuel sentence comes at least every this many laps ...
    pub fuel_laps_every: u32,
    /// ... and earlier when the laps left are this far off what the last one let the driver expect.
    pub fuel_laps_change: f64,
    pub fuel_low_laps: f64,
    pub fuel_box_laps: f64,
    /// Fuel up by more than this (litres) is refuelling.
    pub refuel_l: f32,
    /// Seconds after the pit exit an invalid lap is not called.
    pub invalid_grace_s: f64,
    /// °C, the coach's "very hot" ([`crate::engineer::measure`], `Finding::BrakeHot`)
    pub brakes_hot_c: f32,
    /// °C, the coach's "hot": all brakes below it arm the call again
    pub brakes_cool_c: f32,
    pub brakes_hold_s: f64,
    pub brakes_cooldown_s: f64,
    /// °C, the core temperature window. An assumption for the owner: the coach judges pressures only. The club's
    /// laps of October average 65 to 75 °C on road tyres (GT2 RS MR) and 76 to 97 °C on slicks (E46 Rennsport).
    pub tyres_min_c: f32,
    pub tyres_max_c: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fuel_laps_every: 3,
            fuel_laps_change: 0.5,
            fuel_low_laps: 2.0,
            fuel_box_laps: 1.2,
            refuel_l: 1.0,
            invalid_grace_s: 10.0,
            brakes_hot_c: VERY_HOT as f32,
            brakes_cool_c: HOT as f32,
            brakes_hold_s: 2.0,
            brakes_cooldown_s: 180.0,
            tyres_min_c: 70.0,
            tyres_max_c: 100.0,
        }
    }
}

/// What the engineer reads of one sample: only fields seen working in the game (research §1.2, [V]).
struct Sample {
    lap_count: i32,
    fuel: f32,
    valid: bool,
    pit_lane: bool,
    /// FL FR RL RR
    brake_temp: [f32; 4],
    core_temp: [f32; 4],
}

impl Sample {
    /// None unless the game drives live; the pages have the right sizes.
    fn read(physics: &[u8], graphics: &[u8]) -> Option<Sample> {
        (i32_at(graphics, laps::STATUS) == laps::LIVE).then(|| Sample {
            lap_count: i32_at(graphics, laps::TOTAL_LAP_COUNT),
            fuel: f32_at(graphics, laps::FUEL),
            valid: graphics[laps::IS_VALID_LAP] != 0,
            pit_lane: graphics[laps::IS_IN_PIT_LANE] != 0,
            brake_temp: f32s_at(physics, laps::BRAKE_TEMP),
            core_temp: f32s_at(physics, laps::TYRE_CORE_TEMPERATURE),
        })
    }
}

pub struct Engineer {
    config: Config,
    lang: String,
    laps: LapRecorder,
    /// The previous live sample's lap count, validity and pit lane flag
    last: Option<(i32, bool, bool)>,
    pit_exit_at: Option<f64>,
    // fuel
    fuel_used: VecDeque<f32>,
    /// The lowest fuel since the last refuelling
    fuel_floor: Option<f32>,
    /// The laps left the fuel sentence last said, and the laps counted since; None: say it at the next lap
    fuel_said: Option<(f64, u32)>,
    fuel_low_said: bool,
    fuel_box_said: bool,
    // laps
    given_best: Option<i64>,
    session_best: Option<i64>,
    invalid_said: bool,
    // car
    brakes_hot_since: Option<f64>,
    brakes_armed: bool,
    brakes_said_at: Option<f64>,
    tyres_cold_seen: bool,
    tyres_said: bool,
}

fn say(key: &'static str, group: Group, t: f64, text: Option<String>) -> Say {
    Say { call: Call { key, car: None, priority: Priority::Engineer, at: t, expires: t + EXPIRES_S }, text, group }
}

impl Engineer {
    /// `lang` "de" or "en" (any other is English).
    pub fn new(config: Config, lang: &str) -> Self {
        Self {
            config,
            lang: lang.to_string(),
            laps: LapRecorder::new(),
            last: None,
            pit_exit_at: None,
            fuel_used: VecDeque::new(),
            fuel_floor: None,
            fuel_said: None,
            fuel_low_said: false,
            fuel_box_said: false,
            given_best: None,
            session_best: None,
            invalid_said: false,
            brakes_hot_since: None,
            brakes_armed: true,
            brakes_said_at: None,
            tyres_cold_seen: false,
            tyres_said: false,
        }
    }

    /// The driver's best lap on this car and track before this session, if the app knows one (the shell passes it
    /// from the race engineer's store); None = the first valid lap of the session sets it. A lap of this session
    /// that is better stays the best.
    pub fn set_best_lap(&mut self, lap_ms: Option<i64>) {
        self.given_best = lap_ms;
    }

    /// One sample (20 a second). `t` = seconds on the frames' clock. Answers what to say now (usually nothing).
    /// Pages of another size, and samples outside live driving, are ignored.
    pub fn step(&mut self, t: f64, physics: &[u8], graphics: &[u8]) -> Vec<Say> {
        let mut out = Vec::new();
        if physics.len() != PHYSICS_SIZE || graphics.len() != GRAPHICS_SIZE {
            return out;
        }
        let Some(s) = Sample::read(physics, graphics) else {
            self.last = None;
            self.brakes_hot_since = None;
            return out;
        };
        self.watch_fuel(s.fuel);
        let (lap_count, was_valid, was_in_pit) = self.last.unwrap_or((s.lap_count, s.valid, s.pit_lane));
        self.last = Some((s.lap_count, s.valid, s.pit_lane));
        if was_in_pit && !s.pit_lane {
            self.pit_exit_at = Some(t);
            self.invalid_said = false;
            self.tyres_cold_seen = false;
            self.tyres_said = false;
        }
        if s.lap_count != lap_count {
            self.invalid_said = false;
        }
        if let Some(lap) = self.laps.sample(graphics, physics, &NO_STATICS) {
            self.lap_done(t, &lap, s.fuel, &mut out);
        }
        let after_exit = self.pit_exit_at.is_none_or(|exit| t - exit >= self.config.invalid_grace_s);
        if was_valid && !s.valid && s.lap_count == lap_count && !s.pit_lane && !was_in_pit && after_exit && !self.invalid_said {
            self.invalid_said = true;
            out.push(say("lap_invalid", Group::Laps, t, None));
        }
        self.watch_brakes(t, &s, &mut out);
        self.watch_tyres(t, &s, &mut out);
        out
    }

    fn watch_fuel(&mut self, fuel: f32) {
        match self.fuel_floor {
            Some(floor) if fuel > floor + self.config.refuel_l => {
                self.fuel_floor = Some(fuel);
                self.fuel_said = None;
                self.fuel_low_said = false;
                self.fuel_box_said = false;
            }
            Some(floor) => self.fuel_floor = Some(floor.min(fuel)),
            None => self.fuel_floor = Some(fuel),
        }
    }

    fn lap_done(&mut self, t: f64, lap: &LapSummary, fuel_now: f32, out: &mut Vec<Say>) {
        if let Some((_, since)) = &mut self.fuel_said {
            *since += 1;
        }
        if lap.pit {
            return;
        }
        if lap.valid {
            let ms = lap.lap_time_ms as i64;
            let best = self.given_best.into_iter().chain(self.session_best).min();
            let (key, text) = if best.is_some_and(|best| ms < best) {
                ("personal_best", phrases::personal_best(ms, &self.lang))
            } else {
                ("lap_time", phrases::lap_time(ms, &self.lang))
            };
            self.session_best = Some(self.session_best.map_or(ms, |best| best.min(ms)));
            out.push(say(key, Group::Laps, t, Some(text)));
        }
        if lap.fuel_used_l < NO_FUEL_USE_L {
            return;
        }
        if self.fuel_used.len() == FUEL_LAPS {
            self.fuel_used.pop_front();
        }
        self.fuel_used.push_back(lap.fuel_used_l);
        let per_lap = self.fuel_used.iter().map(|l| *l as f64).sum::<f64>() / self.fuel_used.len() as f64;
        let left = fuel_now as f64 / per_lap;
        let c = &self.config;
        let warning = if left < c.fuel_box_laps && !self.fuel_box_said {
            self.fuel_box_said = true;
            self.fuel_low_said = true;
            Some("fuel_box_this_lap")
        } else if left < c.fuel_low_laps && !self.fuel_low_said {
            self.fuel_low_said = true;
            Some("fuel_low")
        } else {
            None
        };
        let expected = |said: f64, since: u32| (said - since as f64).max(0.0);
        let due = self.fuel_said.is_none_or(|(said, since)| since >= c.fuel_laps_every || (left - expected(said, since)).abs() >= c.fuel_laps_change);
        if let Some(key) = warning {
            out.push(say(key, Group::Fuel, t, None));
        }
        if due || warning.is_some() {
            self.fuel_said = Some((left, 0));
            out.push(say("fuel_laps", Group::Fuel, t, Some(phrases::fuel_laps(left, &self.lang))));
        }
    }

    fn watch_brakes(&mut self, t: f64, s: &Sample, out: &mut Vec<Say>) {
        let hottest = s.brake_temp.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        if hottest < self.config.brakes_cool_c {
            self.brakes_armed = true;
        }
        if s.pit_lane || hottest <= self.config.brakes_hot_c {
            self.brakes_hot_since = None;
            return;
        }
        let since = *self.brakes_hot_since.get_or_insert(t);
        let cooled_down = self.brakes_said_at.is_none_or(|said| t - said >= self.config.brakes_cooldown_s);
        if self.brakes_armed && cooled_down && t - since >= self.config.brakes_hold_s {
            self.brakes_armed = false;
            self.brakes_said_at = Some(t);
            out.push(say("brakes_hot", Group::Car, t, None));
        }
    }

    fn watch_tyres(&mut self, t: f64, s: &Sample, out: &mut Vec<Say>) {
        if s.pit_lane || self.tyres_said {
            return;
        }
        if s.core_temp.iter().any(|c| *c < self.config.tyres_min_c) {
            self.tyres_cold_seen = true;
        } else if self.tyres_cold_seen && s.core_temp.iter().all(|c| *c <= self.config.tyres_max_c) {
            self.tyres_said = true;
            out.push(say("tyres_in_window", Group::Car, t, None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples per lap: 10 s at 20 Hz, above the lap recorder's minimum
    const LAP: usize = 200;
    const DT: f64 = 0.05;

    /// The two pages as the game publishes them, built by offset; one step is one sample, 20 a second.
    struct Pages {
        physics: Vec<u8>,
        graphics: Vec<u8>,
        t: f64,
        /// Everything said, with the time
        said: Vec<Say>,
    }

    impl Pages {
        /// Live, in the pit lane, a valid lap, 30 l, cold tyres, cool brakes.
        fn new() -> Pages {
            let mut pages = Pages { physics: vec![0; PHYSICS_SIZE], graphics: vec![0; GRAPHICS_SIZE], t: 0.0, said: Vec::new() };
            pages.graphics[laps::STATUS..laps::STATUS + 4].copy_from_slice(&laps::LIVE.to_le_bytes());
            pages.valid(true);
            pages.pit(true);
            pages.fuel(30.0);
            pages.tyres([40.0; 4]);
            pages.brakes([300.0; 4]);
            pages
        }

        fn i32(&mut self, at: usize, value: i32) {
            self.graphics[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }

        fn physics(&mut self, at: usize, values: [f32; 4]) {
            for (i, v) in values.iter().enumerate() {
                self.physics[at + i * 4..at + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        }

        fn fuel(&mut self, l: f32) {
            self.graphics[laps::FUEL..laps::FUEL + 4].copy_from_slice(&l.to_le_bytes());
        }

        fn fuel_now(&self) -> f32 {
            f32_at(&self.graphics, laps::FUEL)
        }

        fn valid(&mut self, valid: bool) {
            self.graphics[laps::IS_VALID_LAP] = valid as u8;
        }

        fn pit(&mut self, pit: bool) {
            self.graphics[laps::IS_IN_PIT_LANE] = pit as u8;
        }

        fn tyres(&mut self, c: [f32; 4]) {
            self.physics(laps::TYRE_CORE_TEMPERATURE, c);
        }

        fn brakes(&mut self, c: [f32; 4]) {
            self.physics(laps::BRAKE_TEMP, c);
        }

        /// The game counts a lap of this time (published with the count).
        fn count_lap(&mut self, ms: i32) {
            let count = i32_at(&self.graphics, laps::TOTAL_LAP_COUNT);
            self.i32(laps::TOTAL_LAP_COUNT, count + 1);
            self.i32(laps::LAST_LAPTIME_MS, ms);
        }

        /// One sample; what it said.
        fn step(&mut self, engineer: &mut Engineer) -> Vec<Say> {
            let said = engineer.step(self.t, &self.physics, &self.graphics);
            self.t += DT;
            self.said.extend(said.clone());
            said
        }

        fn drive(&mut self, engineer: &mut Engineer, samples: usize) -> Vec<Say> {
            (0..samples).flat_map(|_| self.step(engineer)).collect()
        }

        /// A lap of `LAP` samples burning `fuel_l`, then the count with its time and the second the lap recorder
        /// waits for a time equal to the last one; what was said in it and at the line.
        fn lap(&mut self, engineer: &mut Engineer, fuel_l: f32, ms: i32) -> Vec<Say> {
            let mut said = self.burn(engineer, LAP, fuel_l);
            self.count_lap(ms);
            said.extend(self.drive(engineer, 21));
            said
        }

        /// `samples` samples burning `fuel_l` evenly.
        fn burn(&mut self, engineer: &mut Engineer, samples: usize, fuel_l: f32) -> Vec<Say> {
            let mut said = Vec::new();
            for _ in 0..samples {
                let l = self.fuel_now() - fuel_l / samples as f32;
                self.fuel(l);
                said.extend(self.step(engineer));
            }
            said
        }

        /// From the box out on track: the game counts a lap at the line in the pit lane, then the car leaves the
        /// pit lane, so the out-lap goes through the pit lane.
        fn leave_the_box(&mut self, engineer: &mut Engineer) -> Vec<Say> {
            let mut said = self.drive(engineer, 20);
            self.count_lap(0);
            said.extend(self.drive(engineer, 20));
            self.pit(false);
            said.extend(self.step(engineer));
            said
        }
    }

    fn keys(said: &[Say]) -> Vec<&'static str> {
        said.iter().map(|s| s.call.key).collect()
    }

    fn texts(said: &[Say]) -> Vec<String> {
        said.iter().map(|s| s.text.clone().unwrap_or_else(|| phrases::text(s.call.key, "de", None, 0).unwrap())).collect()
    }

    fn engineer() -> Engineer {
        Engineer::new(Config::default(), "de")
    }

    /// Warm tyres and the out-lap behind: the next lap is the first that counts.
    fn on_a_flying_lap(e: &mut Engineer, pages: &mut Pages) {
        assert!(keys(&pages.leave_the_box(e)).is_empty());
        pages.tyres([85.0; 4]);
        let out_lap = pages.lap(e, 3.0, 130_000);
        assert_eq!(keys(&out_lap), ["tyres_in_window"], "the out-lap goes through the pit lane: no time, no fuel");
    }

    #[test]
    fn fuel_for_so_many_laps_after_the_first_lap_then_every_third_or_on_a_change() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        on_a_flying_lap(&mut e, &mut pages);
        // 27 l after the out-lap, 4 l a lap
        let fuel = |said: Vec<Say>| texts(&said.into_iter().filter(|s| s.group == Group::Fuel).collect::<Vec<_>>());
        let laps: Vec<Vec<String>> = (0..6).map(|_| fuel(pages.lap(&mut e, 4.0, 95_000))).collect();
        assert_eq!(laps[0], ["Sprit für noch fünfeinhalb Runden."], "23 l / 4 l = 5.75");
        assert!(laps[1].is_empty() && laps[2].is_empty(), "4.75 and 3.75 laps as expected: no nagging");
        assert_eq!(laps[3], ["Sprit für noch zweieinhalb Runden."], "the third lap since: 2.75");
        assert_eq!(laps[4], ["Sprit wird knapp.", "Sprit für noch anderthalb Runden."], "1.75 < 2");
        assert_eq!(laps[5], ["Box diese Runde, zum Tanken.", "Sprit für noch eine halbe Runde."], "0.75 < 1.2");
        let fuel: Vec<&Say> = pages.said.iter().filter(|s| s.group == Group::Fuel).collect();
        assert_eq!(fuel.iter().filter(|s| s.call.key == "fuel_low").count(), 1);
        assert_eq!(fuel.iter().filter(|s| s.call.key == "fuel_box_this_lap").count(), 1);
        assert!(fuel.iter().all(|s| s.call.priority == Priority::Engineer && s.call.expires == s.call.at + EXPIRES_S && s.call.car.is_none()));
        assert!(fuel.iter().all(|s| (s.call.key == "fuel_laps") == s.text.is_some()), "only the sentence is spoken live");
    }

    #[test]
    fn a_change_in_fuel_use_of_half_a_lap_is_said_at_once() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        pages.fuel(60.0);
        on_a_flying_lap(&mut e, &mut pages);
        assert_eq!(keys(&pages.lap(&mut e, 4.0, 95_000)), ["lap_time", "fuel_laps"], "53 l / 4 = 13.25");
        // 6 l: 47 l over a mean of 5 l = 9.4 laps, the driver expected 12.25
        let said = pages.lap(&mut e, 6.0, 95_000);
        assert_eq!(said[1].text.as_deref(), Some("Sprit für noch neun Runden."));
    }

    #[test]
    fn refuelling_arms_the_warnings_again_and_the_next_lap_says_the_fuel() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        pages.fuel(12.0);
        on_a_flying_lap(&mut e, &mut pages);
        assert_eq!(keys(&pages.lap(&mut e, 4.0, 95_000)), ["lap_time", "fuel_low", "fuel_laps"], "5 l / 4 = 1.25");
        // in for fuel: the in-lap is counted in the pit lane
        pages.pit(true);
        pages.drive(&mut e, 20);
        pages.count_lap(110_000);
        // a litre a sample, up to 13 l
        for _ in 0..8 {
            let l = pages.fuel_now() + 1.0;
            pages.fuel(l);
            pages.step(&mut e);
        }
        pages.drive(&mut e, 20);
        pages.pit(false);
        let out_lap = pages.lap(&mut e, 4.0, 130_000);
        assert!(keys(&out_lap).is_empty(), "{:?}", keys(&out_lap));
        // 5 l left again, 1.25 laps: the fuel and the warning are said again
        let said = pages.lap(&mut e, 4.0, 95_000);
        assert_eq!(keys(&said), ["lap_time", "fuel_low", "fuel_laps"]);
        assert_eq!(said[2].text.as_deref(), Some("Sprit für noch eine Runde."));
    }

    #[test]
    fn straight_to_box_this_lap_without_fuel_low_before_it() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        pages.fuel(8.0);
        on_a_flying_lap(&mut e, &mut pages);
        assert_eq!(keys(&pages.lap(&mut e, 4.0, 95_000)), ["lap_time", "fuel_box_this_lap", "fuel_laps"], "1 l / 4");
        assert_eq!(keys(&pages.lap(&mut e, 0.5, 95_000)), ["lap_time"], "0.5 l / 2.25: no second warning, no fuel_low");
    }

    #[test]
    fn no_fuel_sentence_before_a_measured_lap_or_when_the_session_burns_nothing() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        on_a_flying_lap(&mut e, &mut pages);
        for _ in 0..4 {
            assert_eq!(keys(&pages.lap(&mut e, 0.0, 95_000)), ["lap_time"]);
        }
    }

    #[test]
    fn the_in_lap_and_the_out_lap_of_a_stop_get_no_call_and_their_fuel_does_not_count() {
        // the timing line runs across the pit lane: the game counts the in-lap there
        let (mut e, mut pages) = (engineer(), Pages::new());
        pages.fuel(60.6);
        on_a_flying_lap(&mut e, &mut pages);
        assert_eq!(texts(&pages.lap(&mut e, 4.0, 95_000))[1], "Sprit für noch dreizehn Runden.", "53.6 l / 4");
        let mut said = pages.burn(&mut e, 150, 3.0);
        pages.pit(true);
        said.extend(pages.drive(&mut e, 40));
        pages.count_lap(120_000);
        said.extend(pages.drive(&mut e, 40));
        pages.pit(false);
        said.extend(pages.lap(&mut e, 1.0, 125_000));
        assert!(keys(&said).is_empty(), "{:?}", texts(&said));
        // 45.6 l at the line over the 4 l of the flying laps: 11.4 laps (with the in-lap's 3 l and the out-lap's
        // 1 l in the mean, 17.1)
        assert_eq!(texts(&pages.lap(&mut e, 4.0, 95_000)), ["Eins fünfunddreißig null.", "Sprit für noch elf Runden."]);
    }

    #[test]
    fn touristenfahrten_laps_from_the_pit_exit_count_like_any() {
        // every stint starts in the box, and the game counts the lap at the end split, before the pit lane
        let (mut e, mut pages) = (engineer(), Pages::new());
        pages.fuel(80.4);
        let mut laps = Vec::new();
        for ms in [480_000, 475_000, 478_000] {
            pages.drive(&mut e, 40);
            pages.pit(false);
            laps.push(keys(&pages.lap(&mut e, 20.0, ms)));
            pages.pit(true);
        }
        assert_eq!(laps[0], ["lap_time", "fuel_laps"], "60.4 l / 20 l");
        assert_eq!(laps[1], ["personal_best"], "2.02 laps as expected");
        assert_eq!(laps[2], ["lap_time", "fuel_box_this_lap", "fuel_laps"], "1.02 laps");
        assert_eq!(texts(&pages.said)[1], "Sprit für noch drei Runden.");
    }

    #[test]
    fn the_lap_time_or_a_new_personal_best_against_the_session_s_best() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        on_a_flying_lap(&mut e, &mut pages);
        let time = |pages: &mut Pages, e: &mut Engineer, ms| texts(&pages.lap(e, 0.0, ms));
        assert_eq!(time(&mut pages, &mut e, 95_300), ["Eins fünfunddreißig drei."], "the first lap only sets the best");
        assert_eq!(time(&mut pages, &mut e, 94_100), ["Neue Bestzeit, eins vierunddreißig eins."]);
        assert_eq!(time(&mut pages, &mut e, 94_100), ["Eins vierunddreißig eins."], "equal is no new best");
        assert_eq!(time(&mut pages, &mut e, 94_600), ["Eins vierunddreißig sechs."]);
        assert!(pages.said.iter().all(|s| s.group == Group::Laps || s.call.key == "tyres_in_window"));
    }

    #[test]
    fn a_best_lap_from_before_the_session_is_the_one_to_beat() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        e.set_best_lap(Some(94_000));
        on_a_flying_lap(&mut e, &mut pages);
        assert_eq!(keys(&pages.lap(&mut e, 0.0, 95_300)), ["lap_time"]);
        assert_eq!(keys(&pages.lap(&mut e, 0.0, 93_900)), ["personal_best"]);
        assert_eq!(keys(&pages.lap(&mut e, 0.0, 93_950)), ["lap_time"], "this session's best stays the best");
        e.set_best_lap(Some(93_000));
        assert_eq!(keys(&pages.lap(&mut e, 0.0, 93_500)), ["lap_time"]);
    }

    #[test]
    fn an_invalid_lap_is_called_once_and_gets_no_time() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        on_a_flying_lap(&mut e, &mut pages);
        pages.drive(&mut e, 60);
        pages.valid(false);
        let said = pages.drive(&mut e, 60);
        assert_eq!(keys(&said), ["lap_invalid"]);
        // the flag comes back and goes again on the same lap: still once
        pages.valid(true);
        pages.step(&mut e);
        pages.valid(false);
        assert!(pages.drive(&mut e, 60).is_empty());
        assert!(keys(&pages.lap(&mut e, 0.0, 95_000)).is_empty(), "no time for an invalid lap");
        // the game makes the next lap valid again; a cut on it is called again
        pages.valid(true);
        pages.drive(&mut e, 20);
        pages.valid(false);
        assert_eq!(keys(&pages.drive(&mut e, 10)), ["lap_invalid"]);
    }

    #[test]
    fn no_invalid_lap_at_the_line_in_the_pit_lane_or_right_after_the_pit_exit() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        on_a_flying_lap(&mut e, &mut pages);
        // the flag drops with the count
        pages.drive(&mut e, LAP);
        pages.count_lap(95_000);
        pages.valid(false);
        assert!(!keys(&pages.drive(&mut e, 20)).contains(&"lap_invalid"));
        pages.valid(true);
        pages.drive(&mut e, 20);
        // in the pit lane
        pages.pit(true);
        pages.drive(&mut e, 10);
        pages.valid(false);
        assert!(pages.drive(&mut e, 40).is_empty());
        pages.valid(true);
        pages.drive(&mut e, 20);
        // 9.5 s after the pit exit
        pages.pit(false);
        pages.drive(&mut e, 190);
        pages.valid(false);
        assert!(pages.drive(&mut e, 40).is_empty());
    }

    #[test]
    fn nothing_while_in_the_pit_lane() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        pages.brakes([900.0; 4]);
        pages.drive(&mut e, 100);
        pages.tyres([85.0; 4]);
        pages.drive(&mut e, 100);
        pages.valid(false);
        pages.drive(&mut e, 100);
        pages.fuel(5.0);
        pages.count_lap(95_000);
        pages.drive(&mut e, LAP);
        pages.count_lap(96_000);
        pages.drive(&mut e, 40);
        assert!(pages.said.is_empty(), "{:?}", keys(&pages.said));
    }

    #[test]
    fn brakes_very_hot_for_two_seconds_once_until_they_cooled_and_not_within_three_minutes() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        pages.tyres([85.0; 4]);
        pages.leave_the_box(&mut e);
        pages.brakes([500.0, 820.0, 300.0, 300.0]);
        assert!(pages.drive(&mut e, 38).is_empty(), "1.85 s");
        let said = pages.drive(&mut e, 4);
        assert_eq!(keys(&said), ["brakes_hot"]);
        assert_eq!(said[0].group, Group::Car);
        assert!(pages.drive(&mut e, 400).is_empty(), "still hot");
        pages.brakes([750.0; 4]);
        pages.drive(&mut e, 20);
        pages.brakes([850.0; 4]);
        assert!(pages.drive(&mut e, 100).is_empty(), "not below 700 °C in between");
        pages.brakes([600.0; 4]);
        pages.drive(&mut e, 20);
        pages.brakes([850.0; 4]);
        assert!(pages.drive(&mut e, 100).is_empty(), "cooled, but within three minutes");
        pages.brakes([600.0; 4]);
        let t0 = pages.said[0].call.at;
        while pages.t < t0 + 178.0 {
            pages.step(&mut e);
        }
        pages.brakes([850.0; 4]);
        let said = pages.drive(&mut e, 80);
        assert_eq!(keys(&said), ["brakes_hot"]);
        assert!(said[0].call.at >= t0 + 179.99);
        // hot for five minutes without cooling: once
        let (mut e, mut pages) = (engineer(), Pages::new());
        pages.tyres([85.0; 4]);
        pages.leave_the_box(&mut e);
        pages.brakes([850.0; 4]);
        pages.drive(&mut e, 300 * 20);
        assert_eq!(keys(&pages.said), ["brakes_hot"]);
        // a cool sample in between does not count
        let (mut e, mut pages) = (engineer(), Pages::new());
        pages.tyres([85.0; 4]);
        pages.leave_the_box(&mut e);
        for _ in 0..10 {
            pages.brakes([850.0; 4]);
            pages.drive(&mut e, 30);
            pages.brakes([790.0; 4]);
            pages.step(&mut e);
        }
        assert!(pages.said.is_empty());
    }

    #[test]
    fn tyres_in_the_window_once_a_stint_after_they_were_cold() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        pages.leave_the_box(&mut e);
        let mut said = Vec::new();
        for c in (40..=80).step_by(5) {
            pages.tyres([c as f32, c as f32 + 2.0, c as f32 - 3.0, c as f32]);
            said.extend(pages.drive(&mut e, 20));
        }
        assert_eq!(keys(&said), ["tyres_in_window"]);
        assert_eq!(texts(&said), ["Reifen sind im Fenster."]);
        assert!(said[0].text.is_none() && said[0].group == Group::Car);
        pages.tyres([60.0; 4]);
        pages.drive(&mut e, 20);
        pages.tyres([85.0; 4]);
        assert!(pages.drive(&mut e, 20).is_empty(), "once a stint");
        // a short stop: warm tyres, no call
        pages.pit(true);
        pages.drive(&mut e, 40);
        pages.pit(false);
        assert!(pages.drive(&mut e, 200).is_empty());
        // a long stop: cold again
        pages.pit(true);
        pages.tyres([50.0; 4]);
        pages.drive(&mut e, 40);
        pages.pit(false);
        pages.drive(&mut e, 40);
        pages.tyres([75.0, 75.0, 75.0, 104.0]);
        assert!(pages.drive(&mut e, 20).is_empty(), "one tyre above the window");
        pages.tyres([75.0; 4]);
        assert_eq!(keys(&pages.drive(&mut e, 20)), ["tyres_in_window"]);
    }

    #[test]
    fn speaks_english_too() {
        let (mut e, mut pages) = (Engineer::new(Config::default(), "en"), Pages::new());
        pages.fuel(12.0);
        on_a_flying_lap(&mut e, &mut pages);
        let said = pages.lap(&mut e, 4.0, 492_400);
        assert_eq!(keys(&said), ["lap_time", "fuel_low", "fuel_laps"]);
        assert_eq!(said[0].text.as_deref(), Some("Eight twelve four."));
        assert_eq!(said[1].text, None);
        assert_eq!(said[2].text.as_deref(), Some("Fuel for one more lap."));
        assert_eq!(texts(&pages.lap(&mut e, 0.0, 491_000))[0], "New personal best, eight eleven zero.");
    }

    #[test]
    fn ignores_pages_of_another_size_and_samples_outside_live_driving() {
        let (mut e, mut pages) = (engineer(), Pages::new());
        assert!(e.step(0.0, &pages.physics[..799], &pages.graphics).is_empty());
        assert!(e.step(0.0, &pages.physics, &pages.graphics[..4899]).is_empty());
        assert!(e.step(0.0, &[], &[]).is_empty());
        let mut long = pages.physics.clone();
        long.push(0);
        assert!(e.step(0.0, &long, &pages.graphics).is_empty());
        // paused with hot brakes on track: nothing, and the time paused does not count
        pages.tyres([85.0; 4]);
        pages.leave_the_box(&mut e);
        pages.brakes([900.0; 4]);
        pages.drive(&mut e, 30);
        pages.i32(laps::STATUS, 3);
        assert!(pages.drive(&mut e, 100).is_empty());
        pages.i32(laps::STATUS, laps::LIVE);
        assert!(pages.drive(&mut e, 38).is_empty(), "the 1.5 s before the pause do not count");
        assert_eq!(keys(&pages.drive(&mut e, 4)), ["brakes_hot"]);
    }
}
