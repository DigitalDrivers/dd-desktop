//! The race engineer against its contract (`example-session.json`, which the platform's tests read too) and, when
//! `DD_MOTEC_FIXTURE` points at it, against Joe's MoTeC file of 2026-10-04 (`…unl3_NurburgringTouristenfahrten_Sun
//! Oct  4 224023 2026.ld`) and the hand analysis of it (research/2026-10-05-telemetry-check.md).

use dd_core::engineer::clock::days_from_civil;
use dd_core::engineer::evaluate::{run, Input};
use dd_core::engineer::laps::LocalLap;
use dd_core::engineer::{store, EngineerSession};
use serde_json::Value;

const EXAMPLE: &str = include_str!("fixtures/example-session.json");

/// The keys of each object of the contract (`contract-engineer.ts`), by where it sits.
const CONTRACT: [(&str, &[&str]); 17] = [
    ("", &["id", "car", "preset", "carName", "track", "layout", "startedAt", "endedAt", "appVersion", "reference", "corners", "laps", "timingNote"]),
    ("reference", &["sessionId", "lapId", "lapTimeMs"]),
    ("corners", &["version", "frozen", "corners"]),
    ("corners.corners[]", &["n", "startM", "endM", "apexM", "direction", "radiusM"]),
    ("laps[]", &["id", "n", "lapTimeMs", "valid", "pit", "distanceM", "topSpeedKmh", "theoreticalBestMs", "fuel", "tyres", "brakes", "balance", "traction", "corners", "findings"]),
    ("laps[].fuel", &["startL", "usedL"]),
    ("laps[].tyres", &["pressureStart", "pressureAvg", "pressureEnd", "surfaceTempAvg", "airTempAvg", "targetPsi"]),
    ("laps[].tyres.targetPsi", &["min", "max"]),
    ("laps[].brakes", &["tempMax", "secondsAbove700", "frontShare"]),
    ("laps[].balance[]", &["band", "relative", "samples"]),
    ("laps[].traction[]", &["gear", "spinShare", "samples"]),
    ("laps[].corners[]", &["n", "dtS", "entryKmh", "brake", "minSpeed", "coastS", "throttle", "exitKmh", "latGMax", "balance", "lockS", "spinS"]),
    ("laps[].corners[].brake", &["pointM", "vsRefM", "peak", "releaseM"]),
    ("laps[].corners[].minSpeed", &["kmh", "atM", "vsRefKmh", "gear"]),
    ("laps[].corners[].throttle", &["pickupM", "fullM", "vsRefM"]),
    ("laps[].corners[].balance", &["entry", "mid", "exit"]),
    ("laps[].findings[]", &[]),
];

/// Every object found at `path` (`a.b[]` walks into arrays).
fn at<'a>(value: &'a Value, path: &str) -> Vec<&'a Value> {
    let mut found = vec![value];
    for part in path.split('.').filter(|p| !p.is_empty()) {
        let (key, many) = part.strip_suffix("[]").map_or((part, false), |k| (k, true));
        found = found
            .into_iter()
            .filter_map(|v| v.get(key))
            .flat_map(|v| if many { v.as_array().map(|a| a.iter().collect()).unwrap_or_default() } else { vec![v] })
            .filter(|v| !v.is_null())
            .collect();
    }
    found
}

fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    keys.sort();
    keys
}

