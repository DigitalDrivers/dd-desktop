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
/// A point this close (m) to its segment's start or end sits on the edge (research/2026-10-21-renningenieur-coach/
/// audit/REPORT.md §2e).
const EDGE_M: f64 = 2.0;
/// The pressures at a lap's end: the mean of its last 20 s.
const END_S: f64 = 20.0;
const HOT: f64 = 700.0;
const VERY_HOT: f64 = 800.0;
const COLD: f64 = 500.0;
const TRACE_STEP: usize = 5;

/// A corner's finding against the reference needs the corner to have cost this much (s), and a difference beyond
/// both its threshold and [`SPREAD_K`] times the driver's own spread there ([`Spread`]); the audit of Joe's laps
/// (research/2026-10-21-renningenieur-coach/audit/REPORT.md sections 2b and 7) found the fixed thresholds inside his
/// own lap-to-lap spread, so nearly every finding was noise.
const COST_S: f64 = 0.05;
/// 2·√2: a difference of two laps spreads √2 times one lap's σ, and 2 σ of that is beyond noise.
const SPREAD_K: f64 = 2.0 * std::f64::consts::SQRT_2;
const BRAKE_M: f64 = 8.0;
const THROTTLE_M: f64 = 10.0;
const SPEED_KMH: f64 = 4.0;
/// Coasting and wheelspin are judged against the reference's same corner, by this much more (s).
const COAST_S: f64 = 0.4;
const SPIN_S: f64 = 0.1;
/// Locking is judged by the locked seconds of the axle's worse wheel as a share of the lap's braking time:
/// tlm.py's limits (lock_front_high 3 %, lock_rear 0.5 %, the latter an assumption there).
const LOCK_FRONT: f64 = 0.03;
const LOCK_REAR: f64 = 0.005;
/// Balance is judged from the speed bands below 240 km/h with this many samples on both laps, by this much more or
/// less steering for the same yaw, against a reference that started within 5 l of fuel (REPORT.md section 3: the
/// 240+ band and the corners' own thirds are noise).
const BALANCE: f64 = 0.25;
const BALANCE_SAMPLES: u32 = 1000;
const BALANCE_FUEL_L: f64 = 5.0;

/// The driver's own standard deviation at a corner over the pace laps, without the lap judged: braking point,
/// throttle pick-up, minimum speed, exit speed (the order of [`CornerMeasure::pace_numbers`]); None with fewer than 3.
pub type Spread = [Option<f64>; 4];

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

