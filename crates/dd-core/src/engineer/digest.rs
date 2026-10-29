//! The coach's digest of each session: about 1 KB of what the session's best valid lap measured, kept for ever in
//! `<config>/engineer/digests.jsonl` (one per line, whatever the sessions' retention), and the input of the coach's
//! profile ([`super::profile`]). The types are the contract with the platform
//! (`research/2026-10-21-renningenieur-coach/contract-coach.ts`), which uploads the digests with the driver's `coach`
//! consent; each line also carries whether it was uploaded (`sent`) and when it was written (`writtenMs`, 0 for the
//! digests made once from the sessions stored before 0.26.0), for the coach's "new since you last looked".

use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use super::{round, store, EngineerLap, EngineerSession, EngineerTrace, Finding, TargetPsi};

pub const FILE: &str = "digests.jsonl";
/// The lap-level limits of the locked seconds' share of the braking time ([`super::measure::report`]).
const LOCK_FRONT: f64 = 0.03;
const LOCK_REAR: f64 = 0.005;
/// A pedal counts as pressed above this (%).
const PEDAL: f64 = 5.0;
const SLOW_KMH: f64 = 30.0;
/// `engineer_digests` hands out at most this many at once.
const PENDING_MAX: usize = 50;

/// Reading and writing the digests and the coach's state happen under this lock: the evaluation and the page's
/// commands run on different threads.
static LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachDigest {
    pub version: u32,
    /// The engineer session's id
    pub id: String,
    pub car: String,
    pub car_name: String,
    pub preset: Option<String>,
    pub track: String,
    pub layout: String,
    pub started_at: String,
    pub app_version: String,
    pub laps: u32,
    pub valid_laps: u32,
    /// The best valid lap; a digest is only written with one
    pub best_ms: i64,
    pub theoretical_best_ms: Option<i64>,
    /// The reference lap's time at the time of the evaluation
    pub reference_ms: Option<i64>,
    /// The best lap of this car, preset, track and layout before this session
    pub pb_before_ms: Option<i64>,
    /// Population standard deviation of the valid lap times, null with one lap
    pub spread_ms: Option<i64>,
    /// Of the best valid lap:
    pub coast_s: f64,
    /// Locked seconds of the axle, each corner's worse wheel, summed over the lap
    pub lock_front_s: f64,
    pub lock_rear_s: f64,
    /// The same as a share of the lap's braking time (brake above 5 % above 30 km/h); null when neither the report
    /// nor the curves give it
    pub lock_front_share: Option<f64>,
    pub lock_rear_share: Option<f64>,
    /// Spinning seconds of the worse rear wheel
    pub spin_s: f64,
    pub pressure_end: [f64; 4],
    pub target_psi: Option<TargetPsi>,
    /// Valid laps with all four end pressures inside their window, null when no valid lap has one
    pub laps_in_window: Option<u32>,
    pub brake_max_c: f64,
    pub fuel_start_l: f64,
    pub top_kmh: f64,
    /// The corners that cost the most against the reference, at most 5, the worst first
    pub top_losses: Vec<Loss>,
    /// The corners with the most coast / lock / spin seconds, at most 3 each
    pub coast_corners: Vec<u32>,
    pub lock_corners: Vec<u32>,
    pub spin_corners: Vec<u32>,
    /// The best lap's findings of the lap (not of its corners)
    pub findings: Vec<Finding>,
    pub headline: CoachHeadline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Loss {
    pub n: u32,
    pub dt_s: f64,
}

/// The one line the page says about a session (rendered from the platform's i18n).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CoachHeadline {
    /// The first session with this car on this track and layout
    FirstCar,
    /// The first with this preset; `best_ms` / `best_preset` the best earlier session's with the car on the track
    #[serde(rename_all = "camelCase")]
    FirstPreset { preset: Option<String>, best_ms: i64, best_preset: Option<String> },
    /// Faster than the best before
    #[serde(rename_all = "camelCase")]
    NewBest { delta_ms: i64, preset: Option<String> },
    /// At most 1 % slower than the best before
    #[serde(rename_all = "camelCase")]
    NearBest { delta_ms: i64, preset: Option<String> },
    /// More than 1 % slower
    #[serde(rename_all = "camelCase")]
    OffBest { delta_ms: i64, preset: Option<String> },
}