#[test]
fn the_example_session_round_trips_and_has_exactly_the_contract_s_fields() {
    let session: EngineerSession = serde_json::from_str(EXAMPLE).unwrap();
    let written = serde_json::to_value(&session).unwrap();
    assert_eq!(written, serde_json::from_str::<Value>(EXAMPLE).unwrap(), "serialising gives the example back");
    for (path, fields) in CONTRACT {
        let objects = at(&written, path);
        assert!(!objects.is_empty(), "the example has {path:?}");
        if fields.is_empty() {
            continue;
        }
        let mut expected: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
        expected.sort();
        for object in objects {
            assert_eq!(keys(object), expected, "{path}");
        }
    }
    // the enums as the contract spells them
    assert_eq!(written["timingNote"], "partial-window");
    assert_eq!(written["corners"]["corners"][0]["direction"], "L");
    let bands: Vec<&Value> = at(&written, "laps[].balance[]").into_iter().map(|b| &b["band"]).collect();
    assert_eq!(bands[..4], [&Value::from("60-120"), &Value::from("120-180"), &Value::from("180-240"), &Value::from("240+")]);
    let findings: Vec<String> = at(&written, "laps[].findings[]").iter().map(|f| f.as_str().unwrap().to_string()).collect();
    for f in &findings {
        assert!(
            [
                "brake_later", "brake_earlier", "brake_softer", "coast_long", "throttle_later", "throttle_earlier", "min_speed_lower", "exit_slower", "lock_front",
                "lock_rear", "spin_rear", "understeer_entry", "oversteer_exit", "pressure_high", "pressure_low", "brake_hot", "brake_fade", "fuel_heavy"
            ]
            .contains(&f.as_str()),
            "{f}"
        );
    }
    // wheels are arrays of four, nulls are written as null
    assert_eq!(written["laps"][0]["tyres"]["pressureEnd"].as_array().unwrap().len(), 4);
    assert_eq!(written["laps"][0]["corners"][1]["brake"]["pointM"], Value::Null);
    assert_eq!(written["laps"][0]["balance"][3]["relative"], Value::Null);
}

