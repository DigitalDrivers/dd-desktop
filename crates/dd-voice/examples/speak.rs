//! Speaks lines of a tab-separated file (`voice<TAB>lang<TAB>out.wav<TAB>text`) into 32-bit float WAVs, for
//! comparing with the Python SDK by ear and by numbers:
//! `cargo run --release -p dd-voice --example speak -- <model folder> <lines.tsv>`

use std::time::Instant;

use dd_voice::Engine;

fn float_wav(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 4) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    // IEEE float, mono, 44,100 Hz, 4 bytes per frame, 32 bits.
    for (value, bytes) in [(16u32, 4), (3, 2), (1, 2), (Engine::RATE, 4), (Engine::RATE * 4, 4), (4, 2), (32, 2)] {
        out.extend_from_slice(&value.to_le_bytes()[..bytes]);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    samples.iter().for_each(|s| out.extend_from_slice(&s.to_le_bytes()));
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, model, lines] = &args[..] else { panic!("usage: speak <model folder> <lines.tsv>") };
    let start = Instant::now();
    let mut engine = Engine::load(model.as_ref(), 2).unwrap();
    println!("loaded in {:.2} s", start.elapsed().as_secs_f32());
    for line in std::fs::read_to_string(lines).unwrap().lines().filter(|l| !l.trim().is_empty()) {
        let [voice, lang, out, text] = line.splitn(4, '\t').collect::<Vec<_>>()[..] else { panic!("bad line {line:?}") };
        let start = Instant::now();
        let samples = engine.speak(text, lang, voice).unwrap();
        println!("{voice} {lang} {:.2} s audio in {:.2} s: {text}", samples.len() as f32 / Engine::RATE as f32, start.elapsed().as_secs_f32());
        std::fs::write(out, float_wav(&samples)).unwrap();
    }
}
