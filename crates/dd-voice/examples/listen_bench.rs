//! Speed and memory of the listener on this PC (numbers for the README):
//! `cargo run --release -p dd-voice --example listen_bench -- <whisper model> <supertonic model> <threads> <wav>..`
//! Loads the listener, transcribes each WAV (16 kHz) five times after a warm-up, then loads the voice into the same
//! process, speaks a coach answer and transcribes again, and reads the process's threads and memory from the OS.

#[path = "common/mod.rs"]
mod common;

use std::time::Instant;

use common::{read_wav, stats};
use dd_voice::{Engine, Listener};

fn median(mut v: Vec<f64>) -> (f64, f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2], v[v.len() - 1])
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, whisper, supertonic, threads, wavs @ ..] = &args[..] else { panic!("usage: listen_bench <whisper model> <supertonic model> <threads> <wav>..") };
    let threads: usize = threads.parse().unwrap();
    let clips: Vec<(String, Vec<f32>)> = wavs
        .iter()
        .map(|w| {
            let (samples, rate) = read_wav(&std::fs::read(w).unwrap()).unwrap();
            assert_eq!(rate, 16_000, "{w}");
            (w.rsplit(['/', '\\']).next().unwrap().to_string(), samples)
        })
        .collect();
    let before = stats();
    let start = Instant::now();
    let mut listener = Listener::load(whisper.as_ref(), threads).unwrap();
    let load = start.elapsed().as_secs_f64();
    let loaded = stats();
    println!("threads {threads}: listener load {load:.2} s; OS threads {} -> {}; working set {:.0} -> {:.0} MB", before.0, loaded.0, before.2, loaded.2);
    listener.transcribe(&clips[0].1, "de").unwrap();
    let lang = |name: &str| if name.contains("-en-") { "en" } else { "de" };
    let run = |listener: &mut Listener, label: &str| {
        for (name, samples) in &clips {
            let (cpu0, mut times) = (stats().1, Vec::new());
            let wall = Instant::now();
            let mut text = String::new();
            for _ in 0..5 {
                let t = Instant::now();
                text = listener.transcribe(samples, lang(name)).unwrap();
                times.push(t.elapsed().as_secs_f64());
            }
            let cores = (stats().1 - cpu0) / wall.elapsed().as_secs_f64();
            let (lo, mid, hi) = median(times);
            println!("{label} {name} ({:.1} s): median {mid:.2} s (min {lo:.2}, max {hi:.2}), CPU {cores:.1} cores: {text:?}", samples.len() as f64 / 16_000.0);
        }
    };
    run(&mut listener, "listener alone");
    let alone = stats();
    println!("listener alone: working set {:.0} MB, peak {:.0} MB, OS threads {}", alone.2, alone.3, alone.0);

    let start = Instant::now();
    let mut engine = Engine::load(supertonic.as_ref(), threads).unwrap();
    println!("voice load {:.2} s", start.elapsed().as_secs_f64());
    let t = Instant::now();
    engine.speak("Copy, ich schau nach. Du hast noch Sprit für sieben Runden.", "de", "M1").unwrap();
    println!("voice speaks an answer in {:.2} s", t.elapsed().as_secs_f64());
    run(&mut listener, "with the voice loaded");
    let both = stats();
    println!("listener + voice: working set {:.0} MB, peak {:.0} MB, OS threads {}", both.2, both.3, both.0);
}
