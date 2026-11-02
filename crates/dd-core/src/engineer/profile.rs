//! The coach's profile of the driver (`engineer_profile`), computed from the digests on this PC ([`super::digest`]):
//! per car, track and layout the best laps and six skills, and the three things most worth working on. The rules
//! are those of research/2026-10-21-renningenieur-coach/profile_fixture.py, whose output on Joe's sessions
//! (`example-profile.json`) both this crate's and the platform's tests read; the types are the contract
//! (`contract-coach.ts`). Coach v2 (0.27.0) puts the driver first: eight skills as per-cent values over all digests
//! whatever the car (`driver`, rules of research/2026-11-02-coach-v2/profile_fixture_v2.py and its
//! `example-profile-v2.json`, contract `contract-coach-v2.ts`), and the skill the driver works on (`focus.json`).

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::digest::{self, start_secs, CoachDigest};
use super::{clock, round, store};

/// `recent` holds at most this many digests.
const RECENT: usize = 50;
/// A trend within this share of the first half's mean is flat.
const FLAT: f64 = 0.05;
/// A skill's `where` counts the corners of the latest this many digests on the driver's most-driven track.
const WHERE_LATEST: usize = 5;
const FOCUS: &str = "focus.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachProfile {
    pub version: u32,
    /// ISO 8601, local wall clock
    pub generated_at: String,
    pub sessions: u32,
    pub laps: u32,
    /// Digests written since the driver last opened the coach
    pub unseen: u32,
    /// One per car × track × layout, the most recently driven first
    pub cars: Vec<CoachCar>,
    /// Newest first, at most 50
    pub recent: Vec<CoachDigest>,
    /// At most 3, the costliest first
    pub work_items: Vec<CoachWorkItem>,
    /// The driver over all digests, whatever the car
    pub driver: CoachDriver,
    /// Every digest, the oldest first, aligned with each driver skill's `history`
    pub history: Vec<CoachHistory>,
    /// The skill the driver accepted or chose, and since when
    pub focus: Option<SkillKey>,
    pub focus_since: Option<String>,
}

/// The general picture of the driver over all digests on this PC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachDriver {
    pub sessions: u32,
    /// Valid laps
    pub laps: u32,
    pub cars: u32,
    /// Track × layout
    pub tracks: u32,
    /// The first digest's start, empty without one
    pub since: String,
    /// coast, braking, traction, smooth, pace, consistency, clean, tyres
    pub skills: Vec<CoachDriverSkill>,
}

/// A skill of the driver: `history` per digest (null where not measured), `value` the median of the measured ones,
/// `trend` their second half against the first (null below 4, flat within 5 % of the first half's mean), `cars` the
/// median per car, the best first, `where` the corners with the most of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachDriverSkill {
    pub key: SkillKey,
    pub unit: Unit,
    pub lower_better: bool,
    pub history: Vec<Option<f64>>,
    pub value: Option<f64>,
    pub trend: Option<Trend>,
    pub cars: Vec<CoachDriverCar>,
    pub r#where: Option<CoachWhere>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachDriverCar {
    pub car: String,
    pub car_name: String,
    /// Sessions that measured the skill
    pub sessions: u32,
    pub value: f64,
}

/// The corners with the most of a skill over the latest digests on the driver's most-driven track, at most 3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachWhere {
    pub track: String,
    pub layout: String,
    pub corners: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachHistory {
    pub id: String,
    pub at: String,
    pub car: String,
    pub car_name: String,
    pub track: String,
    pub layout: String,
}

