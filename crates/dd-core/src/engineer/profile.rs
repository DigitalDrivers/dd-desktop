//! The coach's profile of the driver (`engineer_profile`), computed from the digests on this PC ([`super::digest`]):
//! per car, track and layout the best laps and six skills, and the three things most worth working on. The rules
//! are those of research/2026-10-21-renningenieur-coach/profile_fixture.py, whose output on Joe's sessions
//! (`example-profile.json`) both this crate's and the platform's tests read; the types are the contract
//! (`contract-coach.ts`).

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::digest::{self, start_secs, CoachDigest};
use super::{clock, round, store};

/// `recent` holds at most this many digests.
const RECENT: usize = 50;
/// A trend within this share of the first half's mean is flat.
const FLAT: f64 = 0.05;

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
    Pace,
    Consistency,
    Tyres,
}

impl SkillKey {
    const ALL: [SkillKey; 6] = [SkillKey::Coast, SkillKey::Braking, SkillKey::Traction, SkillKey::Pace, SkillKey::Consistency, SkillKey::Tyres];

    fn unit(self) -> Unit {
        match self {
            SkillKey::Coast | SkillKey::Traction => Unit::Seconds,
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    #[serde(rename = "s")]
    Seconds,
    #[serde(rename = "%")]
    Percent,
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
    Ok(build(&digests, unseen, clock::iso_local(now_ms, offset)))
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
