//! The race engineer: evaluates the game's MoTeC files on this PC, lap by lap and corner by corner, against the
//! driver's own best lap. Nothing of it leaves the PC; the page shows the report, and only on a click with the
//! driver's consent for the AI does it send the computed numbers (never the curves) to the platform.
//!
//! The lap boundaries come from the app's own lap list ([`laps`], written while the local switch is on), the
//! clock of a MoTeC file from its header and the game's log ([`clock`]), the data from the file ([`ld`]). A lap is
//! cut out of the file and resampled over distance ([`cut`]), its corners are matched against the track's corner
//! map ([`corners`]) and measured ([`measure`]); [`store`] keeps the results in the app's config folder. The types
//! below are the contract with the platform (`research/2026-10-16-tank-renningenieur/contract-engineer.ts`):
//! distances in metres from the lap's start (corner positions on the corner map's distance), speeds in km/h, times
//! in seconds unless the name says ms, pressures in psi, temperatures in °C, pedals in % (0 to 100), wheels in the
//! order FL FR RL RR.

pub mod beacons;
pub mod clock;
pub mod corners;
pub mod cut;
pub mod evaluate;
pub mod gamelog;
pub mod laps;
pub mod ld;
pub mod measure;
pub mod sessions;
pub mod store;
#[cfg(test)]
pub(crate) mod testkit;

use serde::{Deserialize, Serialize};

/// The report of one MoTeC session: its laps, measured on the track's corner map against the reference lap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineerSession {
    /// `<unix seconds of the session start>-<car>`
    pub id: String,
    /// The car's id (its folder below `cars`)
    pub car: String,
    /// The preset as the game's log names it
    pub preset: Option<String>,
    pub car_name: String,
    pub track: String,
    pub layout: String,
    /// ISO 8601 in the PC's local time, with its offset
    pub started_at: String,
    pub ended_at: String,
    pub app_version: String,
    /// The driver's best valid lap of this car, preset, track and layout on this corner map
    pub reference: Option<Reference>,
    pub corners: CornerMap,
    pub laps: Vec<EngineerLap>,
    pub timing_note: TimingNote,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub session_id: String,
    pub lap_id: String,
    pub lap_time_ms: i64,
}

/// `partial-window` where the corner losses do not add up to the lap time: on Touristenfahrten (the recording
/// starts at the pit exit, the lap time at the start split), and for a session without laps of the app's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimingNote {
    FullLap,
    PartialWindow,
}

/// Where a lap's boundaries and time come from: the app's lap list (the game's timing), the game's log, the
/// `.ldx` beacons (both the timing line to the millisecond), or the telemetry (the timed lap found on the corner
/// map inside a Touristenfahrten loop, to about 0.1 s).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimeSource {
    #[default]
    App,
    Log,
    Beacon,
    Telemetry,
}

