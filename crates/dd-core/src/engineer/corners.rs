//! The corner map of a track and layout, from the driver's own laps (no game data): the curvature κ = ROTY / SPEED
//! (above 5 m/s), averaged over ±20 m; a corner is a stretch with |κ| above 1/350 m for at least 25 m, and stretches
//! of the same direction less than 30 m apart are one corner (on Joe's Nordschleife laps of 2026-10-04 that gives
//! 69 corners, research/2026-10-05-club-build/telemetry-core.md §4). Each corner owns the segment from the middle
//! of the straight before it to the middle of the one after it; its apex is the reference lap's lowest speed in it.
//! The map is kept per track and layout (`<config>/engineer/maps/<track>__<layout>.json`) and frozen once three
//! laps agree with it (90 % of the corners found within 15 m), so the corner numbers stay for the history.
//!
//! A lap is put on the map's distance by its own corners: the shift (±300 m) at which most of them meet the map's,
//! then a straight-line fit of their turn-in and turn-out points (offset and scale). On Joe's laps the turn-in
//! points repeat within 5 m (median), the speed minima only within 11 m (41 m at the 90th percentile).

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{round, Corner, CornerMap, Direction};

const MIN_SPEED: f64 = 5.0;
const SMOOTH_M: usize = 20;
pub const KAPPA: f64 = 1.0 / 350.0;
const MIN_CORNER_M: f64 = 25.0;
const MERGE_M: f64 = 30.0;
const MATCH_M: f64 = 15.0;
const SHIFT_M: i32 = 300;
/// Share of the corners that must meet for a lap to agree with the map.
const AGREE: f64 = 0.9;
/// Share of the corners that must meet to measure a lap on the map.
const USABLE: f64 = 0.5;
/// Laps that agree before the map is frozen.
const FREEZE: usize = 3;

/// The curvature at each metre, averaged over ±20 m; > 0 to the left.
pub fn curvature(speed: &[f64], yaw: &[f64]) -> Vec<f64> {
    let raw: Vec<f64> = speed.iter().zip(yaw).map(|(v, r)| if *v > MIN_SPEED { r / v } else { 0.0 }).collect();
    let mut sum = vec![0.0; raw.len() + 1];
    for (i, k) in raw.iter().enumerate() {
        sum[i + 1] = sum[i] + k;
    }
    (0..raw.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(SMOOTH_M), (i + SMOOTH_M + 1).min(raw.len()));
            (sum[b] - sum[a]) / (b - a) as f64
        })
        .collect()
}

/// A corner found on a lap: where |κ| rises above and falls below the limit, and the largest |κ| in between.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detected {
    pub turn_in: f64,
    pub turn_out: f64,
    pub direction: Direction,
    pub peak: f64,
}

