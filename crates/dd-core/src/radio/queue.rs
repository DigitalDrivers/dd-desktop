//! What the radio says next: every call waits in one queue by its priority (research
//! 2026-11-09-boxenfunk/A-evo-spotter-data.md §3, "Speech priority queue"), and the shell asks every tick whether to
//! play one now, cut the running one off for it, or wait. The spotter's calls come from [`call`]; the engineer's calls
//! (fuel, lap times) are pushed into the same queue with their own keys and [`Priority::Engineer`].
//!
//! The rules:
//! - Spotter and safety calls cut off a running call of lower priority mid-word; the call cut off comes back if it is
//!   still valid. They wait for a running call of their own priority or higher, so two spotter calls keep their order.
//! - Tactical, engineer and chatter calls play only when no car was alongside for 2 s, and 2 s after the last
//!   non-spotter call ended (safety calls do not wait for that gap: they are as urgent as the spotter's).
//! - Engineer and chatter calls wait for a straight (low yaw rate, full throttle, no brake), but not longer than
//!   [`STRAIGHT_WAIT_S`]: a track without a straight still hears them.
//! - A key can have a cooldown ([`Queue::cooldowns`]); a call of it waits that long after the last one of it started.
//!   The spotter's per-car rate limits live in the spotter itself, so none is set by default.
//! - Every call expires; an expired one is dropped unspoken.

use std::collections::HashMap;

use super::frame::{CarId, Own};
use super::spotter::Event;

/// P0 to P4 of research §3; the first is the most urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Overlaps, three wide, clear
    Spotter,
    /// Closing fast, slow car ahead
    Safety,
    /// Close behind, slipstream, gaps
    Tactical,
    /// Fuel, lap times, tyres, limiter
    Engineer,
    /// Chatter, briefing lines
    Chatter,
}

/// One thing to say. The key names the phrase in the voice library (`&'static str`: the library's phrases are a
/// fixed list, the engineer's keys too). With a car the shell plays the phrase's named variant when it knows the
/// driver.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub key: &'static str,
    pub car: Option<CarId>,
    pub priority: Priority,
    /// When it was made, seconds on the frames' clock
    pub at: f64,
    /// It is dropped unspoken from this time on
    pub expires: f64,
}

/// How long an overlap call stays worth saying: a "car left" a second late is wrong already.
pub const OVERLAP_EXPIRES_S: f64 = 1.0;
/// A "clear" still helps a little later.
pub const CLEAR_EXPIRES_S: f64 = 2.0;
pub const SAFETY_EXPIRES_S: f64 = 3.0;
pub const TACTICAL_EXPIRES_S: f64 = 5.0;

/// The phrase key, priority and expiry of a spotter event.
pub fn call(event: &Event, t: f64) -> Call {
    use Priority::*;
    let (key, car, priority, life) = match *event {
        Event::CarLeft { car } => ("car_left", Some(car), Spotter, OVERLAP_EXPIRES_S),
        Event::CarRight { car } => ("car_right", Some(car), Spotter, OVERLAP_EXPIRES_S),
        Event::ThreeWide => ("three_wide", None, Spotter, OVERLAP_EXPIRES_S),
        Event::ThreeWideOnLeft => ("three_wide_left", None, Spotter, OVERLAP_EXPIRES_S),
        Event::ThreeWideOnRight => ("three_wide_right", None, Spotter, OVERLAP_EXPIRES_S),
        Event::StillThere { .. } => ("still_there", None, Spotter, OVERLAP_EXPIRES_S),
        Event::ClearLeft => ("clear_left", None, Spotter, CLEAR_EXPIRES_S),
        Event::ClearRight => ("clear_right", None, Spotter, CLEAR_EXPIRES_S),
        Event::ClearAll => ("clear", None, Spotter, CLEAR_EXPIRES_S),
        Event::ClosingFast { car, .. } => ("closing_fast", Some(car), Safety, SAFETY_EXPIRES_S),
        Event::SlowCarAhead { car } => ("slow_car_ahead", Some(car), Safety, SAFETY_EXPIRES_S),
        Event::CloseBehind { car, .. } => ("close_behind", Some(car), Tactical, TACTICAL_EXPIRES_S),
        Event::SlipstreamBehind { car } => ("slipstream_behind", Some(car), Tactical, TACTICAL_EXPIRES_S),
        Event::InTow { car } => ("in_tow", Some(car), Tactical, TACTICAL_EXPIRES_S),
    };
    Call { key, car, priority, at: t, expires: t + life }
}