/// `engineer_digests`: the digests not yet uploaded, the oldest first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoachDigestsPending {
    pub digests: Vec<CoachDigest>,
}

/// A line of `digests.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Stored {
    #[serde(flatten)]
    pub digest: CoachDigest,
    #[serde(default)]
    pub sent: bool,
    #[serde(default)]
    pub written_ms: i64,
}

/// A session's start in unix seconds, from its id.
pub(crate) fn start_secs(id: &str) -> i64 {
    id.split_once('-').and_then(|(secs, _)| secs.parse().ok()).unwrap_or(0)
}

/// The lap's braking time from its curves (every 5 m), for reports of apps before 0.26.0, which do not carry it.
/// Like the report's, only above 30 km/h.
fn braking_from_trace(trace: &EngineerTrace) -> f64 {
    trace.brake.iter().zip(&trace.speed_kmh).filter(|(brake, kmh)| **brake > PEDAL && **kmh > SLOW_KMH).map(|(_, kmh)| trace.step_m / (kmh / 3.6)).sum()
}

/// The corners of `lap` with the most of `value`, at most 3, those with none left out.
fn most(lap: &EngineerLap, value: impl Fn(&super::CornerResult) -> f64) -> Vec<u32> {
    let mut corners: Vec<(u32, f64)> = lap.corners.iter().map(|c| (c.n, value(c))).collect();
    corners.sort_by(|a, b| b.1.total_cmp(&a.1));
    corners.into_iter().take(3).filter(|(_, v)| *v > 0.0).map(|(n, _)| n).collect()
}