pub fn detect(kappa: &[f64]) -> Vec<Detected> {
    // where |κ| crosses the limit between metres i - 1 and i, to a fraction of a metre
    let crossing = |i: usize| -> f64 {
        let (a, b) = (kappa[i - 1].abs(), kappa[i].abs());
        if (a - b).abs() > 1e-12 { i as f64 - 1.0 + ((KAPPA - a) / (b - a)).clamp(0.0, 1.0) } else { i as f64 }
    };
    let mut stretches: Vec<Detected> = Vec::new();
    let mut open: Option<Detected> = None;
    for (i, k) in kappa.iter().enumerate() {
        let direction = if *k > 0.0 { Direction::L } else { Direction::R };
        let inside = k.abs() > KAPPA;
        match &mut open {
            Some(c) if inside && c.direction == direction => c.peak = c.peak.max(k.abs()),
            _ => {
                if let Some(mut c) = open.take() {
                    c.turn_out = crossing(i).max(c.turn_in);
                    stretches.push(c);
                }
                if inside {
                    let turn_in = if i == 0 { 0.0 } else { crossing(i) };
                    open = Some(Detected { turn_in, turn_out: turn_in, direction, peak: k.abs() });
                }
            }
        }
    }
    if let Some(mut c) = open {
        c.turn_out = kappa.len().saturating_sub(1) as f64;
        stretches.push(c);
    }
    let mut merged: Vec<Detected> = Vec::new();
    for s in stretches {
        match merged.last_mut() {
            Some(last) if last.direction == s.direction && s.turn_in - last.turn_out < MERGE_M => {
                last.turn_out = s.turn_out;
                last.peak = last.peak.max(s.peak);
            }
            _ => merged.push(s),
        }
    }
    merged.retain(|c| c.turn_out - c.turn_in >= MIN_CORNER_M);
    merged
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapCorner {
    pub n: u32,
    pub start_m: f64,
    pub end_m: f64,
    pub apex_m: f64,
    pub turn_in_m: f64,
    pub turn_out_m: f64,
    pub direction: Direction,
    pub radius_m: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Map {
    pub version: u32,
    pub frozen: bool,
    /// The laps (ids) that agreed with this version
    pub agreed: Vec<String>,
    pub length_m: f64,
    pub corners: Vec<MapCorner>,
}

impl Map {
    /// The map of a lap's corners, `speed` its speed at each metre.
    pub fn from_lap(found: &[Detected], speed: &[f64], version: u32) -> Map {
        let length = speed.len().saturating_sub(1) as f64;
        let corners = found
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let start = if i == 0 { 0.0 } else { (found[i - 1].turn_out + c.turn_in) / 2.0 };
                let end = found.get(i + 1).map_or(length, |next| (c.turn_out + next.turn_in) / 2.0);
                let (first, last) = (c.turn_in.ceil() as usize, (c.turn_out.floor() as usize).min(speed.len() - 1));
                let apex = (first..=last.max(first)).min_by(|a, b| speed[*a].total_cmp(&speed[*b])).unwrap_or(first);
                MapCorner {
                    n: i as u32 + 1,
                    start_m: round(start, 2),
                    end_m: round(end, 2),
                    apex_m: apex as f64,
                    turn_in_m: round(c.turn_in, 2),
                    turn_out_m: round(c.turn_out, 2),
                    direction: c.direction,
                    radius_m: round(1.0 / c.peak, 0),
                }
            })
            .collect();
        Map { version, frozen: false, agreed: Vec::new(), length_m: length, corners }
    }

    /// The map as the session carries it.
    pub fn contract(&self) -> CornerMap {
        CornerMap {
            version: self.version,
            frozen: self.frozen,
            corners: self
                .corners
                .iter()
                .map(|c| Corner { n: c.n, start_m: round(c.start_m, 1), end_m: round(c.end_m, 1), apex_m: c.apex_m, direction: c.direction, radius_m: c.radius_m })
                .collect(),
        }
    }

    /// Whether a lap whose corners `found` are fitted by `fit` agrees with the map (counts towards freezing it).
    pub fn agrees(&self, found: &[Detected], fit: &Fit) -> bool {
        !self.corners.is_empty() && fit.matched as f64 >= AGREE * self.corners.len().max(found.len()) as f64
    }

    /// Whether the map fits a lap well enough to measure it on: half of the corners meet. Two laps of one
    /// session meet at 88 % (Joe's of 2026-10-04), a lap of another track or a wrong window at almost none.
    pub fn usable(&self, found: &[Detected], fit: &Fit) -> bool {
        !self.corners.is_empty() && fit.matched as f64 >= USABLE * self.corners.len().max(found.len()) as f64
    }
}

/// How a lap's distance maps onto the map's: `map = lap × scale + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub scale: f64,
    pub offset: f64,
    /// The map's corners that meet one of the lap's within 15 m
    pub matched: usize,
}

impl Fit {
    pub fn to_map(&self, lap_m: f64) -> f64 {
        lap_m * self.scale + self.offset
    }

    pub fn to_lap(&self, map_m: f64) -> f64 {
        (map_m - self.offset) / self.scale
    }
}

