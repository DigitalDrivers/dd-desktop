//! When a MoTeC file's data starts, on the PC's clock. The header names the local time the game wrote the file,
//! but the game writes it when the car is back in its box, and the data ends at the car's last lap completion
//! before that, which can be minutes earlier: measured on 70 of Joe's files (2026-10-13 to 10-15), the gap is
//! 9 to 525 s. The game's log has that moment to the millisecond (`Lap test evOnLapCompleted <n> completed`), and
//! the latest such line before the write, minus the file's duration, gives every file of a session the same start
//! within 15 ms (the car's placement in its pit box). Without the log, the header's time is taken as the end.
//! The header's time and the file's modification time (UTC) are the same moment, so they also give the PC's
//! offset from UTC, which turns the log's local times into those of the app's lap list.

/// What the game's log writes when the driver's car completes a lap.
const LAP_COMPLETED: &str = "evOnLapCompleted";
/// A lap completion longer before the write belongs to another session.
const MAX_GAP_MS: i64 = 6 * 60 * 60 * 1000;

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar (H. Hinnant).
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The PC's offset from UTC in seconds: the header's local time against the file's modification time (unix
/// seconds), rounded to a quarter of an hour.
pub fn utc_offset(written_local: i64, modified_unix: i64) -> i64 {
    ((written_local - modified_unix) as f64 / 900.0).round() as i64 * 900
}

/// The local time (ms since 1970, read as UTC) of a log line `[2026-10-15 22:57:02.935] ...`.
pub(crate) fn line_time(line: &str) -> Option<i64> {
    let stamp = line.strip_prefix('[')?.get(..23)?;
    let num = |a: usize, b: usize| stamp.get(a..b)?.parse::<i64>().ok();
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, m, s, ms) = (num(11, 13)?, num(14, 16)?, num(17, 19)?, num(20, 23)?);
    Some((days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + s) * 1000 + ms)
}

/// The last lap completion of the driver's car the log names at or before `until` (local ms), at most six hours
/// before it.
pub fn last_lap_completion(log: &str, until: i64) -> Option<i64> {
    log.lines()
        .filter(|line| line.contains(LAP_COMPLETED))
        .filter_map(line_time)
        .filter(|t| *t <= until && until - t <= MAX_GAP_MS)
        .max()
}

/// The clock of a MoTeC file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FileClock {
    /// The first sample, in unix ms
    pub start_ms: i64,
    /// The PC's offset from UTC, seconds
    pub offset_s: i64,
    /// Whether the end came from the game's log (otherwise from the header)
    pub from_log: bool,
    /// Which of the logs it came from
    pub log: Option<usize>,
}

/// The clock of a file written at `written_local` (the header) and `modified_ms` (unix ms) with `duration` seconds
/// of data, from the first of `logs` that names a lap completion before the write.
pub fn file_clock(written_local: i64, modified_ms: i64, duration: f64, logs: &[&str]) -> FileClock {
    let offset_s = utc_offset(written_local, modified_ms.div_euclid(1000));
    let until = written_local * 1000 + 999;
    let found = logs.iter().enumerate().find_map(|(i, log)| last_lap_completion(log, until).map(|t| (i, t)));
    let duration_ms = (duration * 1000.0).round() as i64;
    let end_ms = found.map_or(written_local * 1000, |(_, t)| t) - offset_s * 1000;
    FileClock { start_ms: end_ms - duration_ms, offset_s, from_log: found.is_some(), log: found.map(|(i, _)| i) }
}

/// ISO 8601 of a moment in the PC's local time: `2026-10-04T23:22:27+02:00`.
pub fn iso_local(unix_ms: i64, offset_s: i64) -> String {
    let secs = unix_ms.div_euclid(1000) + offset_s;
    let stamp = crate::stamp::utc_stamp(secs.max(0) as u64);
    let sign = if offset_s < 0 { '-' } else { '+' };
    let off = offset_s.abs();
    format!("{}{sign}{:02}:{:02}", stamp.trim_end_matches('Z'), off / 3600, off % 3600 / 60)
}