/// Tactical, engineer and chatter calls wait this long after the last overlap ...
pub const QUIET_AFTER_OVERLAP_S: f64 = 2.0;
/// ... and this long after the last non-spotter call ended.
pub const GAP_S: f64 = 2.0;
/// Engineer and chatter calls stop waiting for a straight after this.
pub const STRAIGHT_WAIT_S: f64 = 10.0;
/// A straight: |yaw rate| below this (rad/s, the spotter's slipstream threshold), throttle above, brake below
const STRAIGHT_YAW_RATE: f32 = 0.05;
const STRAIGHT_THROTTLE: f32 = 0.9;
const STRAIGHT_BRAKE: f32 = 0.05;

/// What the queue needs to know about the moment besides what is playing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Situation {
    /// A car is alongside ([`super::spotter::Spotter::overlap`])
    pub overlap: bool,
    /// The car is on a straight: low yaw rate, full throttle, no brake
    pub straight: bool,
}

impl Situation {
    pub fn new(own: &Own, overlap: bool) -> Self {
        Self { overlap, straight: own.yaw_rate.abs() < STRAIGHT_YAW_RATE && own.throttle > STRAIGHT_THROTTLE && own.brake < STRAIGHT_BRAKE }
    }
}

/// The queue's answer for this tick.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Nothing plays: start this
    Play(Call),
    /// Stop the running call mid-word and start this
    Interrupt(Call),
    Nothing,
}

#[derive(Debug)]
pub struct Queue {
    /// Waiting calls, in the order they go: priority, then time, then arrival
    waiting: Vec<(Call, u64)>,
    arrivals: u64,
    last_overlap: f64,
    /// The last tick a non-spotter call was playing (or was handed out)
    last_busy: f64,
    started: HashMap<&'static str, f64>,
    /// Seconds a key waits after a call of it started, by key
    pub cooldowns: HashMap<&'static str, f64>,
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    pub fn new() -> Self {
        Self { waiting: Vec::new(), arrivals: 0, last_overlap: f64::NEG_INFINITY, last_busy: f64::NEG_INFINITY, started: HashMap::new(), cooldowns: HashMap::new() }
    }

    pub fn push(&mut self, call: Call) {
        self.arrivals += 1;
        let entry = (call, self.arrivals);
        let at = self.waiting.partition_point(|(c, n)| (c.priority, c.at, *n) <= (entry.0.priority, entry.0.at, entry.1));
        self.waiting.insert(at, entry);
    }

    /// The calls waiting, in the order they would go.
    pub fn waiting(&self) -> impl Iterator<Item = &Call> {
        self.waiting.iter().map(|(c, _)| c)
    }