/// For each of the map's corners, the lap's corner of the same direction whose turn-in or turn-out lies nearest to
/// the map's after `to_map`, within 15 m (on Joe's two laps of 2026-10-04 the turn-in alone meets 56 of 69 corners,
/// either end 61).
fn matches<'a>(found: &'a [Detected], map: &'a Map, to_map: &dyn Fn(f64) -> f64) -> Vec<(&'a Detected, &'a MapCorner)> {
    let gap = |c: &Detected, m: &MapCorner| (to_map(c.turn_in) - m.turn_in_m).abs().min((to_map(c.turn_out) - m.turn_out_m).abs());
    map.corners
        .iter()
        .filter_map(|m| {
            found
                .iter()
                .filter(|c| c.direction == m.direction)
                .map(|c| (c, gap(c, m)))
                .filter(|(_, d)| *d <= MATCH_M)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(c, _)| (c, m))
        })
        .collect()
}

/// Puts a lap's corners on the map: the whole-metre shift within ±300 m that matches most corners (the smallest
/// one of equals), then a least-squares line through the matched turn-in and turn-out points that lie within 15 m.
pub fn fit(found: &[Detected], map: &Map) -> Fit {
    let mut best = (0usize, 0i32);
    for shift in 0..=2 * SHIFT_M {
        let shift = if shift % 2 == 0 { shift / 2 } else { -(shift + 1) / 2 };
        let count = matches(found, map, &|m| m + shift as f64).len();
        if count > best.0 {
            best = (count, shift);
        }
    }
    let shift = best.1 as f64;
    // a corner cut by the lap's start or end has no turn-in or turn-out there
    let inner = |map_m: f64| map_m > 0.0 && map_m < map.length_m;
    let points: Vec<(f64, f64)> = matches(found, map, &|m| m + shift)
        .into_iter()
        .flat_map(|(c, m)| [(c.turn_in, m.turn_in_m), (c.turn_out, m.turn_out_m)])
        .filter(|(lap, map_m)| inner(*map_m) && (lap + shift - map_m).abs() <= MATCH_M)
        .collect();
    let (scale, offset) = if points.len() >= 4 {
        let n = points.len() as f64;
        let (mx, my) = (points.iter().map(|p| p.0).sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
        let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
        let sxy: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        let scale = if sxx > 0.0 { sxy / sxx } else { 1.0 };
        if (0.9..=1.1).contains(&scale) { (scale, my - scale * mx) } else { (1.0, shift) }
    } else {
        (1.0, shift)
    };
    let matched = matches(found, map, &|m| m * scale + offset).len();
    Fit { scale, offset, matched }
}

/// A lap the map is updated with: its id, its corners and its speed at each metre.
pub struct LapShape<'a> {
    pub id: &'a str,
    pub found: &'a [Detected],
    pub speed: &'a [f64],
}

/// The map after a session's laps. Without a map, the lap `best` makes one. An unfrozen map counts the laps that
/// agree with it and freezes at three; when it fits none of the session's laps even to measure on, `best` makes
/// the next version. A frozen map stays as it is (only forgetting everything makes a new one).
pub fn update(old: Option<Map>, laps: &[LapShape], best: usize) -> Option<Map> {
    let shape = laps.get(best)?;
    let had = old.is_some();
    let mut map = old.unwrap_or_else(|| Map::from_lap(shape.found, shape.speed, 1));
    if map.frozen {
        return Some(map);
    }
    let agreeing = |map: &Map| -> Vec<String> { laps.iter().filter(|l| map.agrees(l.found, &fit(l.found, map))).map(|l| l.id.to_string()).collect() };
    if had && !laps.iter().any(|l| map.usable(l.found, &fit(l.found, &map))) {
        map = Map::from_lap(shape.found, shape.speed, map.version + 1);
    }
    let ids = agreeing(&map);
    for id in ids {
        if !map.agreed.contains(&id) {
            map.agreed.push(id);
        }
    }
    map.frozen = map.agreed.len() >= FREEZE;
    Some(map)
}

/// A file name for a track and layout: letters and digits kept, anything else `_`.
pub fn file_key(parts: &[&str]) -> String {
    parts.iter().map(|p| p.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect::<String>()).collect::<Vec<_>>().join("__")
}

