//! The spotter and the engineer on slices of the shared memory recordings of the test evening of 2026-11-09 (Brands
//! Hatch GP, Porsche 992 Cup, nine AI cars, offline races; research/2026-11-09-testabend), cut with
//! `examples/cut_recording.rs`. Each golden timeline is what Joe heard live that evening (the app's log,
//! `funk/funk-*.log`) and approved, less what was fixed since; times are the recording's seconds.
//!
//! The slices start where no call before them holds one in them back: the same calls come out of the whole
//! recording in these windows.

use std::fs::File;
use std::io::BufReader;

use dd_core::radio::engineer::{self, Engineer};
use dd_core::radio::queue::call;
use dd_core::radio::spotter::{Config, Spotter};
use dd_core::radio::frame;
use dd_core::recorder::{Reader, Record};
use flate2::read::GzDecoder;

/// A call may move this much (s) before the timeline counts as changed.
const SLACK_S: f64 = 0.25;

/// Every call of the spotter and the engineer over a fixture, in order, with the recording's time.
fn replay(name: &str) -> Vec<(f64, &'static str)> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let reader = Reader::new(GzDecoder::new(BufReader::new(File::open(&path).unwrap()))).unwrap();
    let mut spotter = Spotter::new(Config::default());
    let mut engineer = Engineer::new(engineer::Config::default(), "de");
    let mut calls = Vec::new();
    for record in reader {
        let Record::Frame { t_us, physics, graphics, .. } = record.unwrap() else { continue };
        let t = t_us as f64 / 1e6;
        calls.extend(engineer.step(t, &physics, &graphics).into_iter().map(|say| (t, say.call.key)));
        if let Some(frame) = frame::read(t, &physics, &graphics) {
            calls.extend(spotter.step(&frame).iter().map(|event| (t, call(event, t).key)));
        }
    }
    calls
}

fn check(name: &str, golden: &[(f64, &str)]) {
    let calls = replay(name);
    let keys = |timeline: &[(f64, &str)]| timeline.iter().map(|(_, key)| key.to_string()).collect::<Vec<_>>();
    assert_eq!(keys(&calls), keys(golden), "{name}: {calls:?}");
    for ((t, key), (golden_t, _)) in calls.iter().zip(golden) {
        assert!((t - golden_t).abs() <= SLACK_S, "{name}: {key} at {t:.2} s, golden {golden_t:.2} s");
    }
}

#[test]
fn a1_one_car_left_three_times_then_right_twice_then_left_again() {
    // 174240-a1-training-ki, Joe practising with one AI car on the checklist's "left, then right"; live the same
    // calls, plus a closing_fast at 881.5 s while he braked (fixed in 71bc36a)
    check(
        "radio-a1-174240-855-898.ddsm.gz",
        &[
            (858.10, "car_left"),
            (861.10, "still_there"),
            (862.05, "clear_left"),
            (863.55, "car_left"),
            (866.05, "clear_left"),
            (867.55, "car_left"),
            (869.35, "clear_left"),
            (872.15, "close_behind"),
            (882.30, "car_right"),
            (884.65, "clear_right"),
            (886.20, "car_right"),
            (887.55, "clear_right"),
            (890.80, "car_left"),
            (893.85, "still_there"),
            (894.35, "clear_left"),
        ],
    );
}

#[test]
fn a5_right_then_between_two_cars_and_one_closing_fast() {
    // 182929-a5-rennen-gegenprobe, the counter-test race; live also "three wide" at 170.2 s while Joe pushed the car
    // ahead in line (a car in line is no overlap since 38f0337) with the right one leaving, and "clear left" after it
    check(
        "radio-a5-182929-160-200.ddsm.gz",
        &[
            (165.35, "car_right"),
            (168.35, "still_there"),
            (169.05, "in_tow"),
            (171.15, "clear_right"),
            (173.60, "car_left"),
            (173.85, "close_behind"),
            (174.85, "three_wide"),
            (175.15, "clear_left"),
            (175.75, "clear_right"),
            (196.40, "closing_fast"),
        ],
    );
}

#[test]
fn regen_the_pit_lane_from_the_entry_to_a_clear_exit() {
    // 201152-regen-boxenausfahrt, the first stop: entry too fast, the auto limiter, fuel to the flag, the stop's
    // countdown from the control lock, the exit; live the same (funk-211705.log)
    check(
        "radio-regen-201152-228-292.ddsm.gz",
        &[
            (230.05, "pit_entry_speed"),
            (235.75, "limiter_on"),
            (235.75, "fuel_to_flag"),
            (254.30, "pit_stop_time"),
            (275.30, "pit_ten"),
            (280.30, "pit_five"),
            (285.30, "pit_go"),
            (289.40, "limiter_off"),
            (289.40, "pit_exit_clear"),
        ],
    );
}
