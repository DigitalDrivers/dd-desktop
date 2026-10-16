//! Laps from the game's log, for sessions driven without the app's lap list (before the engineer's switch existed,
//! or with it off). The log names, with its local time to the millisecond:
//! - each lap completion of the driver's car on the timing line (`[physics] [info] Lap test evOnLapCompleted <n>
//!   completed`); on a circuit two of them in a row are a lap;
//! - on Touristenfahrten the start and end split of a timed lap (`Received split: isStart true, isEnd false` and
//!   `isStart false, isEnd true`): the lap the game times, while from completion to completion the car always went
//!   through its pit box (the completion is at the pit exit);
//! - the car selected (`Set new car <guid> content\cars\<id>\presets\<preset>`) and the track
//!   (`Creating physics track: <name>`).
//!
//! The split lines are written about 0.1 s after the moment (on Joe's GP laps of 2026-10-15 the split of the line
//! comes 109–128 ms after the completion); both ends of a lap move alike, so the lap time stays.

use super::clock::line_time;

const COMPLETED: &str = "evOnLapCompleted";
const START_SPLIT: &str = "Received split: isStart true, isEnd false";
const END_SPLIT: &str = "Received split: isStart false, isEnd true";
const TRACK: &str = "Creating physics track: ";

/// A lap in local ms (read as UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLap {
    pub start: i64,
    pub end: i64,
}

/// The laps the log names within [from, to] (local ms): the start-to-end split pairs where there are any, otherwise
/// the stretches between lap completions, the first from `from` (the file's start) to the first completion.
pub fn laps(log: &str, from: i64, to: i64) -> Vec<LogLap> {
    let events: Vec<(i64, &str)> = log
        .lines()
        .filter_map(|line| {
            let kind = [COMPLETED, START_SPLIT, END_SPLIT].into_iter().find(|k| line.contains(k))?;
            Some((line_time(line)?, kind)).filter(|(t, _)| *t >= from && *t <= to)
        })
        .collect();
    let mut split_laps = Vec::new();
    let mut started: Option<i64> = None;
    for (t, kind) in &events {
        match *kind {
            START_SPLIT => started = Some(*t),
            END_SPLIT => {
                if let Some(start) = started.take() {
                    split_laps.push(LogLap { start, end: *t });
                }
            }
            _ => {}
        }
    }
    if !split_laps.is_empty() {
        return split_laps;
    }
    let mut previous = from;
    let mut out = Vec::new();
    for (t, kind) in events {
        if kind == COMPLETED && t > previous {
            out.push(LogLap { start: previous, end: t });
            previous = t;
        }
    }
    out
}

/// The lines of the log up to `until` (local ms).
fn until<'a>(log: &'a str, until: i64) -> impl Iterator<Item = &'a str> {
    log.lines().take_while(move |line| line_time(line).is_none_or(|t| t <= until))
}

/// The car (id, preset) the log selected last up to `until`, of `preset` when given.
pub fn car(log: &str, at: i64, preset: Option<&str>) -> Option<(String, String)> {
    until(log, at).filter_map(crate::laps::current_car).filter(|(_, p)| preset.is_none_or(|preset| p == preset)).last()
}