fn map_path(dir: &Path, track: &str, layout: &str) -> std::path::PathBuf {
    dir.join("maps").join(format!("{}.json", file_key(&[track, layout])))
}

pub fn load(dir: &Path, track: &str, layout: &str) -> Option<Map> {
    fs::read(map_path(dir, track, layout)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

pub fn save(dir: &Path, track: &str, layout: &str, map: &Map) -> io::Result<()> {
    let path = map_path(dir, track, layout);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, serde_json::to_vec(map).map_err(io::Error::other)?)
}

#[cfg(test)]
mod tests {
    use super::super::cut::{grid, Data, Grid};
    use super::super::ld::Ld;
    use super::super::testkit::{self, Drive};
    use super::*;
    use std::io::Cursor;

    fn laps() -> [Grid; 2] {
        let session = testkit::session(&Drive { late: None, lock: false });
        let mut r = Cursor::new(testkit::write_ld("04/10/2026", "23:40:23", &session.chans));
        let ld = Ld::read(&mut r).unwrap();
        let data = Data::read(&ld, &mut r).unwrap();
        session.laps.map(|(a, b)| grid(&data, a, b).unwrap())
    }

    fn corners_of(lap: &Grid) -> Vec<Detected> {
        detect(&curvature(&lap.speed, &lap.yaw))
    }

    #[test]
    fn finds_the_track_s_corners_with_their_direction_and_radius() {
        let [lap, _] = laps();
        let found = corners_of(&lap);
        let arcs = testkit::arcs();
        assert_eq!(found.len(), arcs.len(), "{found:?}");
        for (c, (a, b, k)) in found.iter().zip(&arcs) {
            let (centre, arc_centre) = ((c.turn_in + c.turn_out) / 2.0, (a + b) / 2.0);
            assert!((centre - arc_centre).abs() <= 10.0, "corner at {centre} m, arc at {arc_centre} m");
            assert_eq!(c.direction, if *k > 0.0 { Direction::L } else { Direction::R });
            assert!((1.0 / c.peak - 1.0 / k.abs()).abs() <= 0.1 / k.abs(), "radius {} against {}", 1.0 / c.peak, 1.0 / k.abs());
        }
    }

    #[test]
    fn merges_close_stretches_and_drops_short_ones() {
        let mut k = vec![0.0; 400];
        k[10..40].fill(0.01); // 30 m left
        k[60..80].fill(0.01); // 20 m left, 20 m after it: one corner
        k[140..160].fill(-0.01); // 20 m right alone: too short
        k[200..260].fill(-0.01); // right
        k[262..300].fill(0.01); // left right after it: another corner
        let found = detect(&k);
        let spans: Vec<(f64, f64, Direction)> = found.iter().map(|c| (round(c.turn_in, 2), round(c.turn_out, 2), c.direction)).collect();
        // the limit (1/350) is crossed 29 % of the way between the metres
        assert_eq!(spans, [(9.29, 79.71, Direction::L), (199.29, 259.71, Direction::R), (261.29, 299.71, Direction::L)]);
    }

    #[test]
    fn segments_tile_the_lap_and_the_apex_is_the_lowest_speed() {
        let [lap, _] = laps();
        let map = Map::from_lap(&corners_of(&lap), &lap.speed, 1);
        assert_eq!(map.corners.len(), 4);
        assert_eq!(map.corners[0].start_m, 0.0);
        assert_eq!(map.corners[3].end_m, lap.length());
        for pair in map.corners.windows(2) {
            assert_eq!(pair[0].end_m, pair[1].start_m);
            assert!(pair[0].turn_out_m < pair[0].end_m && pair[1].start_m < pair[1].turn_in_m);
        }
        for c in &map.corners {
            let apex = c.apex_m as usize;
            assert!(c.turn_in_m <= c.apex_m && c.apex_m <= c.turn_out_m);
            assert!((c.turn_in_m as usize..=c.turn_out_m as usize).all(|i| lap.speed[i] >= lap.speed[apex]));
        }
        let contract = map.contract();
        assert_eq!((contract.version, contract.frozen, contract.corners[2].n, contract.corners[2].radius_m), (1, false, 3, 40.0));
    }

    #[test]
    fn puts_a_lap_on_the_map_s_distance() {
        let [one, two] = laps();
        let map = Map::from_lap(&corners_of(&one), &one.speed, 1);
        let same = fit(&corners_of(&two), &map);
        assert_eq!(same.matched, 4);
        assert!((same.scale - 1.0).abs() < 0.001 && same.offset.abs() < 2.0, "{same:?}");
        // a lap that started 47 m later, measured 0.5 % short
        let moved: Vec<Detected> = corners_of(&two).iter().map(|c| Detected { turn_in: (c.turn_in - 47.0) * 0.995, turn_out: (c.turn_out - 47.0) * 0.995, ..*c }).collect();
        let f = fit(&moved, &map);
        assert_eq!(f.matched, 4);
        assert!((f.to_map(moved[2].turn_in) - map.corners[2].turn_in_m).abs() < 1.0, "{f:?}");
        assert!((f.to_lap(map.corners[1].turn_out_m) - moved[1].turn_out).abs() < 1.0);
        // corners that do not meet the map's
        let elsewhere: Vec<Detected> = corners_of(&two).iter().map(|c| Detected { turn_in: c.turn_in + 1000.0, turn_out: c.turn_out + 1000.0, ..*c }).collect();
        assert!(!map.agrees(&elsewhere, &fit(&elsewhere, &map)));
    }

    #[test]
    fn freezes_once_three_laps_agree_and_stays_frozen() {
        let [one, two] = laps();
        let (c1, c2) = (corners_of(&one), corners_of(&two));
        let shape = |id, found| LapShape { id, found, speed: &one.speed };
        let map = update(None, &[shape("a#1", &c1), shape("a#2", &c2)], 0).unwrap();
        assert_eq!((map.version, map.frozen, map.agreed.len()), (1, false, 2));
        // the same laps again (a session evaluated a second time) do not count twice
        let map = update(Some(map), &[shape("a#1", &c1), shape("a#2", &c2)], 0).unwrap();
        assert_eq!((map.frozen, map.agreed.len()), (false, 2));
        // a lap the map does not fit makes the next version of an unfrozen map
        let other: Vec<Detected> = c2.iter().map(|c| Detected { direction: if c.direction == Direction::L { Direction::R } else { Direction::L }, ..*c }).collect();
        assert!(!map.usable(&other, &fit(&other, &map)) && map.usable(&c2, &fit(&c2, &map)));
        let next = update(Some(map.clone()), &[shape("b#1", &other)], 0).unwrap();
        assert_eq!((next.version, next.agreed.clone()), (2, vec!["b#1".to_string()]));
        // the third lap freezes it, and a frozen map stays
        let frozen = update(Some(map), &[shape("c#1", &c2)], 0).unwrap();
        assert_eq!((frozen.version, frozen.frozen, frozen.agreed.len()), (1, true, 3));
        assert_eq!(update(Some(frozen.clone()), &[shape("d#1", &other)], 0).unwrap(), frozen);
        assert_eq!(update(None, &[], 0), None);
    }

    #[test]
    fn keeps_one_map_per_track_and_layout() {
        let dir = std::env::temp_dir().join(format!("dd-engineer-maps-{}", std::process::id()));
        let [one, _] = laps();
        let map = Map::from_lap(&corners_of(&one), &one.speed, 1);
        assert_eq!(load(&dir, "Nurburgring", "Touristenfahrten"), None);
        save(&dir, "Nurburgring", "Touristenfahrten", &map).unwrap();
        assert_eq!(load(&dir, "Nurburgring", "Touristenfahrten"), Some(map));
        assert!(dir.join("maps").join("Nurburgring__Touristenfahrten.json").is_file());
        assert_eq!(file_key(&["Spa", "24h / GP"]), "Spa__24h___GP");
        fs::remove_dir_all(&dir).unwrap();
    }
}
