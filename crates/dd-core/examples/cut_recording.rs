//! Cuts a slice out of a recording of the game's shared memory (`*.ddsm.gz`, see `dd_core::recorder`), for the
//! fixtures of `tests/radio_fixtures.rs`: the frames from `from` to `to` seconds, with their times kept (so the slice
//! still lines up with the app's log and the full recording's timeline) and the static page as it was at the start.
//!
//! `cargo run -p dd-core --example cut_recording -- <recording.ddsm.gz> <from s> <to s> <slice.ddsm.gz>`

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::process::ExitCode;

use dd_core::recorder::{Copied, Reader, Record, Writer, GRAPHICS_MOVED, NO_STATIC, PHYSICS_MOVED};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, from, to, output] = args.as_slice() else {
        eprintln!("usage: cut_recording <recording.ddsm.gz> <from s> <to s> <slice.ddsm.gz>");
        return ExitCode::FAILURE;
    };
    let (Ok(from), Ok(to)) = (from.parse::<f64>(), to.parse::<f64>()) else {
        eprintln!("from and to are seconds");
        return ExitCode::FAILURE;
    };
    match cut(input, from, to, output) {
        Ok(frames) => {
            println!("{output}: {frames} frames from {from} s to {to} s");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn cut(input: &str, from: f64, to: f64, output: &str) -> std::io::Result<u64> {
    let reader = Reader::new(GzDecoder::new(BufReader::new(File::open(input)?)))?;
    let mut header = reader.header.clone();
    header.label = format!("{} {from}-{to} s", header.label);
    let mut writer = Writer::new(GzEncoder::new(BufWriter::new(File::create(output)?), Compression::best()), &header)?;
    let (mut statics, mut missing) = (None, 0u8);
    for record in reader {
        // a recording cut short ends with a broken record: the slice ends there too
        let Ok(record) = record else { break };
        match record {
            Record::Static { page, .. } => statics = Some(page),
            Record::Missing { mask, .. } => missing = mask,
            Record::Frame { t_us, flags, physics, graphics } => {
                let t = t_us as f64 / 1e6;
                if t > to {
                    break;
                }
                if t >= from {
                    let statics = statics.as_deref().filter(|_| missing & NO_STATIC == 0);
                    let physics = Copied { page: &physics, moved: flags & PHYSICS_MOVED != 0 };
                    let graphics = Copied { page: &graphics, moved: flags & GRAPHICS_MOVED != 0 };
                    writer.sample(t_us, Some(physics), Some(graphics), statics)?;
                }
            }
        }
    }
    let frames = writer.frames();
    writer.into_inner().finish()?.flush()?;
    Ok(frames)
}