    /// Called every tick with the time, the call the shell is playing (as the queue handed it out) and the
    /// situation.
    pub fn next(&mut self, now: f64, playing: Option<&Call>, situation: Situation) -> Next {
        self.waiting.retain(|(c, _)| c.expires > now);
        if situation.overlap {
            self.last_overlap = now;
        }
        if playing.is_some_and(|p| p.priority != Priority::Spotter) {
            self.last_busy = now;
        }
        let ready = |c: &Call| -> Option<bool> {
            let cooled = self.started.get(c.key).is_none_or(|last| now - last >= self.cooldowns.get(c.key).copied().unwrap_or(0.0));
            if !cooled {
                return None;
            }
            match playing {
                Some(p) => (c.priority <= Priority::Safety && c.priority < p.priority).then_some(true),
                None if c.priority <= Priority::Safety => Some(false),
                None => {
                    let quiet = now - self.last_overlap >= QUIET_AFTER_OVERLAP_S && now - self.last_busy >= GAP_S;
                    let straight = c.priority == Priority::Tactical || situation.straight || now - c.at >= STRAIGHT_WAIT_S;
                    (quiet && straight).then_some(false)
                }
            }
        };
        let Some((i, interrupt)) = self.waiting.iter().enumerate().find_map(|(i, (c, _))| ready(c).map(|interrupt| (i, interrupt))) else {
            return Next::Nothing;
        };
        let (call, _) = self.waiting.remove(i);
        self.started.insert(call.key, now);
        if call.priority != Priority::Spotter {
            self.last_busy = now;
        }
        // A car the spotter calls alongside is there: a warning about it that still waits is over.
        if call.priority == Priority::Spotter && call.car.is_some() {
            self.waiting.retain(|(c, _)| c.car != call.car);
        }
        if interrupt {
            // What was cut off comes back unless it is over: a warning is about a moment, the engineer's call is not.
            if let Some(cut) = playing.filter(|p| p.priority >= Priority::Tactical && p.expires > now) {
                self.push(cut.clone());
            }
            Next::Interrupt(call)
        } else {
            Next::Play(call)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::spotter::Side;

    const CALM: Situation = Situation { overlap: false, straight: true };
    const ALONGSIDE: Situation = Situation { overlap: true, straight: true };
    const CORNER: Situation = Situation { overlap: false, straight: false };

    fn engineer(key: &'static str, at: f64) -> Call {
        Call { key, car: None, priority: Priority::Engineer, at, expires: at + 25.0 }
    }

    fn played(next: Next) -> Call {
        match next {
            Next::Play(call) => call,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_spotter_event_has_its_phrase_key_priority_and_expiry() {
        let car = (7, 8);
        let cases = [
            (Event::CarLeft { car }, "car_left", Some(car), Priority::Spotter, 1.0),
            (Event::CarRight { car }, "car_right", Some(car), Priority::Spotter, 1.0),
            (Event::ThreeWide, "three_wide", None, Priority::Spotter, 1.0),
            (Event::ThreeWideOnLeft, "three_wide_left", None, Priority::Spotter, 1.0),
            (Event::ThreeWideOnRight, "three_wide_right", None, Priority::Spotter, 1.0),
            (Event::StillThere { side: Side::Right }, "still_there", None, Priority::Spotter, 1.0),
            (Event::ClearLeft, "clear_left", None, Priority::Spotter, 2.0),
            (Event::ClearRight, "clear_right", None, Priority::Spotter, 2.0),
            (Event::ClearAll, "clear", None, Priority::Spotter, 2.0),
            (Event::ClosingFast { car, gap_s: 1.2 }, "closing_fast", Some(car), Priority::Safety, 3.0),
            (Event::SlowCarAhead { car }, "slow_car_ahead", Some(car), Priority::Safety, 3.0),
            (Event::CloseBehind { car, gap_s: 0.4 }, "close_behind", Some(car), Priority::Tactical, 5.0),
            (Event::SlipstreamBehind { car }, "slipstream_behind", Some(car), Priority::Tactical, 5.0),
            (Event::InTow { car }, "in_tow", Some(car), Priority::Tactical, 5.0),
        ];
        for (event, key, with, priority, life) in cases {
            assert_eq!(call(&event, 10.0), Call { key, car: with, priority, at: 10.0, expires: 10.0 + life }, "{event:?}");
        }
    }

    #[test]
    fn a_spotter_call_cuts_off_the_engineer_who_comes_back_once_it_is_quiet() {
        let mut queue = Queue::new();
        queue.push(engineer("fuel_laps", 0.0));
        let fuel = played(queue.next(0.0, None, CALM));
        queue.push(call(&Event::CarLeft { car: (0, 1) }, 1.0));
        let left = match queue.next(1.0, Some(&fuel), ALONGSIDE) {
            Next::Interrupt(call) => call,
            other => panic!("{other:?}"),
        };
        assert_eq!(left.key, "car_left");
        assert_eq!(queue.waiting().map(|c| c.key).collect::<Vec<_>>(), vec!["fuel_laps"], "back in the queue");
        assert_eq!(queue.next(1.5, Some(&left), ALONGSIDE), Next::Nothing);
        // alongside until 2.0; quiet from 4.0
        assert_eq!(queue.next(2.0, None, ALONGSIDE), Next::Nothing);
        assert_eq!(queue.next(3.9, None, CALM), Next::Nothing);
        assert_eq!(played(queue.next(4.0, None, CALM)), fuel);
    }

    #[test]
    fn an_engineer_call_cut_off_after_it_expired_does_not_come_back() {
        let mut queue = Queue::new();
        queue.push(Call { expires: 1.0, ..engineer("lap_time", 0.0) });
        let lap = played(queue.next(0.0, None, CALM));
        queue.push(call(&Event::ClosingFast { car: (0, 1), gap_s: 1.0 }, 1.0));
        assert!(matches!(queue.next(1.0, Some(&lap), CALM), Next::Interrupt(Call { key: "closing_fast", .. })));
        assert_eq!(queue.waiting().count(), 0);
    }

    #[test]
    fn a_warning_the_spotter_cut_off_does_not_come_back() {
        // heard live on 2026-11-09: "closing fast" came back 1.5 s late, after "car right" about the same car
        let mut queue = Queue::new();
        queue.push(call(&Event::ClosingFast { car: (0, 1), gap_s: 1.0 }, 0.0));
        let closing = played(queue.next(0.0, None, CALM));
        queue.push(call(&Event::CarRight { car: (0, 1) }, 0.3));
        assert!(matches!(queue.next(0.3, Some(&closing), ALONGSIDE), Next::Interrupt(Call { key: "car_right", .. })));
        assert_eq!(queue.waiting().count(), 0, "the car is alongside: the warning is over");
    }

    #[test]
    fn a_car_called_alongside_takes_what_waited_about_it_off_the_queue() {
        let mut queue = Queue::new();
        let clear = call(&Event::ClearLeft, 0.0);
        queue.push(call(&Event::ClosingFast { car: (0, 1), gap_s: 1.0 }, 0.1));
        queue.push(call(&Event::SlowCarAhead { car: (0, 2) }, 0.1));
        queue.push(call(&Event::CarRight { car: (0, 1) }, 0.2));
        assert_eq!(queue.next(0.2, Some(&clear), ALONGSIDE), Next::Nothing, "the spotter is speaking");
        assert!(matches!(queue.next(0.4, None, ALONGSIDE), Next::Play(Call { key: "car_right", .. })));
        assert_eq!(queue.waiting().map(|c| c.key).collect::<Vec<_>>(), ["slow_car_ahead"], "only the call about the other car waits on");
    }

    #[test]
    fn safety_cuts_off_the_engineer_but_waits_for_the_spotter() {
        let mut queue = Queue::new();
        let left = call(&Event::CarLeft { car: (0, 1) }, 0.0);
        queue.push(call(&Event::SlowCarAhead { car: (0, 2) }, 0.0));
        assert_eq!(queue.next(0.1, Some(&left), ALONGSIDE), Next::Nothing);
        assert!(matches!(queue.next(0.5, None, ALONGSIDE), Next::Play(Call { key: "slow_car_ahead", .. })));
    }

    #[test]
    fn expired_calls_are_dropped_unspoken() {
        let mut queue = Queue::new();
        let playing = call(&Event::ThreeWide, 0.0);
        queue.push(call(&Event::CarRight { car: (0, 1) }, 0.0));
        assert_eq!(queue.next(0.9, Some(&playing), ALONGSIDE), Next::Nothing);
        assert_eq!(queue.next(1.0, None, ALONGSIDE), Next::Nothing, "expired at 1.0");
        assert_eq!(queue.waiting().count(), 0);
    }

    #[test]
    fn tactical_calls_wait_for_two_seconds_without_a_car_alongside() {
        let mut queue = Queue::new();
        queue.push(call(&Event::CloseBehind { car: (0, 1), gap_s: 0.4 }, 0.0));
        assert_eq!(queue.next(0.0, None, ALONGSIDE), Next::Nothing);
        assert_eq!(queue.next(1.0, None, CORNER), Next::Nothing);
        assert!(matches!(queue.next(2.0, None, CORNER), Next::Play(Call { key: "close_behind", .. })), "no straight needed");
    }

    #[test]
    fn engineer_calls_wait_for_a_straight_but_not_for_ever() {
        let mut queue = Queue::new();
        queue.push(engineer("tyres_warm", 0.0));
        for t in 0..10 {
            assert_eq!(queue.next(t as f64, None, CORNER), Next::Nothing, "{t}");
        }
        assert!(matches!(queue.next(10.0, None, CORNER), Next::Play(Call { key: "tyres_warm", .. })), "after 10 s without a straight");

        let mut queue = Queue::new();
        queue.push(engineer("tyres_warm", 0.0));
        assert_eq!(queue.next(2.0, None, CORNER), Next::Nothing);
        assert!(matches!(queue.next(3.0, None, CALM), Next::Play(_)), "the first straight");
    }

    #[test]
    fn two_seconds_between_non_spotter_calls_and_a_cooldown_per_key() {
        let mut queue = Queue::new();
        queue.cooldowns.insert("fuel_laps", 60.0);
        queue.push(engineer("fuel_laps", 0.0));
        let fuel = played(queue.next(0.0, None, CALM));
        queue.push(engineer("lap_time", 1.0));
        queue.push(engineer("fuel_laps", 1.0));
        assert_eq!(queue.next(2.0, Some(&fuel), CALM), Next::Nothing, "playing");
        assert_eq!(queue.next(3.0, None, CALM), Next::Nothing, "fuel ended at 2.0: the gap");
        assert!(matches!(queue.next(4.0, None, CALM), Next::Play(Call { key: "lap_time", .. })), "the other key; fuel_laps cools down");
        for t in 7..=26 {
            assert_eq!(queue.next(t as f64, None, CALM), Next::Nothing, "{t}");
        }
        assert_eq!(queue.waiting().count(), 0, "the second fuel call expired at 26 before its cooldown ended at 60");
    }

    #[test]
    fn spotter_calls_in_a_row_keep_their_order_and_go_before_older_lower_ones() {
        let mut queue = Queue::new();
        queue.push(call(&Event::CloseBehind { car: (0, 3), gap_s: 0.5 }, 0.0));
        queue.push(call(&Event::ClearRight, 1.0));
        queue.push(call(&Event::CarLeft { car: (0, 1) }, 1.0));
        let first = played(queue.next(1.0, None, ALONGSIDE));
        assert_eq!(first.key, "clear_right");
        assert_eq!(queue.next(1.4, Some(&first), ALONGSIDE), Next::Nothing, "a spotter call does not cut off a spotter call");
        assert_eq!(played(queue.next(1.5, None, ALONGSIDE)).key, "car_left");
        assert_eq!(queue.waiting().map(|c| c.key).collect::<Vec<_>>(), vec!["close_behind"]);
    }

    #[test]
    fn the_situation_is_a_straight_with_low_yaw_full_throttle_and_no_brake() {
        let frames = crate::radio::testkit::Drive::new(crate::radio::testkit::straight(40.0, 0.0)).seconds(0.0).frames();
        let mut own = frames[0].own.clone();
        assert!(Situation::new(&own, false).straight);
        own.yaw_rate = -0.06;
        assert!(!Situation::new(&own, false).straight);
        own.yaw_rate = 0.0;
        own.throttle = 0.8;
        assert!(!Situation::new(&own, false).straight);
        own.throttle = 1.0;
        own.brake = 0.1;
        assert!(!Situation::new(&own, true).straight);
        assert!(Situation::new(&own, true).overlap);
    }
}
