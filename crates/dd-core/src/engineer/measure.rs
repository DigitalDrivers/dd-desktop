//! The numbers of a lap ([`measure`]) and the report that compares them with the reference lap ([`report`]).
//! Corner numbers come from the distance grid, each corner over its segment of the map (put on the lap's own
//! distance by the lap's fit), and their positions are given on the map's distance, so they compare with the
//! reference's directly. Fuel, tyres and brakes come from the file's samples over the lap's time window.

use serde::{Deserialize, Serialize};

use super::corners::{Fit, Map, KAPPA};
use super::cut::{mean, Data, Grid};
use super::{round, Band, BandBalance, Brakes, CornerBalance, CornerBrake, CornerResult, CornerThrottle, EngineerLap, EngineerTrace, Finding, Fuel, GearTraction, MinSpeed, TargetPsi, TimeSource, Tyres};
use super::laps::LocalLap;

/// A pedal counts as pressed above this (%).
const PEDAL: f64 = 5.0;
const PICKUP: f64 = 40.0;
const FULL: f64 = 95.0;
/// Throttle above this (%) for spin
const SPIN_THROTTLE: f64 = 50.0;
const SLIP: f64 = 0.15;
/// Yaw rate (rad/s) above which steering per yaw is measured
const YAW: f64 = 0.05;
/// Speeds (m/s) below which brake and traction samples are not counted
const SLOW: f64 = 30.0 / 3.6;
/// The lowest speed of a corner is looked for this far (m) before its turn-in and after its turn-out.
const APEX_MARGIN: f64 = 30.0;
/// The pressures at a lap's end: the mean of its last 20 s.
const END_S: f64 = 20.0;
const HOT: f64 = 700.0;
const VERY_HOT: f64 = 800.0;
const COLD: f64 = 500.0;
const TRACE_STEP: usize = 5;

/// How far a lap's hot pressure may lie from its compound's ideal: the telemetry report's acceptance tolerance
/// (`pressure_accept_tol` 0.5 psi in ~/acevo-setups/tools/tlm_targets.json, from plans/2026-10-05-club-umsetzung.md
/// WP6 "27 ± 0,5 psi"). tlm.py flags its runs at 0.3 already (`pressure_flag_tol`, just above the scatter of two
/// equal runs); the engineer speaks only outside what is accepted.
const WINDOW_PSI: f64 = 0.5;

/// The ideal hot pressure (`idealPressure`, psi; front and rear alike) of each club car's compounds, by the car's
/// id and the compound's short name as the game's log names it. Source: the installed packages, read 2026-10-29
/// with ~/acevo-setups/tools/car.py (`car.load(<id>)["compounds"]`, the tyre files named per entry);
/// research/2026-10-21-renningenieur-coach/check_pressure_table.py checks this table against them. A car or
/// compound not listed (every Kunos car) gets no judgement.
const IDEAL_PSI: [(&str, &str, f64); 20] = [
    // cup2r_hypercar_275_35_20 / _335_30_21.tyre, Michelin Pilot Sport Cup 2 R N0 (the package's default)
    ("dd_porsche_gt2rs_mr", "C2R", 30.0),
    // trofeors_hypercar_275_35_20 / _335_30_21.tyre, Pirelli P Zero Trofeo RS
    ("dd_porsche_gt2rs_mr", "TRS", 30.0),
    // cup2_supercar_275_35_20 / _335_30_21.tyre, Michelin Pilot Sport Cup 2 N0
    ("dd_porsche_gt2rs_mr", "C2", 30.0),
    // slick_slick_325_660_18 / _325_705_18.tyre, Michelin racing slick 20 / 21 in (bespoke)
    ("dd_porsche_gt2rs_mr", "S", 27.0),
    // cup2r_hypercar_235_35_19 / _265_30_19.tyre, Michelin Pilot Sport Cup 2 R
    ("dd_bmw_m3_e46_clubsport", "C2R", 29.0),
    // trofeors_hypercar_235_35_19 / _265_30_19.tyre, Pirelli P Zero Trofeo RS
    ("dd_bmw_m3_e46_clubsport", "TRS", 29.0),
    // a052_hypercar_235_35_19 / _265_30_19.tyre, Yokohama Advan A052
    ("dd_bmw_m3_e46_clubsport", "A052", 29.0),
    // slick_slick_275_645_18 / _305_645_18.tyre, Pirelli P Zero DHF GT3 slick
    ("dd_bmw_m3_e46_gt3", "S", 27.0),
    // wet_wet_275_645_18 / _305_645_18.tyre, Pirelli Cinturato GT3 wet
    ("dd_bmw_m3_e46_gt3", "W", 26.0),
    // cup2r_hypercar_235_35_19 / _265_30_19.tyre, Michelin Pilot Sport Cup 2 R
    ("dd_bmw_m3_e46_gt3", "C2R", 29.0),
    // trofeors_hypercar_235_35_19 / _265_30_19.tyre, Pirelli P Zero Trofeo RS
    ("dd_bmw_m3_e46_gt3", "TRS", 29.0),
    // a052_hypercar_235_35_19 / _265_30_19.tyre, Yokohama Advan A052
    ("dd_bmw_m3_e46_gt3", "A052", 29.0),
    // slick_slick_325_680_18 / _325_705_18.tyre, Pirelli P Zero DHF GT3 slick
    ("dd_bmw_m3_e46_rennsport", "S", 27.0),
    // wet_wet_325_680_18 / _325_705_18.tyre, Pirelli Cinturato GT3 wet
    ("dd_bmw_m3_e46_rennsport", "W", 26.0),
    // slick_slick_325_680_18.tyre front and rear, Pirelli P Zero DHF GT3 slick
    ("dd_audi_rs6_gt3", "S", 27.0),
    // wet_wet_325_680_18.tyre front and rear, Pirelli Cinturato GT3 wet
    ("dd_audi_rs6_gt3", "W", 26.0),
    // slick_slick_325_680_18 / _325_705_18.tyre, Pirelli P Zero DHF GT3 slick
    ("dd_audi_rs6_v12", "S", 27.0),
    // wet_wet_325_680_18 / _325_705_18.tyre, Pirelli Cinturato GT3 wet
    ("dd_audi_rs6_v12", "W", 26.0),
    // slick_slick_275_645_18.tyre front and rear, Pirelli P Zero DHF slick 275/645-18
    ("dd_vw_golf8_rsi_dahlbaeck", "S", 27.0),
    // wet_wet_275_645_18.tyre front and rear, Pirelli Cinturato wet 275/645-18
    ("dd_vw_golf8_rsi_dahlbaeck", "W", 26.0),
];

