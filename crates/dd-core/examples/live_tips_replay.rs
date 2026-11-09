//! Replays a recording of the game's shared memory (`*.ddsm.gz`, see `dd_core::recorder`) through the live tips at
//! the shell's 20 Hz and prints what they say, and each lap that became the reference (its corners and length).
//! The first lap counted whole becomes the reference, so a recording needs two flying laps before any tip.
//!
//! `cargo run -p dd-core --example live_tips_replay -- <recording.ddsm.gz> [de|en] [--touristenfahrten]`

use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;

use dd_core::radio::frame;
use dd_core::radio::live_tips::{Config, LiveTips};
use dd_core::recorder::{Reader, Record};
use flate2::read::GzDecoder;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: live_tips_replay <recording.ddsm.gz> [de|en] [--touristenfahrten]");
        return ExitCode::FAILURE;
    };
    let lang = if args.iter().any(|a| a == "en") { "en" } else { "de" };
    let config = Config { laps_start_at_pit_exit: args.iter().any(|a| a == "--touristenfahrten"), ..Config::default() };
    let reader = match File::open(path).and_then(|file| Reader::new(GzDecoder::new(BufReader::new(file)))) {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("{path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("{path}: '{}', {} Hz, app {}", reader.header.label, reader.header.hz, reader.header.app);
    let mut tips = LiveTips::new(config, lang);
    let (mut last, mut frames, mut said) = (f64::NEG_INFINITY, 0u32, 0u32);
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
        // the shell reads the pages every 50 ms
        if t - last < 0.05 {
            continue;
        }
        let Some(frame) = frame::read(t, &physics, &graphics) else { continue };
        last = t;
        frames += 1;
        if let Some(tip) = tips.step(&frame) {
            said += 1;
            println!("{t:9.3}  lap {:>3}  {}", frame.own.lap_count, tip.text);
        }
        if let Some(reference) = tips.take_new_reference() {
            println!("{t:9.3}  lap {:>3}  new reference: {} corners, {:.0} m", frame.own.lap_count, reference.corners(), reference.length_m());
        }
    }
    println!("{frames} frames at 20 Hz, {said} tips");
    ExitCode::SUCCESS
}