impl CornerMeasure {
    /// The numbers whose spread over the pace laps the corner's findings are judged by.
    pub fn pace_numbers(&self) -> [Option<f64>; 4] {
        [self.point_m, self.pickup_m, Some(self.min_kmh), Some(self.exit_kmh)]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LapMeasures {
    pub distance_m: f64,
    /// Seconds with the brake above 5 %; 0 in the references of apps before 0.26.0
    #[serde(default)]
    pub braking_s: f64,
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
    let slowest = |lo: usize, hi: usize| (lo..=hi.max(lo)).min_by(|x, y| g.speed[*x].total_cmp(&g.speed[*y])).unwrap_or(lo);
    let lo = ((turn_in - APEX_MARGIN).max(a as f64)) as usize;
    let hi = ((turn_out + APEX_MARGIN).min(b as f64)) as usize;
    let mut apex = slowest(lo, hi);
    // a slowest point on the segment's edge is where the segment is cut (the car still slowing for the next corner,
    // or still accelerating out of the one before): the slowest point of the corner's own turn instead
    if apex as f64 <= a as f64 + EDGE_M || apex as f64 >= b as f64 - EDGE_M {
        apex = slowest(turn_in.max(a as f64 + EDGE_M) as usize, turn_out.min(b as f64 - EDGE_M) as usize);
    }
    // braking: walk back from the hardest braking before the apex to where the pedal was first pressed, also into
    // the segment before (the braking for a corner often begins there); one pressed since the lap's start is unknown
    let hardest = (a..=apex).max_by(|x, y| g.brake[*x].total_cmp(&g.brake[*y])).unwrap_or(a);
    let peak = g.brake[hardest];
    let (point, release) = if peak > PEDAL {
        let mut first = hardest;
        while first > 0 && g.brake[first - 1] > PEDAL {
            first -= 1;
        }
        (Some(first).filter(|first| *first > 0), (a..=apex).rev().find(|i| g.brake[*i] > PEDAL))
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
        braking_s: round((0..g.t.len()).filter(|i| g.brake[*i] > PEDAL).map(|i| g.dt(i)).sum(), 2),
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
/// best, the pressure window of the lap's compound (`target`, None: no judgement) and the findings. A corner's
/// findings come only against the reference, where the corner cost at least [`COST_S`]: braking point, throttle
/// pick-up, minimum and exit speed beyond their threshold and the driver's own spread (`spread`, per corner; none
/// without it), coasting and rear wheelspin beyond the reference's at that corner. Locking and balance are findings of
/// the lap.
#[allow(clippy::too_many_arguments)]
pub fn report(id: &str, n: u32, lap: &LocalLap, source: TimeSource, m: &LapMeasures, reference: Option<&LapMeasures>, spread: &[Spread], theoretical_best_ms: Option<i64>, target: Option<TargetPsi>) -> EngineerLap {
    let reference = reference.filter(|r| r.corners.len() == m.corners.len());
    let mut findings = Vec::new();
    let corners: Vec<CornerResult> = m
        .corners
        .iter()
        .enumerate()
        .map(|(c, own)| {
            let r = reference.map(|r| &r.corners[c]);
            let dt_s = r.map(|r| round(own.segment_s - r.segment_s, 3));
            let brake_vs = diff(own.point_m, r.and_then(|r| r.point_m));
            let throttle_vs = diff(own.pickup_m, r.and_then(|r| r.pickup_m));
            let min_vs = r.map(|r| round(own.min_kmh - r.min_kmh, 1));
            let exit_vs = r.map(|r| round(own.exit_kmh - r.exit_kmh, 1));
            let balance: [Option<f64>; 3] = std::array::from_fn(|k| relative(own.balance[k], r.and_then(|r| r.balance[k])));
            let cost = dt_s.is_some_and(|dt| dt >= COST_S);
            // the difference, when it is beyond the threshold and the driver's own spread there
            let beyond = |d: Option<f64>, threshold: f64, k: usize| d.filter(|d| spread.get(c).and_then(|s| s[k]).is_some_and(|sigma| d.abs() >= threshold.max(SPREAD_K * sigma)));
            let rear_spin = |x: &CornerMeasure| x.spin_s[2].max(x.spin_s[3]);
            let checks = [
                (beyond(brake_vs, BRAKE_M, 0).is_some_and(|d| d > 0.0), Finding::BrakeLater),
                (beyond(brake_vs, BRAKE_M, 0).is_some_and(|d| d < 0.0), Finding::BrakeEarlier),
                (r.is_some_and(|r| own.coast_s - r.coast_s > COAST_S), Finding::CoastLong),
                (beyond(throttle_vs, THROTTLE_M, 1).is_some_and(|d| d > 0.0), Finding::ThrottleLater),
                (beyond(throttle_vs, THROTTLE_M, 1).is_some_and(|d| d < 0.0), Finding::ThrottleEarlier),
                (beyond(min_vs, SPEED_KMH, 2).is_some_and(|d| d < 0.0), Finding::MinSpeedLower),
                (beyond(exit_vs, SPEED_KMH, 3).is_some_and(|d| d < 0.0), Finding::ExitSlower),
                (r.is_some_and(|r| rear_spin(own) - rear_spin(r) > SPIN_S), Finding::SpinRear),
            ];
            if cost {
                findings.extend(checks.into_iter().filter(|(hit, _)| *hit).map(|(_, f)| f));
            }
            CornerResult {
                n: c as u32 + 1,
                dt_s,
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
    // the locked seconds of the axle's worse wheel over the lap, as a share of its braking time
    let locked = |wheels: [usize; 2]| wheels.map(|w| corners.iter().map(|c| c.lock_s[w]).sum::<f64>()).into_iter().fold(0.0, f64::max);
    let lock_share = |wheels: [usize; 2]| (m.braking_s > 0.0).then(|| locked(wheels) / m.braking_s);
    let balance: Vec<BandBalance> = Band::ALL.iter().enumerate().map(|(k, band)| BandBalance { band: *band, relative: relative(m.bands[k].0, reference.and_then(|r| r.bands[k].0)), samples: m.bands[k].1 }).collect();
    let like_fuel = reference.is_some_and(|r| (m.fuel.start_l - r.fuel.start_l).abs() <= BALANCE_FUEL_L);
    let judged_bands: Vec<f64> = balance
        .iter()
        .enumerate()
        .filter(|(k, b)| like_fuel && b.band != Band::B240 && b.samples >= BALANCE_SAMPLES && reference.is_some_and(|r| r.bands[*k].1 >= BALANCE_SAMPLES))
        .filter_map(|(_, b)| b.relative)
        .collect();
    let tyres = Tyres { target_psi: target.clone(), ..m.tyres.clone() };
    let lap_checks = [
        (target.as_ref().is_some_and(|t| tyres.pressure_end.iter().any(|p| *p > t.max)), Finding::PressureHigh),
        (target.as_ref().is_some_and(|t| tyres.pressure_end.iter().any(|p| *p < t.min)), Finding::PressureLow),
        (m.brakes.temp_max.iter().any(|t| *t > VERY_HOT), Finding::BrakeHot),
        (m.fade.is_some_and(|f| f >= 0.04), Finding::BrakeFade),
        (reference.is_some_and(|r| m.fuel.start_l >= r.fuel.start_l + 20.0), Finding::FuelHeavy),
        (lock_share([0, 1]).is_some_and(|s| s > LOCK_FRONT), Finding::LockFront),
        (lock_share([2, 3]).is_some_and(|s| s > LOCK_REAR), Finding::LockRear),
        (judged_bands.iter().any(|b| *b >= BALANCE), Finding::Understeer),
        (judged_bands.iter().any(|b| *b <= -BALANCE), Finding::Oversteer),
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
        braking_s: Some(m.braking_s),
        top_speed_kmh: m.top_speed_kmh,
        theoretical_best_ms: reference.and(theoretical_best_ms),
        fuel: m.fuel.clone(),
        tyres,
        brakes: m.brakes.clone(),
        balance,
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
        let r = report("s#2", 2, &lap(1, &lab), TimeSource::App, &two, Some(&one), &TIGHT, None, None);
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
        // braking later gained time there: nothing to say
        assert!(!r.findings.contains(&Finding::BrakeLater), "{:?}", r.findings);
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
        let r = report("s#2", 2, &lap(1, &lab), TimeSource::App, &two, Some(&one), &TIGHT, Some(400_000), slick.clone());
        let hairpin = &r.corners[2];
        assert!(hairpin.lock_s[0] > 0.5 && hairpin.lock_s[1] > 0.5 && hairpin.lock_s[2] == 0.0, "{:?}", hairpin.lock_s);
        // the fronts lock for more than 3 % of the lap's braking time: a finding of the lap
        let front: f64 = (0..2).map(|w| r.corners.iter().map(|c| c.lock_s[w]).sum::<f64>()).fold(0.0, f64::max);
        assert!(r.braking_s.is_some_and(|b| b > 4.0 && front / b > 0.03), "{front} s of {:?} s", r.braking_s);
        assert!(r.findings.contains(&Finding::LockFront) && !r.findings.contains(&Finding::LockRear));
        // the slick's window: 27.5 to 27.8 psi at the end is above 27.5
        assert!(r.findings.contains(&Finding::PressureHigh) && !r.findings.contains(&Finding::PressureLow));
        assert_eq!(r.tyres.target_psi, slick);
        assert_eq!(r.theoretical_best_ms, Some(400_000));
        assert!(r.balance.iter().take(3).all(|b| b.relative.is_some_and(|x| x.abs() < 0.01)), "{:?}", r.balance);
        assert_eq!(r.balance[3].relative, None, "no corner above 240 km/h");
        // without a reference: no comparisons, no theoretical best
        let alone = report("s#1", 1, &lap(0, &lab), TimeSource::App, &one, None, &TIGHT, Some(400_000), None);
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
            let r = report("s#1", 1, &lap(0, &lab), TimeSource::App, &at(psi), None, &[], None, target_psi("dd_bmw_m3_e46_rennsport", compound));
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

    /// The driver's own spread at every corner: none to speak of (the thresholds decide).
    const TIGHT: [Spread; 4] = [[Some(0.5); 4]; 4];

    #[test]
    fn judges_hot_brakes_fade_and_fuel() {
        let lab = lab(Drive { late: None, lock: false });
        let (_, [one, _]) = measured(&lab);
        let mut hot = one.clone();
        hot.brakes.temp_max[1] = 812.0;
        hot.fade = Some(0.045);
        hot.fuel.start_l += 25.0;
        let r = report("s#1", 1, &lap(0, &lab), TimeSource::App, &hot, Some(&one), &TIGHT, None, None);
        assert_eq!(r.findings, [Finding::BrakeHot, Finding::BrakeFade, Finding::FuelHeavy]);
    }

    #[test]
    fn a_corner_s_finding_needs_its_time_lost_and_more_than_the_driver_s_own_spread() {
        let lab = lab(Drive { late: Some(2), lock: false });
        let (_, [one, two]) = measured(&lab);
        // lap 1 brakes 79 m earlier than the reference (lap 2) for the hairpin and loses 0.2 s there
        let findings = |spread: &[Spread]| report("s#1", 1, &lap(0, &lab), TimeSource::App, &one, Some(&two), spread, None, None).findings;
        assert!(findings(&TIGHT).contains(&Finding::BrakeEarlier));
        // a driver whose own braking points there scatter by 30 m: 79 m is inside 2·√2·30 = 85 m
        let mut wide = TIGHT;
        wide[2][0] = Some(30.0);
        assert!(!findings(&wide).contains(&Finding::BrakeEarlier));
        wide[2][0] = Some(25.0);
        assert!(findings(&wide).contains(&Finding::BrakeEarlier));
        // fewer than 3 own pace laps: no corner finding at all
        let mut few = TIGHT;
        few[2] = [None; 4];
        assert!(!findings(&few).contains(&Finding::BrakeEarlier));
        assert_eq!(findings(&[]), Vec::<Finding>::new());
        // a corner that costs nothing says nothing, however far off: corner 1 braked 20 m later, 10 km/h slower out,
        // 0.5 s more coasting
        let mut off = one.clone();
        off.corners[0].point_m = Some(off.corners[0].point_m.unwrap_or(500.0) + 20.0);
        off.corners[0].exit_kmh -= 10.0;
        off.corners[0].min_kmh -= 10.0;
        off.corners[0].coast_s += 0.5;
        off.corners[0].spin_s[3] += 0.5;
        let r = report("s#1", 1, &lap(0, &lab), TimeSource::App, &off, Some(&one), &TIGHT, None, None);
        assert_eq!(r.findings, [], "{:?}", r.corners[0]);
        // the same corner 0.06 s slower: each of them
        off.corners[0].segment_s += 0.06;
        let r = report("s#1", 1, &lap(0, &lab), TimeSource::App, &off, Some(&one), &TIGHT, None, None);
        assert_eq!(r.corners[0].dt_s, Some(0.06));
        for f in [Finding::MinSpeedLower, Finding::ExitSlower, Finding::CoastLong, Finding::SpinRear] {
            assert!(r.findings.contains(&f), "{f:?} in {:?}", r.findings);
        }
        // coasting and spinning count against the reference's same corner: as much as there is nothing
        let mut both = one.clone();
        both.corners[0].coast_s += 0.45;
        both.corners[0].spin_s[3] += 0.45;
        let r = report("s#1", 1, &lap(0, &lab), TimeSource::App, &off, Some(&both), &TIGHT, None, None);
        assert!(!r.findings.contains(&Finding::CoastLong) && !r.findings.contains(&Finding::SpinRear), "{:?}", r.findings);
        // and without a reference nothing is relative
        assert_eq!(report("s#1", 1, &lap(0, &lab), TimeSource::App, &off, None, &TIGHT, None, None).findings, []);
    }

    #[test]
    fn never_says_brake_softer_or_a_corner_s_balance() {
        let lab = lab(Drive { late: None, lock: false });
        let (_, [one, _]) = measured(&lab);
        let mut soft = one.clone();
        soft.corners[0].peak = 50.0;
        soft.corners[0].balance = one.corners[0].balance.map(|b| b.map(|b| b * 1.5));
        soft.corners[3].balance[2] = one.corners[3].balance[2].map(|b| b * 0.5);
        soft.corners[0].segment_s += 0.1;
        soft.corners[3].segment_s += 0.1;
        let r = report("s#1", 1, &lap(0, &lab), TimeSource::App, &soft, Some(&one), &TIGHT, None, None);
        assert_eq!(r.findings, []);
        assert!(r.corners[0].balance.entry.is_some_and(|b| b > 0.4), "the numbers stay: {:?}", r.corners[0].balance);
    }

    #[test]
    fn locking_is_judged_by_its_share_of_the_lap_s_braking_time() {
        let lab = lab(Drive { late: None, lock: false });
        let (_, [one, _]) = measured(&lab);
        let braking = one.braking_s;
        assert!(braking > 4.0 && braking < 20.0, "four stops: {braking} s");
        let findings = |front: f64, rear: f64| {
            let mut m = one.clone();
            // the worse wheel of each axle, spread over two corners
            m.corners[0].lock_s = [front / 2.0, 0.0, rear / 2.0, 0.0];
            m.corners[2].lock_s = [front / 2.0, front / 4.0, rear / 2.0, 0.0];
            report("s#1", 1, &lap(0, &lab), TimeSource::App, &m, None, &[], None, None).findings
        };
        assert_eq!(findings(0.031 * braking, 0.0), [Finding::LockFront]);
        assert_eq!(findings(0.029 * braking, 0.0), []);
        assert_eq!(findings(0.0, 0.006 * braking), [Finding::LockRear]);
        assert_eq!(findings(0.0, 0.004 * braking), []);
    }

    #[test]
    fn balance_is_judged_by_the_speed_bands_below_240_against_a_reference_of_like_fuel() {
        let lab = lab(Drive { late: None, lock: false });
        let (_, [one, _]) = measured(&lab);
        let mut reference = one.clone();
        reference.bands = [(Some(2.0), 1500), (Some(2.0), 1500), (Some(2.0), 1500), (Some(2.0), 1500)];
        let findings = |band: usize, value: f64, samples: u32, fuel: f64| {
            let mut m = one.clone();
            m.bands = reference.bands;
            m.bands[band] = (Some(value), samples);
            m.fuel.start_l = reference.fuel.start_l + fuel;
            report("s#1", 1, &lap(0, &lab), TimeSource::App, &m, Some(&reference), &[], None, None).findings
        };
        // 25 % more steering for the same yaw: understeer, 25 % less: oversteer
        assert_eq!(findings(0, 2.5, 1200, 0.0), [Finding::Understeer]);
        assert_eq!(findings(2, 1.5, 1000, 4.9), [Finding::Oversteer]);
        // within the noise, too few samples, the 240+ band, a reference 5 l or more lighter or heavier: nothing
        assert_eq!(findings(1, 2.48, 1200, 0.0), []);
        assert_eq!(findings(1, 2.6, 999, 0.0), []);
        assert_eq!(findings(3, 3.0, 5000, 0.0), []);
        assert_eq!(findings(0, 2.6, 1200, -5.1), []);
        let json = serde_json::to_value([Finding::Understeer, Finding::Oversteer]).unwrap();
        assert_eq!(json, serde_json::json!(["understeer", "oversteer"]));
    }

    /// A 400 m lap by hand, a point every metre: corner 1 turns from 60 to 100 m at 30 m/s, the car accelerates to
    /// 31 m/s and brakes from 105 m at 3 m/s² for corner 2 (turning from 240 to 280 m at 15 m/s), whose segment
    /// starts at 120 m: the braking begins in corner 1's segment, and corner 1's segment ends while the car slows.
    fn edge_lap() -> (Grid, Map) {
        let n = 401;
        let v = |m: f64| match m {
            m if m < 100.0 => 30.0,
            m if m < 105.0 => 30.0 + (m - 100.0) / 5.0,
            m if m < 280.0 => (961.0 - 6.0 * (m - 105.0)).max(225.0).sqrt(),
            m => (225.0 + 6.0 * (m - 280.0)).sqrt(),
        };
        let speed: Vec<f64> = (0..n).map(|i| v(i as f64)).collect();
        let mut t = vec![0.0; n];
        for i in 1..n {
            t[i] = t[i - 1] + 2.0 / (speed[i] + speed[i - 1]);
        }
        let braking = |i: usize| (105..228).contains(&i);
        let yaw: Vec<f64> = (0..n).map(|i| if (60..100).contains(&i) { speed[i] / 60.0 } else if (240..280).contains(&i) { speed[i] / 40.0 } else { 0.0 }).collect();
        let g = Grid {
            t,
            throttle: (0..n).map(|i| if braking(i) || (228..280).contains(&i) { 0.0 } else if (100..105).contains(&i) || i >= 280 { 100.0 } else { 30.0 }).collect(),
            brake: (0..n).map(|i| if braking(i) { 60.0 } else { 0.0 }).collect(),
            steer: yaw.iter().map(|y| y * 20.0).collect(),
            gear: vec![3.0; n],
            g_lat: vec![0.0; n],
            yaw,
            slip: std::array::from_fn(|_| vec![0.0; n]),
            speed,
        };
        let corner = |n: u32, start_m: f64, end_m: f64, turn_in_m: f64, turn_out_m: f64| super::super::corners::MapCorner {
            n,
            start_m,
            end_m,
            apex_m: (turn_in_m + turn_out_m) / 2.0,
            turn_in_m,
            turn_out_m,
            direction: super::super::Direction::L,
            radius_m: 50.0,
        };
        let map = Map { version: 1, frozen: false, agreed: vec![], length_m: 400.0, corners: vec![corner(1, 0.0, 120.0, 60.0, 100.0), corner(2, 120.0, 400.0, 240.0, 280.0)] };
        (g, map)
    }

    #[test]
    fn a_braking_begun_in_the_segment_before_and_a_car_still_slowing_at_the_segment_s_end_are_measured_where_they_happen() {
        let (mut g, map) = edge_lap();
        let fit = Fit { scale: 1.0, offset: 0.0, matched: 2 };
        // corner 2's braking point is where the pedal went down, 15 m before its segment starts
        assert_eq!(corner(&g, &map, &fit, 1).point_m, Some(105.0));
        // corner 1's slowest point is in its turn (30 m/s), not at its segment's end (29.5 m/s, braking for corner 2)
        let one = corner(&g, &map, &fit, 0);
        assert!(one.min_kmh == 108.0 && (60.0..=100.0).contains(&one.min_at_m), "{one:?}");
        assert_eq!(one.pickup_m, Some(100.0));
        // a braking that began before the lap's start has no known point
        g.brake[..=10].fill(50.0);
        assert_eq!(corner(&g, &map, &fit, 0).point_m, None);
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