/// The moment (unix ms) of an ISO 8601 time with its offset as [`iso_local`] writes it: `2026-10-04T23:22:27+02:00`.
pub fn unix_ms(iso: &str) -> Option<i64> {
    let num = |a: usize, b: usize| iso.get(a..b)?.parse::<i64>().ok();
    if iso.len() != 25 || iso.as_bytes()[10] != b'T' {
        return None;
    }
    let local = days_from_civil(num(0, 4)?, num(5, 7)?, num(8, 10)?) * 86_400 + num(11, 13)? * 3600 + num(14, 16)? * 60 + num(17, 19)?;
    let offset = num(20, 22)? * 3600 + num(23, 25)? * 60;
    let offset = match &iso[19..20] {
        "+" => offset,
        "-" => -offset,
        _ => return None,
    };
    Some((local - offset) * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Joe's game log of 2026-10-15 evening (excerpt): the GT2 RS MR base session, placed in its box at
    /// 22:55:34.511, its three lap completions, and the next session's first one.
    const LOG: &str = "\
[2026-10-15 22:55:34.511] [gameplay] [info] [VehicleSystem][459b9d8284b077e5-9f8ba578ddcb83bd] TeleportCarCommand: Slot: 2\t Pos: x: 4028.69971 y: 569.944641 z: 4847.63281\r
[2026-10-15 22:57:02.935] [physics] [info] Lap test evOnLapCompleted 1 completed\r
[2026-10-15 22:57:12.534] [gameplay] [info] Car 459b9d8284b077e5-9f8ba578ddcb83bd exited to pitlane\r
[2026-10-15 22:58:14.344] [physics] [info] Lap test evOnLapCompleted 2 completed\r
[2026-10-15 23:07:26.274] [physics] [info] Lap test evOnLapCompleted 3 completed\r
[2026-10-15 23:27:00.606] [physics] [info] Lap test evOnLapCompleted 1 completed\r
";

    /// The base session's three files: header time, modification time (unix s), SPEED samples at 60 Hz.
    const FILES: [(&str, f64, u32); 3] =
        [("22:57:45", 1792097865.983389, 5305), ("23:06:34", 1792098394.9682093, 9589), ("23:15:28", 1792098928.416851, 42705)];

    fn local(hms: &str) -> i64 {
        let p: Vec<i64> = hms.split(':').map(|x| x.parse().unwrap()).collect();
        days_from_civil(2026, 10, 15) * 86_400 + p[0] * 3600 + p[1] * 60 + p[2]
    }

    #[test]
    fn every_file_of_a_session_starts_when_the_car_was_placed_in_its_box() {
        // 22:55:34.511 CEST
        let placed = 1_792_097_734_511i64;
        for (time, modified, samples) in FILES {
            let clock = file_clock(local(time), (modified * 1000.0) as i64, samples as f64 / 60.0, &[LOG]);
            assert_eq!((clock.offset_s, clock.from_log), (7200, true));
            assert!((clock.start_ms - placed).abs() <= 20, "{time}: {}", clock.start_ms - placed);
        }
    }

    #[test]
    fn without_the_log_the_header_s_time_is_the_end() {
        let (time, modified, samples) = FILES[2];
        let clock = file_clock(local(time), (modified * 1000.0) as i64, samples as f64 / 60.0, &["[2026-10-15 22:00:00.000] nothing\n"]);
        assert!(!clock.from_log);
        assert_eq!(clock.start_ms, (local(time) - 7200) * 1000 - 711_750);
        // a completion more than six hours before the write belongs to another session
        let old = "[2026-10-15 17:15:27.000] [physics] [info] Lap test evOnLapCompleted 1 completed\n";
        assert!(!file_clock(local(time), (modified * 1000.0) as i64, 711.75, &[old]).from_log);
        // the first log that has one counts
        assert_eq!(file_clock(local(time), (modified * 1000.0) as i64, 711.75, &["", LOG]).log, Some(1));
    }

    #[test]
    fn reads_the_offset_and_writes_local_times() {
        assert_eq!(utc_offset(local("23:15:28"), 1792098928), 7200);
        assert_eq!(utc_offset(1000 - 3600, 1000), -3600);
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(iso_local(1_792_097_734_511, 7200), "2026-10-15T22:55:34+02:00");
        assert_eq!(iso_local(1_792_097_734_511, -5400), "2026-10-15T19:25:34-01:30");
        assert_eq!(line_time("[2026-10-15 22:57:02.935] x"), Some(local("22:57:02") * 1000 + 935));
        assert_eq!(line_time("2026-10-15 22:57:02.935"), None);
        // and reads them back
        assert_eq!(unix_ms("2026-10-15T22:55:34+02:00"), Some(1_792_097_734_000));
        assert_eq!(unix_ms("2026-10-15T19:25:34-01:30"), Some(1_792_097_734_000));
        assert_eq!(unix_ms(&iso_local(1_793_025_378_000, 3600)), Some(1_793_025_378_000));
        assert_eq!(unix_ms("2026-10-15 22:55:34"), None);
    }
}