/// The track's corners T1…Tn from the lap's start. Each corner owns the segment from the middle of the straight
/// before it to the middle of the straight after it, so the segments tile the lap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CornerMap {
    pub version: u32,
    /// Frozen once three laps agreed: the numbers then stay for the history
    pub frozen: bool,
    pub corners: Vec<Corner>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Corner {
    pub n: u32,
    pub start_m: f64,
    pub end_m: f64,
    pub apex_m: f64,
    pub direction: Direction,
    pub radius_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    L,
    R,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineerLap {
    /// `<session id>#<n>`
    pub id: String,
    pub n: u32,
    pub lap_time_ms: Option<i64>,
    pub valid: bool,
    pub pit: bool,
    pub distance_m: f64,
    /// Seconds with the brake above 5 % (what the lock shares are of); null in reports of apps before 0.26.0
    #[serde(default)]
    pub braking_s: Option<f64>,
    pub top_speed_kmh: f64,
    /// Sum of the best segment times of this driver on this map (valid laps), null without a reference
    pub theoretical_best_ms: Option<i64>,
    pub fuel: Fuel,
    pub tyres: Tyres,
    pub brakes: Brakes,
    /// Steering per yaw rate relative to the reference lap by speed band: > 0 more steering for the same yaw
    pub balance: Vec<BandBalance>,
    /// Share of the throttle samples with rear slip ratio above 0.15, per gear
    pub traction: Vec<GearTraction>,
    pub corners: Vec<CornerResult>,
    pub findings: Vec<Finding>,
    /// Missing in reports of app 0.25.0: the app's lap list
    #[serde(default)]
    pub time_source: TimeSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fuel {
    pub start_l: f64,
    pub used_l: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tyres {
    pub pressure_start: [f64; 4],
    pub pressure_avg: [f64; 4],
    /// The mean of the lap's last 20 s
    pub pressure_end: [f64; 4],
    /// Mean of the three surface temperatures of each wheel
    pub surface_temp_avg: [f64; 4],
    pub air_temp_avg: [f64; 4],
    /// The target window of the hot pressure (the lap's end) for the compound fitted, null when there is no
    /// judgement (compound unknown, a car without a table entry, a circuit's out lap or pit lap)
    pub target_psi: Option<TargetPsi>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetPsi {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Brakes {
    pub temp_max: [f64; 4],
    /// Seconds above 700 °C of the disc that spent the most there
    pub seconds_above700: f64,
    /// The front axle's share of the brake torque over the braking samples
    pub front_share: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Band {
    #[serde(rename = "60-120")]
    B60,
    #[serde(rename = "120-180")]
    B120,
    #[serde(rename = "180-240")]
    B180,
    #[serde(rename = "240+")]
    B240,
}

impl Band {
    pub const ALL: [Band; 4] = [Band::B60, Band::B120, Band::B180, Band::B240];

    /// The band of a speed in km/h; None below 60 km/h.
    pub fn of(kmh: f64) -> Option<Band> {
        match kmh {
            k if k >= 240.0 => Some(Band::B240),
            k if k >= 180.0 => Some(Band::B180),
            k if k >= 120.0 => Some(Band::B120),
            k if k >= 60.0 => Some(Band::B60),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BandBalance {
    pub band: Band,
    pub relative: Option<f64>,
    pub samples: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GearTraction {
    pub gear: i32,
    pub spin_share: f64,
    pub samples: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CornerResult {
    pub n: u32,
    /// Time lost (> 0) or gained against the reference over the corner's segment; null without a reference
    pub dt_s: Option<f64>,
    pub entry_kmh: f64,
    pub brake: CornerBrake,
    pub min_speed: MinSpeed,
    /// Seconds with neither brake nor throttle above 5 % between the brake release and the throttle pick-up
    pub coast_s: f64,
    pub throttle: CornerThrottle,
    pub exit_kmh: f64,
    pub lat_g_max: f64,
    pub balance: CornerBalance,
    /// Seconds a wheel locked under braking (slip ratio below -0.15), FL FR RL RR
    pub lock_s: [f64; 4],
    /// Seconds a wheel spun under throttle above 50 % (slip ratio above 0.15), FL FR RL RR
    pub spin_s: [f64; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CornerBrake {
    pub point_m: Option<f64>,
    pub vs_ref_m: Option<f64>,
    pub peak: f64,
    pub release_m: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinSpeed {
    pub kmh: f64,
    pub at_m: f64,
    pub vs_ref_kmh: Option<f64>,
    pub gear: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CornerThrottle {
    pub pickup_m: Option<f64>,
    pub full_m: Option<f64>,
    pub vs_ref_m: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CornerBalance {
    pub entry: Option<f64>,
    pub mid: Option<f64>,
    pub exit: Option<f64>,
}

/// Codes; the sentences live in the platform's i18n files (`engineer.findings.*`). A corner's codes (brake_later,
/// brake_earlier, coast_long, throttle_later, throttle_earlier, min_speed_lower, exit_slower, spin_rear) come only
/// where the corner cost at least 0.05 s against the reference, the braking, throttle and speed ones only beyond the
/// driver's own spread there ([`measure::report`]); lock_front, lock_rear, understeer and oversteer are codes of the
/// lap. Reports of apps before 0.26.0 also carry brake_softer, understeer_entry and oversteer_exit, and
/// lock_front and lock_rear per corner; they stay readable, the app no longer writes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finding {
    BrakeLater,
    BrakeEarlier,
    /// No longer written (noise alone breaks its threshold, REPORT.md section 7.2)
    BrakeSofter,
    CoastLong,
    ThrottleLater,
    ThrottleEarlier,
    MinSpeedLower,
    ExitSlower,
    LockFront,
    LockRear,
    SpinRear,
    /// No longer written: a corner's balance is noise (REPORT.md section 3)
    UndersteerEntry,
    /// No longer written
    OversteerExit,
    PressureHigh,
    PressureLow,
    BrakeHot,
    BrakeFade,
    FuelHeavy,
    /// More steering for the same yaw than the reference in a speed band below 240 km/h
    Understeer,
    /// Less steering for the same yaw than the reference in a speed band below 240 km/h
    Oversteer,
}

/// The curves of a lap for the charts, every 5 m from the lap's start; they stay on the PC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineerTrace {
    pub lap_id: String,
    pub step_m: f64,
    pub speed_kmh: Vec<f64>,
    pub throttle: Vec<f64>,
    pub brake: Vec<f64>,
    pub steer_deg: Vec<f64>,
    pub gear: Vec<i32>,
}

/// `value` rounded to `decimals` places, so the reports stay small and readable (and never `-0`).
pub(crate) fn round(value: f64, decimals: i32) -> f64 {
    let f = 10f64.powi(decimals);
    (value * f).round() / f + 0.0
}