/// The skill the driver works on (`coach_focus_get`, `coach_focus_set`, `<config>/engineer/focus.json`), and when they
/// chose it (ISO 8601, local wall clock); both null until then.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachFocus {
    pub focus: Option<SkillKey>,
    pub focus_since: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachCar {
    pub car: String,
    pub car_name: String,
    pub track: String,
    pub layout: String,
    pub sessions: u32,
    pub laps: u32,
    pub best_ms: i64,
    pub best_session_id: String,
    pub best_preset: Option<String>,
    pub first_best_ms: i64,
    pub theoretical_best_ms: Option<i64>,
    pub last_at: String,
    /// Oldest first
    pub bests: Vec<CoachBest>,
    /// coast, braking, traction, pace, consistency, tyres
    pub skills: Vec<CoachSkill>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachBest {
    pub session_id: String,
    pub at: String,
    pub best_ms: i64,
    pub theoretical_best_ms: Option<i64>,
    pub preset: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillKey {
    Coast,
    Braking,
    Traction,
    /// The driver's skills only (coach v2)
    Smooth,
    Pace,
    Consistency,
    /// The driver's skills only (coach v2)
    Clean,
    Tyres,
}

impl SkillKey {
    /// A car × track's skills
    const ALL: [SkillKey; 6] = [SkillKey::Coast, SkillKey::Braking, SkillKey::Traction, SkillKey::Pace, SkillKey::Consistency, SkillKey::Tyres];
    /// The driver's skills, in Joe's order
    pub const DRIVER: [SkillKey; 8] =
        [SkillKey::Coast, SkillKey::Braking, SkillKey::Traction, SkillKey::Smooth, SkillKey::Pace, SkillKey::Consistency, SkillKey::Clean, SkillKey::Tyres];

    /// The driver's value of the skill in a session (digest version 2): per cent, smooth a count per corner.
    fn driver_value(self, d: &CoachDigest) -> Option<f64> {
        match self {
            SkillKey::Coast => Some(d.coast_pct),
            SkillKey::Braking => d.lock_pct,
            SkillKey::Traction => Some(d.spin_pct),
            SkillKey::Smooth => d.smooth_per_corner,
            SkillKey::Pace => d.pace_pct,
            SkillKey::Consistency => d.spread_pct,
            SkillKey::Clean => Some(d.clean_pct),
            SkillKey::Tyres => d.tyres_pct,
        }
    }

    fn lower_better(self) -> bool {
        !matches!(self, SkillKey::Clean | SkillKey::Tyres)
    }

    /// The corners of a digest with the most of the skill (coast, braking, traction and smooth only).
    fn corners(self, d: &CoachDigest) -> Option<&Vec<u32>> {
        match self {
            SkillKey::Coast => Some(&d.coast_corners),
            SkillKey::Braking => Some(&d.lock_corners),
            SkillKey::Traction => Some(&d.spin_corners),
            SkillKey::Smooth => Some(&d.smooth_corners),
            _ => None,
        }
    }

    fn unit(self) -> Unit {
        match self {
            SkillKey::Coast | SkillKey::Traction => Unit::Seconds,
            SkillKey::Smooth => Unit::Count,
            _ => Unit::Percent,
        }
    }

    /// The skill's value in a session; lower is better except for the tyres.
    fn value(self, d: &CoachDigest) -> Option<f64> {
        match self {
            SkillKey::Coast => Some(d.coast_s),
            // the front lock share of the braking time, in %
            SkillKey::Braking => d.lock_front_share.map(|s| round(s * 100.0, 2)),
            SkillKey::Traction => Some(d.spin_s),
            SkillKey::Pace => d.theoretical_best_ms.filter(|t| *t > 0).map(|t| round((d.best_ms as f64 / t as f64 - 1.0) * 100.0, 2)),
            SkillKey::Consistency => d.spread_ms.map(|s| round(s as f64 / d.best_ms as f64 * 100.0, 2)),
            SkillKey::Tyres => d.laps_in_window.map(|n| round(n as f64 / d.valid_laps as f64 * 100.0, 1)),
            SkillKey::Smooth | SkillKey::Clean => self.driver_value(d),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    #[serde(rename = "s")]
    Seconds,
    #[serde(rename = "%")]
    Percent,
    /// Smoothness: corrections and lifts per corner
    #[serde(rename = "n")]
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trend {
    Better,
    Worse,
    Flat,
}

/// A skill on one car × track: `value` the latest measured one, `score` 100 at the driver's best session and 0 at
/// the worst (null with fewer than 2), `trend` the second half of the sessions against the first (null with fewer
/// than 4), `history` per session, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachSkill {
    pub key: SkillKey,
    pub value: Option<f64>,
    pub unit: Unit,
    pub score: Option<i64>,
    pub trend: Option<Trend>,
    pub best: Option<f64>,
    pub worst: Option<f64>,
    pub history: Vec<Option<f64>>,
}

/// Something worth working on: the latest coast, braking or traction of a car × track against the driver's best on
/// that track with any car.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachWorkItem {
    pub metric: SkillKey,
    pub car: String,
    pub car_name: String,
    pub track: String,
    pub layout: String,
    pub value: f64,
    pub unit: Unit,
    pub best_value: f64,
    pub best_car: String,
    pub best_car_name: String,
    pub history: Vec<Option<f64>>,
    pub trend: Option<Trend>,
    /// The corners of the latest best lap with the most of it, at most 3
    pub r#where: Vec<u32>,
}

fn skill(key: SkillKey, rows: &[&CoachDigest]) -> CoachSkill {
    let history: Vec<Option<f64>> = rows.iter().map(|d| key.value(d)).collect();
    let values: Vec<f64> = history.iter().flatten().copied().collect();
    let higher_better = key == SkillKey::Tyres;
    let latest = values.last().copied();
    let (lo, hi) = (values.iter().copied().reduce(f64::min), values.iter().copied().reduce(f64::max));
    let score = match (latest, lo, hi) {
        (Some(latest), Some(lo), Some(hi)) if values.len() >= 2 => {
            Some(if hi == lo { 100 } else { ((if higher_better { latest - lo } else { hi - latest }) / (hi - lo) * 100.0).round_ties_even() as i64 })
        }
        _ => None,
    };
    let trend = (values.len() >= 4).then(|| {
        let (a, b) = values.split_at(values.len() / 2);
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let (ma, mb) = (mean(a), mean(b));
        if (mb - ma).abs() <= ma.abs() * FLAT {
            Trend::Flat
        } else if (mb > ma) == higher_better {
            Trend::Better
        } else {
            Trend::Worse
        }
    });
    let (best, worst) = if higher_better { (hi, lo) } else { (lo, hi) };
    CoachSkill { key, value: latest, unit: key.unit(), score, trend, best, worst, history }
}

/// The median of `values` (not empty), the mean of the middle two for an even count.
fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// The driver's trend of `values` (the measured ones, the oldest first): the mean of the second half against the
/// first's, flat within 5 % of it (or when the first's is 0); null below 4.
fn driver_trend(values: &[f64], lower_better: bool) -> Option<Trend> {
    if values.len() < 4 {
        return None;
    }
    let (a, b) = values.split_at(values.len() / 2);
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (ma, mb) = (mean(a), mean(b));
    if ma == 0.0 || ((mb - ma) / ma.abs()).abs() < FLAT {
        return Some(Trend::Flat);
    }
    Some(if (mb < ma) == lower_better { Trend::Better } else { Trend::Worse })
}

/// The driver's skill over `digests` (the oldest first); `track` the most-driven track and layout, for `where`.
fn driver_skill(key: SkillKey, digests: &[CoachDigest], track: Option<(&str, &str)>) -> CoachDriverSkill {
    let history: Vec<Option<f64>> = digests.iter().map(|d| key.driver_value(d)).collect();
    let values: Vec<f64> = history.iter().flatten().copied().collect();
    let lower_better = key.lower_better();
    // per car in the order first driven, each with its latest name
    let mut by_car: Vec<(&str, &str, Vec<f64>)> = Vec::new();
    for (d, v) in digests.iter().zip(&history) {
        let Some(v) = v else { continue };
        match by_car.iter_mut().find(|(car, _, _)| *car == d.car) {
            Some(entry) => {
                entry.1 = &d.car_name;
                entry.2.push(*v);
            }
            None => by_car.push((&d.car, &d.car_name, vec![*v])),
        }
    }
    // sorted by the median before rounding: two cars that round alike stay in the order of their real values
    let mut cars: Vec<CoachDriverCar> =
        by_car.into_iter().map(|(car, car_name, v)| CoachDriverCar { car: car.into(), car_name: car_name.into(), sessions: v.len() as u32, value: median(&v) }).collect();
    cars.sort_by(|a, b| if lower_better { a.value.total_cmp(&b.value) } else { b.value.total_cmp(&a.value) });
    cars.iter_mut().for_each(|c| c.value = round(c.value, 2));
    // where: over the latest digests on the most-driven track that measured it, the corners named most often, then
    // those named higher up, then the lower number
    let r#where = track.and_then(|(track, layout)| {
        let latest: Vec<&Vec<u32>> =
            digests.iter().rev().filter(|d| d.track == track && d.layout == layout && key.driver_value(d).is_some()).filter_map(|d| key.corners(d)).take(WHERE_LATEST).collect();
        let mut counted: Vec<(u32, usize, usize)> = Vec::new();
        for (rank, n) in latest.iter().flat_map(|corners| corners.iter().enumerate()) {
            match counted.iter_mut().find(|c| c.0 == *n) {
                Some(c) => (c.1, c.2) = (c.1 + 1, c.2 + rank),
                None => counted.push((*n, 1, rank)),
            }
        }
        counted.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
        let corners: Vec<u32> = counted.into_iter().take(3).map(|c| c.0).collect();
        (!corners.is_empty()).then(|| CoachWhere { track: track.into(), layout: layout.into(), corners })
    });
    CoachDriverSkill {
        key,
        unit: if key == SkillKey::Smooth { Unit::Count } else { Unit::Percent },
        lower_better,
        value: (!values.is_empty()).then(|| round(median(&values), 2)),
        trend: driver_trend(&values, lower_better),
        cars,
        r#where,
        history,
    }
}

/// The driver over all `digests` (the oldest first).
fn driver(digests: &[CoachDigest]) -> CoachDriver {
    let distinct = |key: &dyn Fn(&CoachDigest) -> (&str, &str)| {
        let mut seen: Vec<(&str, &str)> = digests.iter().map(key).collect();
        seen.sort();
        seen.dedup();
        seen.len() as u32
    };
    // the most-driven track × layout, the latest driven of equals
    let mut tracks: Vec<((&str, &str), usize)> = Vec::new();
    for d in digests.iter().rev() {
        match tracks.iter_mut().find(|(t, _)| *t == (d.track.as_str(), d.layout.as_str())) {
            Some(t) => t.1 += 1,
            None => tracks.push(((&d.track, &d.layout), 1)),
        }
    }
    let track = tracks.iter().rev().max_by_key(|t| t.1).map(|t| t.0);
    CoachDriver {
        sessions: digests.len() as u32,
        laps: digests.iter().map(|d| d.valid_laps).sum(),
        cars: distinct(&|d| (&d.car, "")),
        tracks: distinct(&|d| (&d.track, &d.layout)),
        since: digests.first().map(|d| d.started_at.clone()).unwrap_or_default(),
        skills: SkillKey::DRIVER.iter().map(|k| driver_skill(*k, digests, track)).collect(),
    }
}

/// The profile of `digests` (the oldest first), `unseen` of them new, generated at `generated_at`.
pub fn build(digests: &[CoachDigest], unseen: u32, generated_at: String) -> CoachProfile {
    // car × track × layout in the order first driven, each with its sessions the oldest first
    let mut groups: Vec<Vec<&CoachDigest>> = Vec::new();
    for d in digests {
        match groups.iter_mut().find(|g| g[0].car == d.car && g[0].track == d.track && g[0].layout == d.layout) {
            Some(g) => g.push(d),
            None => groups.push(vec![d]),
        }
    }
    let mut cars: Vec<(CoachCar, &Vec<&CoachDigest>)> = groups
        .iter()
        .map(|rows| {
            let (first, last) = (rows[0], rows[rows.len() - 1]);
            let best = rows.iter().min_by_key(|d| d.best_ms).unwrap();
            let car = CoachCar {
                car: first.car.clone(),
                car_name: last.car_name.clone(),
                track: first.track.clone(),
                layout: first.layout.clone(),
                sessions: rows.len() as u32,
                laps: rows.iter().map(|d| d.valid_laps).sum(),
                best_ms: best.best_ms,
                best_session_id: best.id.clone(),
                best_preset: best.preset.clone(),
                first_best_ms: first.best_ms,
                theoretical_best_ms: rows.iter().filter_map(|d| d.theoretical_best_ms).min(),
                last_at: last.started_at.clone(),
                bests: rows.iter().map(|d| CoachBest { session_id: d.id.clone(), at: d.started_at.clone(), best_ms: d.best_ms, theoretical_best_ms: d.theoretical_best_ms, preset: d.preset.clone() }).collect(),
                skills: SkillKey::ALL.iter().map(|k| skill(*k, rows)).collect(),
            };
            (car, rows)
        })
        .collect();
    cars.sort_by_key(|(_, rows)| std::cmp::Reverse(start_secs(&rows[rows.len() - 1].id)));

    // work items: each car × track's latest coast, braking and traction against the best on the track with any car,
    // by the excess in seconds per lap (braking: its share of the latest lap's braking time); none at its best
    let mut items: Vec<(f64, CoachWorkItem)> = Vec::new();
    for (car, rows) in cars.iter().filter(|(c, _)| c.sessions >= 2) {
        for (metric, corners) in [(SkillKey::Coast, 0), (SkillKey::Braking, 1), (SkillKey::Traction, 2)] {
            let skill = car.skills.iter().find(|s| s.key == metric).unwrap();
            let Some(value) = skill.value else { continue };
            let on_track = groups.iter().filter(|g| g[0].track == car.track && g[0].layout == car.layout).flatten().filter_map(|d| metric.value(d).map(|v| (*d, v)));
            let Some((best, best_value)) = on_track.min_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
            let latest = rows.iter().rev().find(|d| metric.value(d).is_some()).unwrap();
            let excess = match metric {
                SkillKey::Braking => latest.lock_front_share.filter(|s| *s > 0.0).map_or(0.0, |s| (value - best_value) / 100.0 * latest.lock_front_s / s),
                _ => value - best_value,
            };
            if excess <= 0.0 {
                continue;
            }
            let last = rows[rows.len() - 1];
            items.push((
                excess,
                CoachWorkItem {
                    metric,
                    car: car.car.clone(),
                    car_name: car.car_name.clone(),
                    track: car.track.clone(),
                    layout: car.layout.clone(),
                    value,
                    unit: skill.unit,
                    best_value,
                    best_car: best.car.clone(),
                    best_car_name: best.car_name.clone(),
                    history: skill.history.clone(),
                    trend: skill.trend,
                    r#where: [&last.coast_corners, &last.lock_corners, &last.spin_corners][corners].clone(),
                },
            ));
        }
    }
    items.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut work_items: Vec<CoachWorkItem> = Vec::new();
    for (_, item) in items {
        if work_items.len() < 3 && !work_items.iter().any(|w| w.car == item.car && w.metric == item.metric) {
            work_items.push(item);
        }
    }
    CoachProfile {
        version: 1,
        generated_at,
        sessions: digests.len() as u32,
        laps: digests.iter().map(|d| d.valid_laps).sum(),
        unseen,
        cars: cars.into_iter().map(|(c, _)| c).collect(),
        recent: digests.iter().rev().take(RECENT).cloned().collect(),
        work_items,
        driver: driver(digests),
        history: digests
            .iter()
            .map(|d| CoachHistory { id: d.id.clone(), at: d.started_at.clone(), car: d.car.clone(), car_name: d.car_name.clone(), track: d.track.clone(), layout: d.layout.clone() })
            .collect(),
        focus: None,
        focus_since: None,
    }
}

/// The offset from UTC (s) of an ISO 8601 time with its offset.
fn offset_s(iso: &str) -> Option<i64> {
    let utc = clock::unix_ms(&format!("{}+00:00", iso.get(..19)?))?;
    Some((utc - clock::unix_ms(iso)?) / 1000)
}

/// The profile from the digests on this PC at `now_ms`, in the offset of the newest session.
pub fn profile(dir: &Path, now_ms: i64) -> io::Result<CoachProfile> {
    store::upgrade(dir)?;
    let _lock = digest::lock();
    let all = digest::load(dir);
    let unseen = digest::unseen(dir, &all);
    let offset = all.last().and_then(|s| offset_s(&s.digest.started_at)).unwrap_or(0);
    let digests: Vec<CoachDigest> = all.into_iter().map(|s| s.digest).collect();
    let mut p = build(&digests, unseen, clock::iso_local(now_ms, offset));
    CoachFocus { focus: p.focus, focus_since: p.focus_since } = focus(dir);
    Ok(p)
}

/// The skill the driver works on.
pub fn focus(dir: &Path) -> CoachFocus {
    fs::read(dir.join(FOCUS)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// The driver accepted or chose `focus` at `now_ms` (null: none), one of the eight skills; choosing the same one again
/// keeps when it was first chosen. The time speaks the newest session's offset, as the profile does.
pub fn set_focus(dir: &Path, focus: Option<&str>, now_ms: i64) -> io::Result<CoachFocus> {
    let key: Option<SkillKey> = focus
        .map(|f| serde_json::from_value(serde_json::Value::from(f)).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "not a skill")))
        .transpose()?;
    let _lock = digest::lock();
    let before = self::focus(dir);
    let since = match key {
        None => None,
        Some(k) if before.focus == Some(k) => before.focus_since,
        Some(_) => {
            let offset = digest::load(dir).last().and_then(|s| offset_s(&s.digest.started_at)).unwrap_or(0);
            Some(clock::iso_local(now_ms, offset))
        }
    };
    let chosen = CoachFocus { focus: key, focus_since: since };
    fs::create_dir_all(dir)?;
    fs::write(dir.join(FOCUS), serde_json::to_vec(&chosen).map_err(io::Error::other)?)?;
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Whether two values are the same, numbers within 0.01 (Python rounds a half to the even digit).
    fn same(a: &Value, b: &Value, path: &str) -> Result<(), String> {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) if (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() <= 0.0101 => Ok(()),
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x.iter().zip(y).enumerate().try_for_each(|(i, (x, y))| same(x, y, &format!("{path}[{i}]"))),
            (Value::Object(x), Value::Object(y)) if x.len() == y.len() => x.iter().try_for_each(|(k, v)| same(v, y.get(k).ok_or(format!("{path}.{k} missing"))?, &format!("{path}.{k}"))),
            _ if a == b => Ok(()),
            _ => Err(format!("{path}: {a} against {b}")),
        }
    }

    const FIXTURE: &str = include_str!("../../tests/fixtures/example-profile.json");

    #[test]
    fn builds_the_fixture_s_cars_and_work_items_from_its_digests() {
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        let mut digests: Vec<CoachDigest> = serde_json::from_value(fixture["recent"].clone()).unwrap();
        digests.reverse();
        let built = serde_json::to_value(build(&digests, 1, fixture["generatedAt"].as_str().unwrap().into())).unwrap();
        same(&built["workItems"], &fixture["workItems"], "workItems").unwrap();
        same(&built["generatedAt"], &fixture["generatedAt"], "generatedAt").unwrap();
        let (cars, want) = (built["cars"].as_array().unwrap(), fixture["cars"].as_array().unwrap());
        assert_eq!(cars.len(), want.len());
        for (car, want) in cars.iter().zip(want) {
            let name = format!("{} {}", car["car"], car["layout"]);
            if car["car"] == "dd_porsche_gt2rs_mr" && car["layout"] == "Touristenfahrten" {
                // `recent` lacks the fixture's oldest digest (51 sessions, at most 50 recent), one of this car's:
                // the same from its second session on
                assert_eq!(car["sessions"], 27);
                same(&car["bests"], &Value::from(want["bests"].as_array().unwrap()[1..].to_vec()), &name).unwrap();
                for (s, w) in car["skills"].as_array().unwrap().iter().zip(want["skills"].as_array().unwrap()) {
                    same(&s["history"], &Value::from(w["history"].as_array().unwrap()[1..].to_vec()), &format!("{name} {}", s["key"])).unwrap();
                }
                continue;
            }
            same(car, want, &name).unwrap();
        }
    }

    #[test]
    fn upgrades_joe_s_digests_and_builds_the_v2_fixture_s_driver() {
        // example-profile-v2.json (profile_fixture_v2.py) from the 60 digests app 0.26 stored on Joe's PC by 2026-11-02;
        // without their sessions the upgrade derives version 2 from each digest's fields, as the script does
        let dir = std::env::temp_dir().join(format!("dd-engineer-profile-v2-{}", std::process::id())).join(store::DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(digest::FILE), include_str!("../../tests/fixtures/digests-v1.jsonl")).unwrap();
        let p = profile(&dir, 1_793_600_000_000).unwrap();
        let fixture: Value = serde_json::from_str(include_str!("../../tests/fixtures/example-profile-v2.json")).unwrap();
        let built = serde_json::to_value(&p).unwrap();
        let d = &p.driver;
        assert_eq!((d.sessions, d.laps, d.cars, d.tracks, d.since.as_str()), (60, 98, 11, 3, "2026-10-11T23:19:12+02:00"));
        assert_eq!((p.history.len(), p.recent.len(), p.focus, p.focus_since.clone()), (60, 50, None, None));
        // coast: the median of 60 sessions' coasting share (3.42 and 3.43 the middle two; Python rounds the 3.425 down,
        // Rust up), falling; the Alfa's single session the best car
        let coast = &d.skills[0];
        assert_eq!((coast.key, coast.unit, coast.lower_better, coast.trend), (SkillKey::Coast, Unit::Percent, true, Some(Trend::Better)));
        assert!((coast.value.unwrap() - 3.425).abs() <= 0.0051, "{:?}", coast.value);
        assert_eq!((coast.cars.len(), coast.cars[0].car.as_str(), coast.cars[0].sessions), (11, "ks_alfa_romeo_giulia_gtam", 1));
        assert!(coast.cars.windows(2).all(|w| w[0].value <= w[1].value));
        assert_eq!(coast.r#where.as_ref().map(|w| (w.layout.as_str(), w.corners.clone())), Some(("Touristenfahrten", vec![19, 28, 6])));
        let pace = &d.skills[4];
        assert_eq!((pace.key, pace.value, pace.trend, pace.history.iter().flatten().count(), pace.r#where.clone()), (SkillKey::Pace, Some(1.23), Some(Trend::Better), 60, None));
        // the old reports measured no smoothness; clean and tyres are better the higher
        assert_eq!((d.skills[3].key, d.skills[3].unit, d.skills[3].value, d.skills[3].r#where.clone()), (SkillKey::Smooth, Unit::Count, None, None));
        assert_eq!(d.skills.iter().map(|s| s.lower_better).collect::<Vec<_>>(), [true, true, true, true, true, true, false, false]);
        // and every number as the script has it: the driver, the history, the digests of version 2, the cars and
        // work items of version 1
        for key in ["driver", "history", "recent", "cars", "workItems", "focus", "focusSince", "sessions", "laps", "version"] {
            same(&built[key], &fixture[key], key).unwrap();
        }
        // each digest goes up again as version 2
        assert_eq!(digest::pending(&dir).unwrap().digests.len(), 50);
        assert!(digest::digests(&dir).iter().all(|d| d.version == 2));
        store::forget(&dir).unwrap();
    }

    #[test]
    fn keeps_the_focus_the_driver_chose_and_since_when() {
        let dir = std::env::temp_dir().join(format!("dd-engineer-focus-{}", std::process::id())).join(store::DIR);
        assert_eq!(focus(&dir), CoachFocus { focus: None, focus_since: None });
        // one of the eight skills, at the newest session's offset (none yet: UTC)
        let chosen = set_focus(&dir, Some("smooth"), 1_793_200_000_000).unwrap();
        assert_eq!(chosen, CoachFocus { focus: Some(SkillKey::Smooth), focus_since: Some("2026-10-28T15:06:40+00:00".into()) });
        assert_eq!(focus(&dir), chosen);
        assert_eq!(serde_json::to_value(&chosen).unwrap(), serde_json::json!({ "focus": "smooth", "focusSince": "2026-10-28T15:06:40+00:00" }));
        // the same again keeps its start; another starts anew; anything else is refused and changes nothing
        assert_eq!(set_focus(&dir, Some("smooth"), 1_793_300_000_000).unwrap(), chosen);
        assert_eq!(set_focus(&dir, Some("clean"), 1_793_300_000_000).unwrap().focus_since.as_deref(), Some("2026-10-29T18:53:20+00:00"));
        for wrong in ["Clean", "lap", ""] {
            assert_eq!(set_focus(&dir, Some(wrong), 0).unwrap_err().kind(), io::ErrorKind::InvalidInput, "{wrong}");
        }
        assert_eq!(focus(&dir).focus, Some(SkillKey::Clean));
        // the profile and the digests to upload carry it
        assert_eq!((profile(&dir, 0).unwrap().focus, digest::pending(&dir).unwrap().focus), (Some(SkillKey::Clean), Some(SkillKey::Clean)));
        assert_eq!(set_focus(&dir, None, 0).unwrap(), CoachFocus::default());
        let p = profile(&dir, 0).unwrap();
        assert_eq!((p.focus, p.focus_since), (None, None));
        store::forget(&dir).unwrap();
    }

    fn digest(id: &str, track: &str, coast_s: f64, front: Option<(f64, f64)>) -> CoachDigest {
        let mut d: CoachDigest = serde_json::from_value(serde_json::from_str::<Value>(FIXTURE).unwrap()["recent"][0].clone()).unwrap();
        d.id = id.into();
        d.car = id.split_once('-').unwrap().1.into();
        d.car_name = d.car.to_uppercase();
        d.track = track.into();
        d.started_at = clock::iso_local(start_secs(id) * 1000, 3600);
        d.coast_s = coast_s;
        d.spin_s = 0.0;
        (d.lock_front_s, d.lock_front_share) = front.map_or((0.0, None), |(s, share)| (s, Some(share)));
        d
    }

    #[test]
    fn works_on_the_largest_excess_per_lap_one_per_car_and_metric() {
        let digests = [
            digest("1-a", "T", 5.0, Some((1.2, 0.02))),
            digest("2-a", "T", 6.0, Some((3.0, 0.05))),
            digest("3-b", "T", 2.0, None),
            digest("4-b", "T", 2.5, Some((0.6, 0.01))),
            // a car driven once has nothing to work on yet; another track has its own best
            digest("5-c", "T", 9.0, None),
            digest("6-a", "U", 7.0, None),
            digest("7-a", "U", 7.5, None),
        ];
        let p = build(&digests, 0, String::new());
        let items: Vec<(SkillKey, &str, &str, f64, f64, &str)> = p.work_items.iter().map(|w| (w.metric, w.car.as_str(), w.track.as_str(), w.value, w.best_value, w.best_car.as_str())).collect();
        // a's coast 6.0 against b's 2.0: 4 s; a's braking 5 % against b's 1 % of a's 60 s braking time: 2.4 s;
        // b's coast 2.5 against its own 2.0: 0.5 s (a's on U too, but a's coast is in already); b's braking is the
        // track's best
        assert_eq!(items, [(SkillKey::Coast, "a", "T", 6.0, 2.0, "b"), (SkillKey::Braking, "a", "T", 5.0, 1.0, "b"), (SkillKey::Coast, "b", "T", 2.5, 2.0, "b")]);
        assert_eq!(p.cars.iter().map(|c| (c.car.as_str(), c.track.as_str())).collect::<Vec<_>>(), [("a", "U"), ("c", "T"), ("b", "T"), ("a", "T")]);
        let braking = &p.cars[3].skills[1];
        assert_eq!((braking.value, braking.score, braking.best, braking.worst, braking.history.clone()), (Some(5.0), Some(0), Some(2.0), Some(5.0), vec![Some(2.0), Some(5.0)]));
        assert_eq!(serde_json::to_value(&p.work_items[0]).unwrap()["where"], serde_json::json!([25, 22, 39]));
    }

    #[test]
    fn scores_and_trends_a_skill_over_the_sessions() {
        let rows: Vec<CoachDigest> = [10.0, 8.0, 9.0, 6.0, 5.0].iter().enumerate().map(|(i, c)| digest(&format!("{}-a", i + 1), "T", *c, None)).collect();
        let s = skill(SkillKey::Coast, &rows.iter().collect::<Vec<_>>());
        // 5 s is the best: 100; the second half (9, 6, 5) against the first (10, 8): better
        assert_eq!((s.value, s.score, s.trend, s.best, s.worst), (Some(5.0), Some(100), Some(Trend::Better), Some(5.0), Some(10.0)));
        let s = skill(SkillKey::Coast, &rows[..3].iter().collect::<Vec<_>>());
        assert_eq!((s.score, s.trend), (Some(50), None));
        let s = skill(SkillKey::Coast, &rows[..1].iter().collect::<Vec<_>>());
        assert_eq!((s.score, s.trend), (None, None));
        let flat: Vec<CoachDigest> = [10.0, 10.2, 10.1, 10.4].iter().enumerate().map(|(i, c)| digest(&format!("{}-a", i + 1), "T", *c, None)).collect();
        assert_eq!(skill(SkillKey::Coast, &flat.iter().collect::<Vec<_>>()).trend, Some(Trend::Flat));
    }

    #[test]
    fn the_profile_on_this_pc_counts_what_is_new_and_speaks_the_newest_session_s_offset() {
        let dir = std::env::temp_dir().join(format!("dd-engineer-profile-{}", std::process::id())).join(store::DIR);
        let p = profile(&dir, 1_793_200_000_000).unwrap();
        assert_eq!((p.sessions, p.cars.len(), p.unseen, p.generated_at.as_str()), (0, 0, 0, "2026-10-28T15:06:40+00:00"));
        let mut s: super::super::EngineerSession = serde_json::from_str(include_str!("../../tests/fixtures/example-session.json")).unwrap();
        s.id = "1793100000-dd_bmw_m3_e46_rennsport".into();
        s.started_at = "2026-10-28T10:20:00+01:00".into();
        store::save_session(&dir, &s, &[]).unwrap();
        digest::record(&dir, &s, 1_793_100_500_000).unwrap();
        let p = profile(&dir, 1_793_200_000_000).unwrap();
        assert_eq!((p.sessions, p.laps, p.cars.len(), p.recent.len(), p.unseen), (1, 2, 1, 1, 1));
        assert_eq!(p.generated_at, "2026-10-28T16:06:40+01:00");
        digest::seen(&dir, 1_793_200_000_000).unwrap();
        assert_eq!(profile(&dir, 1_793_200_000_000).unwrap().unseen, 0);
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["cars"][0]["skills"].as_array().unwrap().iter().map(|s| s["key"].as_str().unwrap()).collect::<Vec<_>>(), ["coast", "braking", "traction", "pace", "consistency", "tyres"]);
        assert_eq!((json["cars"][0]["skills"][0]["unit"].as_str(), json["cars"][0]["skills"][1]["unit"].as_str()), (Some("s"), Some("%")));
        store::forget(&dir).unwrap();
    }
}