/// The digest of a session (its laps as they count: a caller drops the laps another session holds), without the
/// best before and the headline ([`judge`]); None without a valid lap. `traces` give the braking time where the
/// report has none.
pub fn digest(s: &EngineerSession, traces: Option<&[EngineerTrace]>) -> Option<CoachDigest> {
    let valid: Vec<&EngineerLap> = s.laps.iter().filter(|l| l.valid && l.lap_time_ms.is_some()).collect();
    let best = *valid.iter().min_by_key(|l| l.lap_time_ms)?;
    let times: Vec<f64> = valid.iter().filter_map(|l| l.lap_time_ms).map(|t| t as f64).collect();
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    // a half to the even ms, as the fixture (Python) rounds: two laps an odd number of ms apart give one
    let spread = (times.len() > 1).then(|| (times.iter().map(|t| (t - mean).powi(2)).sum::<f64>() / times.len() as f64).sqrt().round_ties_even() as i64);
    let worse = |wheels: [usize; 2], seconds: fn(&super::CornerResult) -> [f64; 4]| wheels.map(|w| best.corners.iter().map(|c| seconds(c)[w]).sum::<f64>()).into_iter().fold(0.0, f64::max);
    // locking: each corner's worse wheel of the axle, as the report judges it
    let locked = |[a, b]: [usize; 2]| best.corners.iter().map(|c| c.lock_s[a].max(c.lock_s[b])).sum::<f64>();
    let (lock_front, lock_rear) = (locked([0, 1]), locked([2, 3]));
    let braking = best.braking_s.or_else(|| traces?.iter().find(|t| t.lap_id == best.id).map(braking_from_trace)).filter(|b| *b > 0.0);
    let share = |locked: f64| braking.map(|b| round(locked / b, 4));
    let (front_share, rear_share) = (share(lock_front), share(lock_rear));
    let in_window = |l: &EngineerLap| l.tyres.target_psi.as_ref().is_some_and(|t| l.tyres.pressure_end.iter().all(|p| *p >= t.min && *p <= t.max));
    let mut losses: Vec<Loss> = best.corners.iter().filter_map(|c| c.dt_s.filter(|dt| *dt > 0.0).map(|dt_s| Loss { n: c.n, dt_s })).collect();
    losses.sort_by(|a, b| b.dt_s.total_cmp(&a.dt_s));
    losses.truncate(5);
    let mut findings: Vec<Finding> = best
        .findings
        .iter()
        .copied()
        .filter(|f| matches!(f, Finding::PressureHigh | Finding::PressureLow | Finding::BrakeHot | Finding::BrakeFade | Finding::FuelHeavy | Finding::Understeer | Finding::Oversteer))
        .collect();
    // locking from the shares, the same rule for the reports that judged it per corner
    findings.extend(front_share.filter(|s| *s > LOCK_FRONT).map(|_| Finding::LockFront));
    findings.extend(rear_share.filter(|s| *s > LOCK_REAR).map(|_| Finding::LockRear));
    findings.sort();
    Some(CoachDigest {
        version: 1,
        id: s.id.clone(),
        car: s.car.clone(),
        car_name: s.car_name.clone(),
        preset: s.preset.clone(),
        track: s.track.clone(),
        layout: s.layout.clone(),
        started_at: s.started_at.clone(),
        app_version: s.app_version.clone(),
        laps: s.laps.len() as u32,
        valid_laps: valid.len() as u32,
        best_ms: best.lap_time_ms?,
        theoretical_best_ms: best.theoretical_best_ms,
        reference_ms: s.reference.as_ref().map(|r| r.lap_time_ms),
        pb_before_ms: None,
        spread_ms: spread,
        coast_s: round(best.corners.iter().map(|c| c.coast_s).sum(), 2),
        lock_front_s: round(lock_front, 2),
        lock_rear_s: round(lock_rear, 2),
        lock_front_share: front_share,
        lock_rear_share: rear_share,
        spin_s: round(worse([2, 3], |c| c.spin_s), 2),
        pressure_end: best.tyres.pressure_end,
        target_psi: best.tyres.target_psi.clone(),
        laps_in_window: valid.iter().any(|l| l.tyres.target_psi.is_some()).then(|| valid.iter().filter(|l| in_window(l)).count() as u32),
        brake_max_c: best.brakes.temp_max.iter().copied().fold(0.0, f64::max),
        fuel_start_l: best.fuel.start_l,
        top_kmh: best.top_speed_kmh,
        top_losses: losses,
        coast_corners: most(best, |c| c.coast_s),
        lock_corners: most(best, |c| c.lock_s[0].max(c.lock_s[1])),
        spin_corners: most(best, |c| c.spin_s[2].max(c.spin_s[3])),
        findings,
        headline: CoachHeadline::FirstCar,
    })
}

/// Fills in the best before this session and the headline, from the digests of the sessions that started before it.
pub fn judge(d: &mut CoachDigest, others: &[&CoachDigest]) {
    let start = start_secs(&d.id);
    let earlier: Vec<&&CoachDigest> = others.iter().filter(|o| o.id != d.id && start_secs(&o.id) < start && o.car == d.car && o.track == d.track && o.layout == d.layout).collect();
    d.pb_before_ms = earlier.iter().filter(|o| o.preset == d.preset).map(|o| o.best_ms).min();
    d.headline = match d.pb_before_ms {
        Some(pb) => {
            let delta_ms = d.best_ms - pb;
            let preset = d.preset.clone();
            if delta_ms < 0 {
                CoachHeadline::NewBest { delta_ms, preset }
            } else if delta_ms as f64 <= pb as f64 * 0.01 {
                CoachHeadline::NearBest { delta_ms, preset }
            } else {
                CoachHeadline::OffBest { delta_ms, preset }
            }
        }
        None => match earlier.iter().min_by_key(|o| o.best_ms) {
            Some(best) => CoachHeadline::FirstPreset { preset: d.preset.clone(), best_ms: best.best_ms, best_preset: best.preset.clone() },
            None => CoachHeadline::FirstCar,
        },
    };
}