/// The hot pressure window of `compound` on `car`: its ideal ± [`WINDOW_PSI`]; None for a car or compound
/// [`IDEAL_PSI`] does not list.
pub fn target_psi(car: &str, compound: &str) -> Option<TargetPsi> {
    IDEAL_PSI.iter().find(|(c, short, _)| *c == car && *short == compound).map(|(_, _, ideal)| TargetPsi { min: ideal - WINDOW_PSI, max: ideal + WINDOW_PSI })
}

/// What a corner measures on its own; positions on the map's distance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CornerMeasure {
    /// Seconds over the segment
    pub segment_s: f64,
    pub entry_kmh: f64,
    pub exit_kmh: f64,
    pub point_m: Option<f64>,
    pub peak: f64,
    pub release_m: Option<f64>,
    pub min_kmh: f64,
    pub min_at_m: f64,
    pub gear: i32,
    pub coast_s: f64,
    pub pickup_m: Option<f64>,
    pub full_m: Option<f64>,
    pub lat_g: f64,
    /// Steering (deg) per yaw rate (rad/s) in the first, middle and last third of the corner
    pub balance: [Option<f64>; 3],
    pub lock_s: [f64; 4],
    pub spin_s: [f64; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LapMeasures {
    pub distance_m: f64,
    pub top_speed_kmh: f64,
    pub fuel: Fuel,
    pub tyres: Tyres,
    pub brakes: Brakes,
    /// How much less front torque at full pedal above 700 °C than below 500 °C (0.05 = 5 %), when both were seen
    pub fade: Option<f64>,
    /// Steering per yaw rate in the corners by speed band, with the number of metres
    pub bands: [(Option<f64>, u32); 4],
    pub traction: Vec<GearTraction>,
    pub corners: Vec<CornerMeasure>,
}

/// Steering per yaw rate over the points `range` of the grid that turn: None with fewer than five.
fn steer_per_yaw(g: &Grid, points: impl Iterator<Item = usize>) -> Option<f64> {
    let (mut steer, mut yaw, mut n) = (0.0, 0.0, 0);
    for i in points.filter(|i| g.yaw[*i].abs() > YAW) {
        steer += g.steer[i].abs();
        yaw += g.yaw[i].abs();
        n += 1;
    }
    (n >= 5).then(|| steer / yaw)
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn corner(g: &Grid, map: &Map, fit: &Fit, c: usize) -> CornerMeasure {
    let mc = &map.corners[c];
    let len = g.length();
    let lap = |m: f64| fit.to_lap(m).clamp(0.0, len);
    let (a, b) = (lap(mc.start_m) as usize, lap(mc.end_m) as usize);
    let (turn_in, turn_out) = (lap(mc.turn_in_m), lap(mc.turn_out_m));
    let at_map = |i: usize| round(fit.to_map(i as f64), 1);
    let lo = ((turn_in - APEX_MARGIN).max(a as f64)) as usize;
    let hi = ((turn_out + APEX_MARGIN).min(b as f64)) as usize;
    let apex = (lo..=hi.max(lo)).min_by(|x, y| g.speed[*x].total_cmp(&g.speed[*y])).unwrap_or(a);
    // braking: walk back from the hardest braking before the apex to where the pedal was first pressed
    let hardest = (a..=apex).max_by(|x, y| g.brake[*x].total_cmp(&g.brake[*y])).unwrap_or(a);
    let peak = g.brake[hardest];
    let (point, release) = if peak > PEDAL {
        let mut first = hardest;
        while first > a && g.brake[first - 1] > PEDAL {
            first -= 1;
        }
        (Some(first), (a..=apex).rev().find(|i| g.brake[*i] > PEDAL))
    } else {
        (None, None)
    };
    let pickup = (apex..=b).find(|i| g.throttle[*i] > PICKUP);
    let full = (apex..=b).find(|i| g.throttle[*i] > FULL);
    let coast_s = (release.unwrap_or(apex)..pickup.unwrap_or(b)).filter(|i| g.brake[*i] < PEDAL && g.throttle[*i] < PEDAL).map(|i| g.dt(i)).sum();
    let mut lock_s = [0.0; 4];
    let mut spin_s = [0.0; 4];
    for i in a..b {
        for w in 0..4 {
            if g.brake[i] > PEDAL && g.slip[w][i] < -SLIP {
                lock_s[w] += g.dt(i);
            }
            if g.throttle[i] > SPIN_THROTTLE && g.slip[w][i] > SLIP {
                spin_s[w] += g.dt(i);
            }
        }
    }
    let third = (turn_out - turn_in) / 3.0;
    let balance = [0.0, 1.0, 2.0].map(|k| {
        let (x, y) = ((turn_in + k * third) as usize, (turn_in + (k + 1.0) * third) as usize);
        steer_per_yaw(g, x..y.max(x))
    });
    CornerMeasure {
        segment_s: round(g.time_at(lap(mc.end_m)) - g.time_at(lap(mc.start_m)), 3),
        entry_kmh: round(g.speed[a] * 3.6, 1),
        exit_kmh: round(g.speed[b] * 3.6, 1),
        point_m: point.map(at_map),
        peak: round(peak, 1),
        release_m: release.map(at_map),
        min_kmh: round(g.speed[apex] * 3.6, 1),
        min_at_m: at_map(apex),
        gear: g.gear[apex] as i32,
        coast_s: round(coast_s, 2),
        pickup_m: pickup.map(at_map),
        full_m: full.map(at_map),
        lat_g: round((a..=b).map(|i| g.g_lat[i].abs()).fold(0.0, f64::max), 2),
        balance: balance.map(|v| v.map(|v| round(v, 3))),
        lock_s: lock_s.map(|s| round(s, 2)),
        spin_s: spin_s.map(|s| round(s, 2)),
    }
}

/// The numbers of the lap in the file's window [start, end] (seconds), on the grid `g` with its curvature `kappa`,
/// fitted onto `map` by `fit`.
pub fn measure(g: &Grid, kappa: &[f64], data: &Data, start: f64, end: f64, map: &Map, fit: &Fit) -> LapMeasures {
    let wheels = |f: &dyn Fn(usize) -> f64| -> [f64; 4] { std::array::from_fn(|w| round(f(w), 2)) };
    // 0 for a channel the file does not have, so the report never holds an infinity (JSON has none)
    let series_max = |s: &super::ld::Series| s.values[s.range(start, end)].iter().map(|v| *v as f64).reduce(f64::max).unwrap_or(0.0);
    let fuel_range = data.fuel.range(start, end);
    let used: f64 = fuel_range.clone().skip(1).map(|i| (data.fuel.values[i - 1] - data.fuel.values[i]).max(0.0) as f64).sum();
    // brakes: the hottest disc's time above 700 °C, the front's share of the torque while braking, fade at full pedal
    let temps = &data.brake_temp;
    let hot = temps.iter().map(|t| t.values[t.range(start, end)].iter().filter(|v| **v as f64 > HOT).count() as f64 / t.rate).fold(0.0, f64::max);
    let (mut share, mut braking) = (0.0, 0);
    let (mut hot_torque, mut cold_torque) = (Vec::new(), Vec::new());
    for i in data.brake.range(start, end) {
        let t = i as f64 / data.brake.rate;
        let pedal = data.brake.values[i] as f64;
        let torque: [f64; 4] = std::array::from_fn(|w| data.torque[w].at(t));
        let total: f64 = torque.iter().sum();
        let front = torque[0] + torque[1];
        if pedal > 10.0 && data.speed.at(t) > SLOW && total > 0.0 {
            share += front / total;
            braking += 1;
        }
        if pedal > FULL && data.speed.at(t) > 100.0 / 3.6 && data.abs.at(t) < 0.5 && front > 0.0 {
            let disc = (temps[0].at(t) + temps[1].at(t)) / 2.0;
            if disc > HOT {
                hot_torque.push(front);
            } else if disc < COLD {
                cold_torque.push(front);
            }
        }
    }
    let fade = (hot_torque.len() >= 10 && cold_torque.len() >= 10).then(|| round(1.0 - median(hot_torque) / median(cold_torque), 3));
    // balance by speed band and traction by gear, over the grid
    let mut bands = [(None, 0u32); 4];
    for (k, band) in Band::ALL.iter().enumerate() {
        let points: Vec<usize> = (0..g.t.len()).filter(|i| kappa[*i].abs() > KAPPA && Band::of(g.speed[*i] * 3.6) == Some(*band)).collect();
        bands[k] = (steer_per_yaw(g, points.iter().copied()).map(|v| round(v, 3)), points.iter().filter(|i| g.yaw[**i].abs() > YAW).count() as u32);
    }
    let mut gears: Vec<(i32, f64, f64, u32)> = Vec::new();
    for i in (0..g.t.len()).filter(|i| g.throttle[*i] > SPIN_THROTTLE && g.speed[*i] > SLOW && g.gear[*i] >= 1.0) {
        let gear = g.gear[i] as i32;
        let spin = g.slip[2][i].max(g.slip[3][i]) > SLIP;
        let k = match gears.iter().position(|x| x.0 == gear) {
            Some(k) => k,
            None => {
                gears.push((gear, 0.0, 0.0, 0));
                gears.len() - 1
            }
        };
        gears[k].1 += g.dt(i);
        gears[k].2 += if spin { g.dt(i) } else { 0.0 };
        gears[k].3 += 1;
    }
    gears.sort_by_key(|x| x.0);
    LapMeasures {
        distance_m: round(g.length(), 0),
        top_speed_kmh: round(series_max(&data.speed) * 3.6, 1),
        fuel: Fuel { start_l: round(data.fuel.at(start), 2), used_l: round(used, 2) },
        tyres: Tyres {
            pressure_start: wheels(&|w| data.pressure[w].at(start)),
            pressure_avg: wheels(&|w| mean(&data.pressure[w], start, end)),
            pressure_end: wheels(&|w| mean(&data.pressure[w], end - END_S, end)),
            surface_temp_avg: wheels(&|w| data.surface[w].iter().map(|s| mean(s, start, end)).sum::<f64>() / 3.0),
            air_temp_avg: wheels(&|w| mean(&data.tyre_air[w], start, end)),
            target_psi: None,
        },
        brakes: Brakes {
            temp_max: wheels(&|w| series_max(&temps[w])).map(|t| round(t, 0)),
            seconds_above700: round(hot, 1),
            front_share: round(if braking == 0 { 0.0 } else { share / braking as f64 }, 3),
        },
        fade,
        bands,
        traction: gears.into_iter().map(|(gear, time, spin, n)| GearTraction { gear, spin_share: round(if time > 0.0 { spin / time } else { 0.0 }, 3), samples: n }).collect(),
        corners: (0..map.corners.len()).map(|c| corner(g, map, fit, c)).collect(),
    }
}

/// The lap's curves every 5 m.
pub fn trace(lap_id: &str, g: &Grid) -> EngineerTrace {
    let every = |v: &[f64], d: i32| v.iter().step_by(TRACE_STEP).map(|x| round(*x, d)).collect::<Vec<f64>>();
    EngineerTrace {
        lap_id: lap_id.to_string(),
        step_m: TRACE_STEP as f64,
        speed_kmh: g.speed.iter().step_by(TRACE_STEP).map(|v| round(v * 3.6, 1)).collect(),
        throttle: every(&g.throttle, 1),
        brake: every(&g.brake, 1),
        steer_deg: every(&g.steer, 1),
        gear: g.gear.iter().step_by(TRACE_STEP).map(|v| *v as i32).collect(),
    }
}

/// A relative difference `own / reference − 1`.
fn relative(own: Option<f64>, reference: Option<f64>) -> Option<f64> {
    match (own, reference) {
        (Some(a), Some(b)) if b > 0.0 => Some(round(a / b - 1.0, 3)),
        _ => None,
    }
}

fn diff(own: Option<f64>, reference: Option<f64>) -> Option<f64> {
    Some(round(own? - reference?, 1))
}

/// The report of a lap: its numbers against the reference's (when there is one on the same map), the theoretical
/// best, the pressure window of the lap's compound (`target`, None: no judgement) and the findings.
#[allow(clippy::too_many_arguments)]
pub fn report(id: &str, n: u32, lap: &LocalLap, source: TimeSource, m: &LapMeasures, reference: Option<&LapMeasures>, theoretical_best_ms: Option<i64>, target: Option<TargetPsi>) -> EngineerLap {
    let reference = reference.filter(|r| r.corners.len() == m.corners.len());
    let mut findings = Vec::new();
    let corners: Vec<CornerResult> = m
        .corners
        .iter()
        .enumerate()
        .map(|(c, own)| {
            let r = reference.map(|r| &r.corners[c]);
            let brake_vs = diff(own.point_m, r.and_then(|r| r.point_m));
            let throttle_vs = diff(own.pickup_m, r.and_then(|r| r.pickup_m));
            let min_vs = r.map(|r| round(own.min_kmh - r.min_kmh, 1));
            let balance: [Option<f64>; 3] = std::array::from_fn(|k| relative(own.balance[k], r.and_then(|r| r.balance[k])));
            let checks = [
                (brake_vs.is_some_and(|d| d >= 8.0), Finding::BrakeLater),
                (brake_vs.is_some_and(|d| d <= -8.0), Finding::BrakeEarlier),
                (r.is_some_and(|r| r.peak > PEDAL && own.peak < 0.8 * r.peak), Finding::BrakeSofter),
                (own.coast_s > 0.4, Finding::CoastLong),
                (throttle_vs.is_some_and(|d| d >= 10.0), Finding::ThrottleLater),
                (throttle_vs.is_some_and(|d| d <= -10.0), Finding::ThrottleEarlier),
                (min_vs.is_some_and(|d| d <= -4.0), Finding::MinSpeedLower),
                (r.is_some_and(|r| own.exit_kmh - r.exit_kmh <= -4.0), Finding::ExitSlower),
                (own.lock_s[0].max(own.lock_s[1]) > 0.1, Finding::LockFront),
                (own.lock_s[2].max(own.lock_s[3]) > 0.1, Finding::LockRear),
                (own.spin_s[2].max(own.spin_s[3]) > 0.1, Finding::SpinRear),
                (balance[0].is_some_and(|b| b > 0.15), Finding::UndersteerEntry),
                (balance[2].is_some_and(|b| b < -0.15), Finding::OversteerExit),
            ];
            findings.extend(checks.into_iter().filter(|(hit, _)| *hit).map(|(_, f)| f));
            CornerResult {
                n: c as u32 + 1,
                dt_s: r.map(|r| round(own.segment_s - r.segment_s, 3)),
                entry_kmh: own.entry_kmh,
                brake: CornerBrake { point_m: own.point_m, vs_ref_m: brake_vs, peak: own.peak, release_m: own.release_m },
                min_speed: MinSpeed { kmh: own.min_kmh, at_m: own.min_at_m, vs_ref_kmh: min_vs, gear: own.gear },
                coast_s: own.coast_s,
                throttle: CornerThrottle { pickup_m: own.pickup_m, full_m: own.full_m, vs_ref_m: throttle_vs },
                exit_kmh: own.exit_kmh,
                lat_g_max: own.lat_g,
                balance: CornerBalance { entry: balance[0], mid: balance[1], exit: balance[2] },
                lock_s: own.lock_s,
                spin_s: own.spin_s,
            }
        })
        .collect();
    let tyres = Tyres { target_psi: target.clone(), ..m.tyres.clone() };
    let lap_checks = [
        (target.as_ref().is_some_and(|t| tyres.pressure_end.iter().any(|p| *p > t.max)), Finding::PressureHigh),
        (target.as_ref().is_some_and(|t| tyres.pressure_end.iter().any(|p| *p < t.min)), Finding::PressureLow),
        (m.brakes.temp_max.iter().any(|t| *t > VERY_HOT), Finding::BrakeHot),
        (m.fade.is_some_and(|f| f >= 0.04), Finding::BrakeFade),
        (reference.is_some_and(|r| m.fuel.start_l >= r.fuel.start_l + 20.0), Finding::FuelHeavy),
    ];
    findings.extend(lap_checks.into_iter().filter(|(hit, _)| *hit).map(|(_, f)| f));
    findings.sort();
    findings.dedup();
    EngineerLap {
        id: id.to_string(),
        n,
        lap_time_ms: Some(lap.lap_time_ms),
        valid: lap.valid,
        pit: lap.pit,
        time_source: source,
        distance_m: m.distance_m,
        top_speed_kmh: m.top_speed_kmh,
        theoretical_best_ms: reference.and(theoretical_best_ms),
        fuel: m.fuel.clone(),
        tyres,
        brakes: m.brakes.clone(),
        balance: Band::ALL.iter().enumerate().map(|(k, band)| BandBalance { band: *band, relative: relative(m.bands[k].0, reference.and_then(|r| r.bands[k].0)), samples: m.bands[k].1 }).collect(),
        traction: m.traction.clone(),
        corners,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::super::corners::{curvature, detect, fit, Map};
    use super::super::cut::{grid, Data};
    use super::super::ld::Ld;
    use super::super::testkit::{self, Drive};
    use super::*;
    use std::io::Cursor;

    struct Lab {
        session: testkit::Session,
        data: Data,
    }

    fn lab(drive: Drive) -> Lab {
        let session = testkit::session(&drive);
        let mut r = Cursor::new(testkit::write_ld("04/10/2026", "23:40:23", &session.chans));
        let ld = Ld::read(&mut r).unwrap();
        let data = Data::read(&ld, &mut r).unwrap();
        Lab { session, data }
    }

    /// Both timed laps measured on the map of the first.
    fn measured(lab: &Lab) -> (Map, [LapMeasures; 2]) {
        let grids = lab.session.laps.map(|(a, b)| grid(&lab.data, a, b).unwrap());
        let kappas = grids.each_ref().map(|g| curvature(&g.speed, &g.yaw));
        let map = Map::from_lap(&detect(&kappas[0]), &grids[0].speed, 1);
        let m = std::array::from_fn(|k| {
            let f = fit(&detect(&kappas[k]), &map);
            measure(&grids[k], &kappas[k], &lab.data, lab.session.laps[k].0, lab.session.laps[k].1, &map, &f)
        });
        (map, m)
    }

    fn lap(n: usize, lab: &Lab) -> LocalLap {
        testkit::local_laps(&lab.session, 0)[n].clone()
    }

    #[test]
    fn a_later_braking_point_shows_its_time_in_that_corner_only() {
        let lab = lab(Drive { late: Some(2), lock: false });
        let (map, [one, two]) = measured(&lab);
        let r = report("s#2", 2, &lap(1, &lab), TimeSource::App, &two, Some(&one), None, None);
        let dts: Vec<f64> = r.corners.iter().map(|c| c.dt_s.unwrap()).collect();
        // corner 3 (the hairpin) is braked for harder and later: faster there, equal elsewhere (within what the
        // 20 Hz yaw rate allows: it places a turn-in to ±1.5 m, 0.02 s at 75 m/s)
        assert!(dts[2] < -0.2, "{dts:?}");
        for (c, dt) in dts.iter().enumerate().filter(|(c, _)| *c != 2) {
            assert!(dt.abs() <= 0.02, "corner {}: {dt}", c + 1);
        }
        let lap_dt = (lap(1, &lab).lap_time_ms - lap(0, &lab).lap_time_ms) as f64 / 1000.0;
        assert!((dts.iter().sum::<f64>() - lap_dt).abs() < 0.02, "{dts:?} against {lap_dt}");
        // its braking point: out of the R 150 m (47.4 m/s), 5 m/s² up and 12 m/s² down to 24.5 m/s on the 500 m
        // straight, so braking 195.6 m before the arc; the late lap reaches the 75 m/s cap and brakes at 21.6 m/s²
        // from there, 116.3 m before it
        let hairpin = &r.corners[2];
        let arc = testkit::arcs()[2].0;
        let own = one.corners[2].point_m.unwrap();
        assert!((own - (arc - 195.6)).abs() < 2.0, "{own} against {}", arc - 195.6);
        assert!((hairpin.brake.vs_ref_m.unwrap() - 79.3).abs() < 2.0, "{:?}", hairpin.brake);
        assert_eq!(hairpin.brake.peak, 90.0);
        assert!((hairpin.brake.release_m.unwrap() - arc).abs() < 3.0);
        assert!(r.findings.contains(&Finding::BrakeLater));
        assert!(map.corners[2].apex_m >= map.corners[2].turn_in_m);
        assert!(r.corners.iter().enumerate().all(|(c, x)| c == 2 || x.brake.vs_ref_m.is_some_and(|d| d.abs() <= 1.5)), "{:?}", r.corners);
    }

    #[test]
    fn measures_a_corner() {
        let lab = lab(Drive { late: None, lock: false });
        let (_, [one, _]) = measured(&lab);
        let (a, b, k) = testkit::arcs()[0];
        let c = &one.corners[0];
        // R 60 m at 15 m/s²: 30 m/s = 108 km/h in 3rd (the test car shifts every 13 m/s)
        assert!((c.min_kmh - 108.0).abs() < 0.5 && c.gear == 3, "{c:?}");
        assert!(c.min_at_m >= a - 1.0 && c.min_at_m <= b);
        // the lap starts out of the last corner (R 200 m: 54.8 m/s); throttle 30 % in the arc, full after it
        assert!((c.entry_kmh - (15.0f64 * 200.0).sqrt() * 3.6).abs() < 1.0, "{}", c.entry_kmh);
        assert!((c.pickup_m.unwrap() - b).abs() < 2.0 && (c.full_m.unwrap() - c.pickup_m.unwrap()).abs() <= 1.0, "{c:?}");
        assert_eq!(c.coast_s, 0.0);
        assert!((c.lat_g - 15.0 / testkit::G).abs() < 0.05, "{}", c.lat_g);
        assert!(c.lock_s == [0.0; 4] && c.spin_s == [0.0; 4]);
        // steering per yaw: 13 × atan(2.7 / R) in degrees per v / R
        let base = 13.0 * (2.7 * k).atan().to_degrees() / (30.0 / 60.0);
        assert!(c.balance.iter().all(|b| (b.unwrap() - base).abs() < 0.05 * base), "{:?} against {base}", c.balance);
        // the segments add up to the lap
        let total: f64 = one.corners.iter().map(|c| c.segment_s).sum();
        let (start, end) = lab.session.laps[0];
        assert!((total - (end - start)).abs() < 0.02);
    }

    #[test]
    fn measures_fuel_tyres_and_brakes_of_a_lap() {
        let lab = lab(Drive { late: None, lock: false });
        let (_, [one, _]) = measured(&lab);
        let len = testkit::track_length();
        // 1 l per km, the lap starts after the out lap
        assert!((one.fuel.start_l - (60.0 - len / 1000.0)).abs() < 0.02 && (one.fuel.used_l - len / 1000.0).abs() < 0.02, "{:?}", one.fuel);
        assert_eq!(one.tyres.pressure_start, [27.5, 27.6, 27.7, 27.8]);
        assert_eq!(one.tyres.pressure_end, one.tyres.pressure_start);
        assert_eq!((one.tyres.surface_temp_avg, one.tyres.air_temp_avg), ([86.0; 4], [95.0; 4]));
        assert_eq!(one.brakes.temp_max, [750.0, 750.0, 300.0, 300.0]);
        assert_eq!(one.brakes.front_share, 0.69);
        assert_eq!(one.fade, None, "no full pedal in the test car");
        assert!((one.distance_m - len).abs() <= 2.0 && (one.top_speed_kmh - 270.0).abs() < 0.5);
        // time above 700 °C is the time braking: four stops
        assert!(one.brakes.seconds_above700 > 4.0 && one.brakes.seconds_above700 < 20.0, "{}", one.brakes.seconds_above700);
        // no wheel spins; throttle samples in every gear the car accelerates through
        assert!(one.traction.iter().all(|t| t.spin_share == 0.0 && t.samples > 0) && one.traction.len() >= 4, "{:?}", one.traction);
        // steering per yaw in the bands the corners fall in: the hairpin (88 km/h), the 60 m (108), 150 m (171), 200 m (197)
        let samples: Vec<u32> = one.bands.iter().map(|b| b.1).collect();
        assert!(samples[0] > 100 && samples[1] > 100 && samples[2] > 50 && samples[3] == 0, "{samples:?}");
    }

    #[test]
    fn finds_locking_and_compares_with_the_reference() {
        let lab = lab(Drive { late: Some(2), lock: true });
        let (_, [one, two]) = measured(&lab);
        let slick = target_psi("dd_bmw_m3_e46_rennsport", "S");
        let r = report("s#2", 2, &lap(1, &lab), TimeSource::App, &two, Some(&one), Some(400_000), slick.clone());
        let hairpin = &r.corners[2];
        assert!(hairpin.lock_s[0] > 0.5 && hairpin.lock_s[1] > 0.5 && hairpin.lock_s[2] == 0.0, "{:?}", hairpin.lock_s);
        assert!(r.findings.contains(&Finding::LockFront));
        // the slick's window: 27.5 to 27.8 psi at the end is above 27.5
        assert!(r.findings.contains(&Finding::PressureHigh) && !r.findings.contains(&Finding::PressureLow));
        assert_eq!(r.tyres.target_psi, slick);
        assert_eq!(r.theoretical_best_ms, Some(400_000));
        assert!(r.balance.iter().take(3).all(|b| b.relative.is_some_and(|x| x.abs() < 0.01)), "{:?}", r.balance);
        assert_eq!(r.balance[3].relative, None, "no corner above 240 km/h");
        // without a reference: no comparisons, no theoretical best
        let alone = report("s#1", 1, &lap(0, &lab), TimeSource::App, &one, None, Some(400_000), None);
        assert!(alone.corners.iter().all(|c| c.dt_s.is_none() && c.brake.vs_ref_m.is_none() && c.balance.entry.is_none()));
        assert_eq!((alone.theoretical_best_ms, alone.tyres.target_psi.clone()), (None, None));
        assert!(alone.findings.iter().all(|f| matches!(f, Finding::CoastLong | Finding::LockFront | Finding::LockRear | Finding::SpinRear | Finding::BrakeHot | Finding::BrakeFade)));
    }

    #[test]
    fn the_target_window_is_the_ideal_of_the_compound_fitted_half_a_psi_either_way() {
        let window = |car: &str, compound: &str| target_psi(car, compound).map(|t| (t.min, t.max));
        // the packages' idealPressure (car.py, 2026-10-29)
        assert_eq!(window("dd_porsche_gt2rs_mr", "TRS"), Some((29.5, 30.5)));
        assert_eq!(window("dd_porsche_gt2rs_mr", "C2R"), Some((29.5, 30.5)));
        assert_eq!(window("dd_porsche_gt2rs_mr", "S"), Some((26.5, 27.5)));
        assert_eq!(window("dd_bmw_m3_e46_clubsport", "A052"), Some((28.5, 29.5)));
        assert_eq!(window("dd_bmw_m3_e46_gt3", "TRS"), Some((28.5, 29.5)));
        assert_eq!(window("dd_bmw_m3_e46_gt3", "S"), Some((26.5, 27.5)));
        assert_eq!(window("dd_bmw_m3_e46_rennsport", "W"), Some((25.5, 26.5)));
        assert_eq!(window("dd_vw_golf8_rsi_dahlbaeck", "S"), Some((26.5, 27.5)));
        // a compound the car does not have, a car the table does not know, a Kunos car: no judgement
        assert_eq!(window("dd_bmw_m3_e46_rennsport", "TRS"), None);
        assert_eq!(window("dd_test_car", "S"), None);
        assert_eq!(window("ks_bmw_m4_gt3", "S"), None);
    }

    #[test]
    fn judges_the_hot_pressures_by_the_compound_s_window() {
        let lab = lab(Drive { late: None, lock: false });
        let (_, [one, _]) = measured(&lab);
        let at = |psi: [f64; 4]| LapMeasures { tyres: Tyres { pressure_end: psi, ..one.tyres.clone() }, ..one.clone() };
        let findings = |psi: [f64; 4], compound: &str| {
            let r = report("s#1", 1, &lap(0, &lab), TimeSource::App, &at(psi), None, None, target_psi("dd_bmw_m3_e46_rennsport", compound));
            r.findings.into_iter().filter(|f| matches!(f, Finding::PressureHigh | Finding::PressureLow)).collect::<Vec<_>>()
        };
        // a Rennsport on slicks at Joe's Porsche pressures is too high
        assert_eq!(findings([29.8, 29.6, 29.8, 29.6], "S"), [Finding::PressureHigh]);
        // on wets 26 psi is the ideal, 27.5 too high, 25.4 too low
        assert_eq!(findings([26.0, 26.1, 25.9, 26.0], "W"), []);
        assert_eq!(findings([27.5, 26.1, 25.9, 26.0], "W"), [Finding::PressureHigh]);
        assert_eq!(findings([26.0, 26.1, 25.4, 26.0], "W"), [Finding::PressureLow]);
        // the window's edges are in it
        assert_eq!(findings([26.5, 25.5, 26.5, 25.5], "W"), []);
    }

    #[test]
    fn judges_hot_brakes_fade_fuel_and_coasting() {
        let lab = lab(Drive { late: None, lock: false });
        let (_, [one, _]) = measured(&lab);
        let mut hot = one.clone();
        hot.brakes.temp_max[1] = 812.0;
        hot.fade = Some(0.045);
        hot.fuel.start_l += 25.0;
        hot.corners[1].coast_s = 0.5;
        hot.corners[0].balance = one.corners[0].balance.map(|b| b.map(|b| b * 1.2));
        hot.corners[3].balance[2] = one.corners[3].balance[2].map(|b| b * 0.8);
        hot.corners[1].min_kmh -= 5.0;
        hot.corners[1].exit_kmh -= 4.0;
        hot.corners[0].peak = 70.0;
        let r = report("s#1", 1, &lap(0, &lab), TimeSource::App, &hot, Some(&one), None, None);
        for f in [Finding::BrakeHot, Finding::BrakeFade, Finding::FuelHeavy, Finding::CoastLong, Finding::UndersteerEntry, Finding::OversteerExit, Finding::MinSpeedLower, Finding::ExitSlower, Finding::BrakeSofter] {
            assert!(r.findings.contains(&f), "{f:?} in {:?}", r.findings);
        }
        assert_eq!(r.corners[1].min_speed.vs_ref_kmh, Some(-5.0));
        let json = serde_json::to_value(&r.findings).unwrap();
        assert_eq!(json[0], "brake_softer");
    }

    #[test]
    fn traces_every_five_metres() {
        let lab = lab(Drive { late: None, lock: false });
        let (a, b) = lab.session.laps[0];
        let g = grid(&lab.data, a, b).unwrap();
        let t = trace("s#1", &g);
        assert_eq!(t.speed_kmh.len(), g.t.len().div_ceil(5));
        assert_eq!((t.step_m, t.speed_kmh[0], t.gear[0]), (5.0, round(g.speed[0] * 3.6, 1), g.gear[0] as i32));
        assert_eq!((t.throttle.len(), t.brake.len(), t.steer_deg.len(), t.gear.len()), (t.speed_kmh.len(), t.speed_kmh.len(), t.speed_kmh.len(), t.speed_kmh.len()));
    }
}