/// Runs with `DD_MOTEC_FIXTURE=<the 224023 .ld> cargo test -p dd-core --test engineer -- --ignored --nocapture`.
#[test]
#[ignore]
fn evaluates_joe_s_laps_of_2026_10_04_as_the_hand_analysis_measured_them() {
    let path = std::env::var("DD_MOTEC_FIXTURE").expect("DD_MOTEC_FIXTURE: the path of the 224023 .ld");
    let modified_ms = std::fs::metadata(&path).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    // t 0 of the file is 23:22:27.291 CEST (the game log's pit stop and lap events, telemetry-check.md §0). That
    // log is gone; the data ends at the car's last lap completion, so its line is written here at t 0 + 1,009.367 s.
    let t0_ms = (days_from_civil(2026, 10, 4) * 86_400 + 21 * 3600 + 22 * 60 + 27) * 1000 + 291;
    let log = "[2026-10-04 23:39:16.658] [physics] [info] Lap test evOnLapCompleted 3 completed\n";
    let lap = |end_s: f64, lap_time_ms: i64| LocalLap {
        end_ms: t0_ms + (end_s * 1000.0).round() as i64,
        lap_time_ms,
        valid: true,
        pit: false,
        car: "dd_bmw_m3_e46_rennsport".into(),
        preset: Some("preset_dd_bmw_m3_e46_rennsport_unl3".into()),
        track: "Nurburgring".into(),
        layout: "Touristenfahrten".into(),
        session_start_ms: t0_ms + 101_571,
    };
    let laps = [lap(477.355, 375_780), lap(983.020, 368_283)];
    let dir = std::env::temp_dir().join(format!("dd-engineer-fixture-{}", std::process::id()));
    let input = Input {
        name: "BMW M3 E46 CSL GT3 Rennsport/NurburgringTouristenfahrten/BMW M3 E46 CSL GT3 Rennsport_preset_dd_bmw_m3_e46_rennsport_unl3_NurburgringTouristenfahrten_Sun Oct  4 224023 2026.ld",
        modified_ms,
        logs: &[log],
        laps: &laps,
        app_version: "0.25.0",
        now_ms: modified_ms,
    };
    let started = std::time::Instant::now();
    let outcome = run(&dir, &mut std::fs::File::open(&path).unwrap(), &input).unwrap();
    let elapsed = started.elapsed();
    let s = store::session(&dir, &outcome.id).unwrap();
    println!("{outcome:?} in {elapsed:?}");
    println!("session {} {} {} {} to {}, map v{} frozen {} with {} corners, reference {:?}, {:?}", s.id, s.car, s.preset.as_deref().unwrap_or("-"), s.started_at, s.ended_at, s.corners.version, s.corners.frozen, s.corners.corners.len(), s.reference.as_ref().map(|r| &r.lap_id), s.timing_note);
    for l in &s.laps {
        println!("lap {} ({} ms): {} m, top {} km/h, theoretical best {:?} ms", l.n, l.lap_time_ms.unwrap(), l.distance_m, l.top_speed_kmh, l.theoretical_best_ms);
        println!("  fuel start {} l, used {} l", l.fuel.start_l, l.fuel.used_l);
        println!("  psi start {:?} avg {:?} end (last 20 s) {:?}", l.tyres.pressure_start, l.tyres.pressure_avg, l.tyres.pressure_end);
        println!("  surface {:?} air {:?}", l.tyres.surface_temp_avg, l.tyres.air_temp_avg);
        println!("  brakes max {:?}, {} s above 700 °C, front share {}", l.brakes.temp_max, l.brakes.seconds_above700, l.brakes.front_share);
        println!("  balance {:?}", l.balance.iter().map(|b| (b.relative, b.samples)).collect::<Vec<_>>());
        println!("  traction {:?}", l.traction.iter().map(|t| (t.gear, t.spin_share)).collect::<Vec<_>>());
        let dts: f64 = l.corners.iter().filter_map(|c| c.dt_s).sum();
        println!("  corners {}, sum of dt {:.3} s, findings {:?}", l.corners.len(), dts, l.findings);
    }
    assert!(outcome.from_log && outcome.dropped.is_empty(), "{outcome:?}");
    assert_eq!(s.laps.len(), 2);
    let two = &s.laps[1];
    // telemetry-check.md, lap 2 (6:08.283, t 614.8–983.0)
    assert!((two.distance_m - 18_930.0).abs() <= 20.0, "18,933 m (telemetry-core.md §4)");
    assert!((two.fuel.start_l - 60.61).abs() < 0.05 && (two.fuel.used_l - 21.56).abs() < 0.1, "60.61 -> 39.04 l");
    assert!((s.laps[0].fuel.start_l - 28.15).abs() < 0.05 && (s.laps[0].fuel.used_l - 20.53).abs() < 0.1, "lap 1: 28.15 -> 7.62 l");
    for (got, want) in two.tyres.pressure_start.iter().zip([23.20, 23.21, 23.17, 23.19]) {
        assert!((got - want).abs() <= 0.02, "start {:?}", two.tyres.pressure_start);
    }
    for (got, want) in two.tyres.pressure_end.iter().zip([27.81, 27.74, 27.67, 27.55]) {
        assert!((got - want).abs() <= 0.03, "last 20 s {:?}", two.tyres.pressure_end);
    }
    assert!((two.brakes.temp_max[0] - 843.0).abs() <= 2.0 && (two.brakes.temp_max[1] - 859.0).abs() <= 2.0, "{:?}", two.brakes.temp_max);
    assert!((44.5..=47.0).contains(&two.brakes.seconds_above700), "45–46 s: {}", two.brakes.seconds_above700);
    assert!((two.brakes.front_share - 0.69).abs() <= 0.01, "68.5–69.2 %: {}", two.brakes.front_share);
    assert!((65..=73).contains(&s.corners.corners.len()), "69 corners at 1/350 m: {}", s.corners.corners.len());
    // lap 2 is the reference; lap 1's corner losses add up to the 7.5 s between the laps
    assert_eq!(s.reference.as_ref().unwrap().lap_id, two.id);
    let dts: f64 = s.laps[0].corners.iter().filter_map(|c| c.dt_s).sum();
    assert!((dts - 7.497).abs() < 0.1, "{dts}");
    store::forget(&dir).unwrap();
}