pub(crate) fn load(dir: &Path) -> Vec<Stored> {
    let text = fs::read_to_string(dir.join(FILE)).unwrap_or_default();
    let mut all: Vec<Stored> = text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
    all.sort_by(|a, b| start_secs(&a.digest.id).cmp(&start_secs(&b.digest.id)).then(a.digest.id.cmp(&b.digest.id)));
    all
}

fn save(dir: &Path, all: &[Stored]) -> io::Result<()> {
    let mut text = String::new();
    for stored in all {
        text += &serde_json::to_string(stored).map_err(io::Error::other)?;
        text.push('\n');
    }
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    fs::write(&tmp, text)?;
    fs::rename(tmp, dir.join(FILE))
}

/// The digests on this PC, the oldest first.
pub fn digests(dir: &Path) -> Vec<CoachDigest> {
    load(dir).into_iter().map(|s| s.digest).collect()
}

/// Writes the digest of an evaluated session at `now_ms`, in place of one of the same id (not yet uploaded then);
/// none without a valid lap (an earlier digest of the session stays then).
pub fn record(dir: &Path, session: &EngineerSession, now_ms: i64) -> io::Result<Option<CoachDigest>> {
    let _lock = lock();
    let traces = session.laps.iter().any(|l| l.braking_s.is_none()).then(|| store::traces(dir, &session.id)).flatten();
    let Some(mut d) = digest(session, traces.as_deref()) else { return Ok(None) };
    let mut all = load(dir);
    all.retain(|s| s.digest.id != d.id);
    judge(&mut d, &all.iter().map(|s| &s.digest).collect::<Vec<_>>());
    all.push(Stored { digest: d.clone(), sent: false, written_ms: now_ms });
    all.sort_by(|a, b| start_secs(&a.digest.id).cmp(&start_secs(&b.digest.id)).then(a.digest.id.cmp(&b.digest.id)));
    save(dir, &all)?;
    Ok(Some(d))
}

/// The digests of the sessions stored before the coach existed (`sessions`, the oldest first, each lap once:
/// [`store::physical`]); a session that has a digest keeps it. Called by [`store::upgrade`] under the lock. Answers
/// how many it wrote.
pub(crate) fn backfill(dir: &Path, sessions: &[EngineerSession]) -> io::Result<usize> {
    let mut all = load(dir);
    let mut written = 0;
    for s in sessions {
        if !all.iter().any(|d| d.digest.id == s.id) {
            if let Some(mut d) = digest(s, store::traces(dir, &s.id).as_deref()) {
                judge(&mut d, &all.iter().map(|s| &s.digest).collect::<Vec<_>>());
                all.push(Stored { digest: d, sent: false, written_ms: 0 });
                written += 1;
            }
        }
    }
    all.sort_by(|a, b| start_secs(&a.digest.id).cmp(&start_secs(&b.digest.id)).then(a.digest.id.cmp(&b.digest.id)));
    save(dir, &all)?;
    Ok(written)
}

/// The digests not yet uploaded, the oldest first, at most 50.
pub fn pending(dir: &Path) -> io::Result<CoachDigestsPending> {
    store::upgrade(dir)?;
    let _lock = lock();
    Ok(CoachDigestsPending { digests: load(dir).into_iter().filter(|s| !s.sent).take(PENDING_MAX).map(|s| s.digest).collect() })
}

/// Marks the digests `ids` as uploaded.
pub fn mark_sent(dir: &Path, ids: &[String]) -> io::Result<()> {
    let _lock = lock();
    let mut all = load(dir);
    for s in all.iter_mut().filter(|s| ids.contains(&s.digest.id)) {
        s.sent = true;
    }
    save(dir, &all)
}

/// The driver opened the coach at `now_ms`: what was written until then is no longer new.
pub fn seen(dir: &Path, now_ms: i64) -> io::Result<()> {
    let _lock = lock();
    let mut state = store::coach_state(dir);
    state.seen_ms = now_ms;
    store::save_coach_state(dir, &state)
}