/// The track the log loaded last up to `until`, as the shared memory names it (`Nurburgring`).
pub fn track(log: &str, at: i64) -> Option<String> {
    until(log, at).filter_map(|line| line.split_once(TRACK)).map(|(_, name)| name.trim().to_string()).filter(|name| !name.is_empty()).last()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Joe's log of 2026-10-15 (excerpts): a GP stint with three laps, and a Touristenfahrten session.
    const GP: &str = "\
[2026-10-15 17:02:50.046] [gameplay] [info] ACEVO-2629 onSetPlayerCurrentCarCommand: Set new car 4a642ae8-6590-72bd-c652-500a179082ab content\\cars\\dd_bmw_m3_e46_rennsport\\presets\\preset_dd_bmw_m3_e46_rennsport_stage1.mechanicalcarpreset\r
[2026-10-15 17:02:51.000] [physics] [info] Creating physics track: Nurburgring\r
[2026-10-15 17:09:06.023] [gameplay] [info] Received split: isStart false, isEnd false, index 0 \r
[2026-10-15 17:10:30.486] [physics] [info] Lap test evOnLapCompleted 1 completed\r
[2026-10-15 17:10:30.595] [gameplay] [info] Received split: isStart true, isEnd true, index 2 \r
[2026-10-15 17:12:35.280] [physics] [info] Lap test evOnLapCompleted 2 completed\r
[2026-10-15 17:12:35.389] [gameplay] [info] Received split: isStart true, isEnd true, index 2 \r
[2026-10-15 17:14:36.362] [physics] [info] Lap test evOnLapCompleted 3 completed\r
[2026-10-15 17:14:36.490] [gameplay] [info] Received split: isStart true, isEnd true, index 2 \r
";

    const TOURIST: &str = "\
[2026-10-15 22:55:34.511] [gameplay] [info] [VehicleSystem][459b9d8284b077e5-9f8ba578ddcb83bd] TeleportCarCommand: Slot: 2\r
[2026-10-15 22:57:02.935] [physics] [info] Lap test evOnLapCompleted 1 completed\r
[2026-10-15 22:57:20.320] [gameplay] [info] Received split: isStart true, isEnd false, index 0 \r
[2026-10-15 22:58:14.344] [physics] [info] Lap test evOnLapCompleted 2 completed\r
[2026-10-15 22:58:31.746] [gameplay] [info] Received split: isStart true, isEnd false, index 0 \r
[2026-10-15 23:05:26.210] [gameplay] [info] Received split: isStart false, isEnd true, index 1 \r
[2026-10-15 23:07:26.274] [physics] [info] Lap test evOnLapCompleted 3 completed\r
[2026-10-15 23:07:44.155] [gameplay] [info] Received split: isStart true, isEnd false, index 0 \r
[2026-10-15 23:14:36.466] [gameplay] [info] Received split: isStart false, isEnd true, index 1 \r
";

    fn ms(hms: &str) -> i64 {
        line_time(&format!("[2026-10-15 {hms}] x")).unwrap()
    }

    #[test]
    fn on_a_circuit_a_lap_runs_from_one_completion_to_the_next() {
        let found = laps(GP, ms("17:03:06.716"), ms("17:14:36.362"));
        let times: Vec<i64> = found.iter().map(|l| l.end - l.start).collect();
        // the first from the file's start (the box), then 2:04.794 and 2:01.082
        assert_eq!(times, [ms("17:10:30.486") - ms("17:03:06.716"), 124_794, 121_082]);
        // only what lies in the file
        assert_eq!(laps(GP, ms("17:03:06.716"), ms("17:12:35.280")).len(), 2);
    }

    #[test]
    fn on_touristenfahrten_a_lap_runs_from_the_start_split_to_the_end_split() {
        // the base session's third file: data from 22:55:34.5 to 23:07:26.274
        let found = laps(TOURIST, ms("22:55:34.524"), ms("23:07:26.274"));
        // a start split without its end (the car went back to its box) is no lap; the timed lap is 6:54.464
        // (the app's lap list had 6:54.447 for it)
        assert_eq!(found, [LogLap { start: ms("22:58:31.746"), end: ms("23:05:26.210") }]);
    }

    #[test]
    fn names_the_car_and_the_track_selected_before() {
        let at = ms("17:12:00.000");
        assert_eq!(car(GP, at, None), Some(("dd_bmw_m3_e46_rennsport".into(), "preset_dd_bmw_m3_e46_rennsport_stage1".into())));
        assert_eq!(car(GP, at, Some("preset_dd_bmw_m3_e46_rennsport_stage2")), None);
        assert_eq!(car(GP, ms("17:00:00.000"), None), None);
        assert_eq!(track(GP, at).as_deref(), Some("Nurburgring"));
        assert_eq!(track(TOURIST, at), None);
    }
}
