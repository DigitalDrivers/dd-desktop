//! Replays a recording of the game's shared memory (`*.ddsm.gz`, see `dd_core::recorder`) through the spotter and
//! prints its calls as a timeline: the time, the phrase key and priority of the call, the car, and where that car
//! was in the own car's frame (metres ahead, metres to the right).
//!
//! `cargo run -p dd-core --example spotter_replay -- <recording.ddsm.gz>`

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;

use dd_core::evo_memory::car_id;
use dd_core::radio::frame;
use dd_core::radio::queue::call;
use dd_core::radio::spotter::{Config, Event, Spotter};
use dd_core::recorder::{Reader, Record};
use flate2::read::GzDecoder;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: spotter_replay <recording.ddsm.gz>");
        return ExitCode::FAILURE;
    };
    let reader = match File::open(&path).and_then(|file| Reader::new(GzDecoder::new(BufReader::new(file)))) {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("{path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let header = reader.header.clone();
    println!("{path}: '{}', {} Hz, app {}", header.label, header.hz, header.app);

    let mut spotter = Spotter::new(Config::default());
    let (mut frames, mut unread, mut calls) = (0u32, 0u32, 0u32);
    let mut sources: BTreeMap<String, u32> = BTreeMap::new();
    for record in reader {
        let record = match record {
            Ok(record) => record,
            Err(error) => {
                eprintln!("stopped reading: {error}");
                break;
            }
        };
        let Record::Frame { t_us, physics, graphics, .. } = record else { continue };
        let t = t_us as f64 / 1e6;
        let Some(frame) = frame::read(t, &physics, &graphics) else {
            unread += 1;
            continue;
        };
        if frames == 0 {
            println!("first frame at {t:.3} s: own car {}, {} other cars, live {}", car_id(frame.own.id.0, frame.own.id.1), frame.others.len(), frame.live);
        }
        frames += 1;
        let events = spotter.step(&frame);
        *sources.entry(spotter.pose().map_or("none".into(), |pose| format!("{:?}", pose.source))).or_default() += 1;
        for event in events {
            calls += 1;
            let said = call(&event, t);
            let car = said.car.map_or(String::new(), |car| {
                let at = spotter.offset(car).map_or(String::new(), |(long, lat)| format!("  (long {long:+.1} m, lat {lat:+.1} m)"));
                format!("  {}{at}", car_id(car.0, car.1))
            });
            let gap = match event {
                Event::ClosingFast { gap_s, .. } | Event::CloseBehind { gap_s, .. } => format!("  gap {gap_s:.2} s"),
                Event::StillThere { side } => format!("  {side:?}"),
                _ => String::new(),
            };
            println!("{t:9.3}  {:<18} {:<9}{car}{gap}", said.key, format!("{:?}", said.priority));
        }
    }
    println!("{frames} frames, {unread} unreadable (no own car or another page size), {calls} calls; own pose per frame: {sources:?}");
    ExitCode::SUCCESS
}