/// The digests written since the driver last opened the coach.
pub(crate) fn unseen(dir: &Path, all: &[Stored]) -> u32 {
    let seen = store::coach_state(dir).seen_ms;
    all.iter().filter(|s| s.written_ms > seen).count() as u32
}

#[cfg(test)]
mod tests {
    use super::super::clock;
    use super::super::store::{self, DIR};
    use super::super::{EngineerTrace, Fuel};
    use super::*;
    use serde_json::Value;

    fn temp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("dd-engineer-digest-{tag}-{}", std::process::id())).join(DIR)
    }

    /// The contract's example session under `id` (`<secs>-<car>`), its second lap once for each of `times`.
    fn session(id: &str, times: &[i64], preset: &str) -> EngineerSession {
        let mut s: EngineerSession = serde_json::from_str(include_str!("../../tests/fixtures/example-session.json")).unwrap();
        let lap = s.laps[1].clone();
        s.id = id.into();
        s.car = id.split_once('-').unwrap().1.into();
        s.preset = Some(preset.into());
        s.started_at = clock::iso_local(start_secs(id) * 1000, 3600);
        s.ended_at = clock::iso_local(start_secs(id) * 1000 + 1_200_000, 3600);
        s.laps = times.iter().enumerate().map(|(i, t)| EngineerLap { id: format!("{id}#{}", i + 1), n: i as u32 + 1, lap_time_ms: Some(*t), fuel: Fuel { start_l: 60.0 - i as f64 * 20.0, used_l: 20.0 }, ..lap.clone() }).collect();
        s
    }

    #[test]
    fn digests_the_best_valid_lap_of_a_session() {
        let mut s: EngineerSession = serde_json::from_str(include_str!("../../tests/fixtures/example-session.json")).unwrap();
        // make lap 1 the best, with its locking and coasting
        s.laps[1].lap_time_ms = Some(380_000);
        s.laps[0].findings.push(Finding::Understeer);
        // each corner's worse front wheel counts: FL 0.12 in T1, FR 0.2 in T2
        s.laps[0].corners[1].lock_s[1] = 0.2;
        let d = digest(&s, None).unwrap();
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["bestMs"], 375_780);
        assert_eq!((d.laps, d.valid_laps, d.spread_ms, d.theoretical_best_ms, d.reference_ms), (2, 2, Some(2110), Some(366_950), Some(368_283)));
        // each corner's worse front wheel: 0.32 s of 61.3 s braking; the worse rear wheel spun 0.15 s
        assert_eq!((d.lock_front_s, d.lock_rear_s, d.lock_front_share, d.lock_rear_share, d.spin_s), (0.32, 0.0, Some(0.0052), Some(0.0), 0.15));
        assert_eq!((d.coast_s, d.coast_corners.clone(), d.lock_corners.clone(), d.spin_corners.clone()), (0.12, vec![1], vec![2, 1], vec![2]));
        assert_eq!(d.top_losses, [Loss { n: 2, dt_s: 7.468 }, Loss { n: 1, dt_s: 0.029 }]);
        // the lap's own findings only (its corners' codes are left out); lap 1 ends inside the slick window, lap 2 not
        assert_eq!(d.findings, [Finding::BrakeHot, Finding::Understeer]);
        assert_eq!((d.laps_in_window, d.brake_max_c, d.fuel_start_l, d.top_kmh), (Some(1), 896.0, 28.15, 362.1));
        assert_eq!(json["headline"], serde_json::json!({ "code": "first_car" }));
        // without a valid lap, no digest
        s.laps.iter_mut().for_each(|l| l.valid = false);
        assert_eq!(digest(&s, None), None);
    }

    #[test]
    fn an_old_report_s_braking_time_comes_from_its_curves() {
        let mut s: EngineerSession = serde_json::from_str(include_str!("../../tests/fixtures/example-session.json")).unwrap();
        s.laps[1].lap_time_ms = Some(380_000);
        s.laps.iter_mut().for_each(|l| l.braking_s = None);
        assert_eq!(digest(&s, None).unwrap().lock_front_share, None);
        // 30 points of 5 m braking at 108 km/h: 5 s; 2 points under 5 %, 2 at 20 km/h (below 30 km/h)
        let trace = EngineerTrace { lap_id: s.laps[0].id.clone(), step_m: 5.0, speed_kmh: [vec![108.0; 32], vec![20.0; 2]].concat(), throttle: vec![0.0; 34], brake: [vec![50.0; 30], vec![4.0; 2], vec![50.0; 2]].concat(), steer_deg: vec![0.0; 34], gear: vec![3; 34] };
        let d = digest(&s, Some(&[trace])).unwrap();
        assert_eq!(d.lock_front_share, Some(0.024));
    }

    fn digest_at(id: &str, preset: &str, best_ms: i64) -> CoachDigest {
        digest(&session(id, &[best_ms], preset), None).unwrap()
    }

    #[test]
    fn says_first_car_first_preset_new_near_and_off_best() {
        let mut all: Vec<CoachDigest> = Vec::new();
        let mut next = |id: &str, preset: &str, best_ms: i64| {
            let mut d = digest_at(id, preset, best_ms);
            judge(&mut d, &all.iter().collect::<Vec<_>>());
            all.push(d.clone());
            (d.pb_before_ms, serde_json::to_value(&d.headline).unwrap())
        };
        assert_eq!(next("1000-dd_x", "base", 400_000), (None, serde_json::json!({ "code": "first_car" })));
        assert_eq!(next("2000-dd_x", "stage1", 390_000), (None, serde_json::json!({ "code": "first_preset", "preset": "stage1", "bestMs": 400_000, "bestPreset": "base" })));
        assert_eq!(next("3000-dd_x", "base", 398_500), (Some(400_000), serde_json::json!({ "code": "new_best", "deltaMs": -1500, "preset": "base" })));
        // within 1 % of the best before (398.5 s): up to 3,985 ms slower
        assert_eq!(next("4000-dd_x", "base", 402_485).1, serde_json::json!({ "code": "near_best", "deltaMs": 3985, "preset": "base" }));
        assert_eq!(next("5000-dd_x", "base", 402_486).1, serde_json::json!({ "code": "off_best", "deltaMs": 3986, "preset": "base" }));
        // another car is a first again; a session judged again does not count against itself
        assert_eq!(next("6000-dd_y", "base", 380_000).1["code"], "first_car");
        let mut again = digest_at("3000-dd_x", "base", 398_500);
        judge(&mut again, &all.iter().collect::<Vec<_>>());
        assert_eq!(again.pb_before_ms, Some(400_000));
    }

    #[test]
    fn the_headlines_of_joe_s_sessions_are_the_fixture_s() {
        // example-profile.json (profile_fixture.py on Joe's sessions): its 50 newest digests; the oldest, of the
        // GT2 RS MR, is not among them, so that car's are left out
        let profile: Value = serde_json::from_str(include_str!("../../tests/fixtures/example-profile.json")).unwrap();
        let mut recent: Vec<CoachDigest> = serde_json::from_value(profile["recent"].clone()).unwrap();
        recent.reverse();
        let mut checked = 0;
        for d in recent.iter().filter(|d| d.car != "dd_porsche_gt2rs_mr") {
            let mut again = d.clone();
            judge(&mut again, &recent.iter().collect::<Vec<_>>());
            assert_eq!((again.pb_before_ms, &again.headline), (d.pb_before_ms, &d.headline), "{}", d.id);
            checked += 1;
        }
        assert_eq!(checked, 23);
        // and the fixture reads as the contract's digests, every field
        let written = serde_json::to_value(&recent[0]).unwrap();
        let fixture = profile["recent"].as_array().unwrap().last().unwrap();
        let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(keys(&written), keys(fixture));
    }

    #[test]
    fn backfills_each_lap_once_and_keeps_the_digests_through_retention() {
        let dir = temp("backfill");
        // three overlapping files of one drive (2026-10-26 Clubsport), then the same lap time a day later
        let a = session("1793025378-dd_x", &[429_006, 436_868], "p");
        let mut b = session("1793025609-dd_x", &[436_868, 431_000], "p");
        b.laps[0].fuel = a.laps[1].fuel.clone();
        b.laps[0].fuel.used_l += 0.01;
        let mut c = session("1793026295-dd_x", &[429_006], "p");
        c.laps[0].fuel = a.laps[0].fuel.clone();
        let d = session("1793111778-dd_x", &[429_006], "p");
        for s in [&a, &b, &c, &d] {
            store::save_session(&dir, s, &[]).unwrap();
        }
        store::upgrade(&dir).unwrap();
        let all = digests(&dir);
        let laps: Vec<(&str, u32, i64)> = all.iter().map(|d| (d.id.as_str(), d.valid_laps, d.best_ms)).collect();
        // b keeps its own lap only; c held nothing of its own: no digest
        assert_eq!(laps, [("1793025378-dd_x", 2, 429_006), ("1793025609-dd_x", 1, 431_000), ("1793111778-dd_x", 1, 429_006)]);
        assert_eq!(all[2].headline, CoachHeadline::NearBest { delta_ms: 0, preset: Some("p".into()) });
        // made once, written before the coach existed: nothing new to the driver
        assert_eq!(unseen(&dir, &load(&dir)), 0);
        store::upgrade(&dir).unwrap();
        assert_eq!(digests(&dir).len(), 3);
        // a new session's digest; the sessions go after 30 days, the digests stay
        let e = session("1793200000-dd_x", &[428_000], "p");
        store::save_session(&dir, &e, &[]).unwrap();
        assert_eq!(record(&dir, &e, 1_793_201_000_000).unwrap().unwrap().headline, CoachHeadline::NewBest { delta_ms: -1006, preset: Some("p".into()) });
        let gone = store::apply_retention(&dir, 1_793_200_000_000 + 400 * 24 * 3_600_000, Some(30)).unwrap();
        assert_eq!(gone.len(), 5);
        assert!(store::heads(&dir).is_empty());
        assert_eq!(digests(&dir).len(), 4);
        store::forget(&dir).unwrap();
    }

    #[test]
    fn hands_out_the_digests_not_uploaded_and_counts_the_new_ones() {
        let dir = temp("pending");
        let ids = |p: CoachDigestsPending| p.digests.into_iter().map(|d| d.id).collect::<Vec<_>>();
        let s1 = session("1793200000-dd_x", &[428_000], "p");
        let s2 = session("1793100000-dd_x", &[429_000], "p");
        for (s, at) in [(&s1, 10), (&s2, 20)] {
            store::save_session(&dir, s, &[]).unwrap();
            record(&dir, s, at).unwrap();
        }
        // the oldest session first
        assert_eq!(ids(pending(&dir).unwrap()), [s2.id.clone(), s1.id.clone()]);
        assert_eq!(unseen(&dir, &load(&dir)), 2);
        mark_sent(&dir, std::slice::from_ref(&s2.id)).unwrap();
        assert_eq!(ids(pending(&dir).unwrap()), std::slice::from_ref(&s1.id));
        seen(&dir, 15).unwrap();
        assert_eq!(unseen(&dir, &load(&dir)), 1, "written at 20, seen at 15");
        // evaluated again: replaced, to upload again, new again
        record(&dir, &s2, 30).unwrap();
        assert_eq!(digests(&dir).len(), 2);
        assert_eq!(ids(pending(&dir).unwrap()), [s2.id.clone(), s1.id.clone()]);
        seen(&dir, 30).unwrap();
        assert_eq!(unseen(&dir, &load(&dir)), 0);
        // a line in the file is the contract's digest with the two of the PC
        let line = fs::read_to_string(dir.join(FILE)).unwrap().lines().next().unwrap().to_string();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!((value["id"].as_str(), value["sent"].as_bool(), value["writtenMs"].as_i64(), value["version"].as_i64()), (Some(s2.id.as_str()), Some(false), Some(30), Some(1)));
        store::forget(&dir).unwrap();
    }
}
