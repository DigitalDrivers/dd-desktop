//! Speed and memory of the voice on this PC (numbers for the README):
//! `cargo run --release -p dd-voice --example bench -- <model folder> <threads>`
//! Loads the model, speaks short calls and live-coach answers (two passes, voice M1), renders the German and the
//! English library and one driver's named calls into a fresh temporary cache, and reads the process's threads,
//! CPU time and memory from the OS (PowerShell on Windows, /proc on Linux).

use std::time::Instant;

use dd_voice::{render_library, render_names, Engine};

const SHORT: [(&str, &str); 9] = [
    ("de", "Auto links."),
    ("de", "Frei links."),
    ("de", "Drei nebeneinander!"),
    ("de", "Sprit für noch vier Runden."),
    ("de", "Jörn, Box diese Runde."),
    ("en", "Car left."),
    ("en", "Clear right."),
    ("en", "Joe, car closing fast behind."),
    ("de", "Im Windschatten."),
];
const LONG: [(&str, &str); 3] = [
    ("de", "Deine Reifen sind jetzt warm, du kannst in Kurve drei später bremsen und früher aufs Gas gehen."),
    ("en", "Your tyres are in the window now, so you can brake a little later into turn three and get on the power earlier."),
    ("de", "In der letzten Runde warst du am Kurvenausgang sechs Kilometer pro Stunde schneller, halte diese Linie und bleib ruhig am Gas."),
];

/// Threads, CPU seconds, working set and peak working set (MB) of this process.
fn stats() -> (u32, f64, f64, f64) {
    if cfg!(windows) {
        let script = format!(
            "$p = Get-Process -Id {}; '{{0}} {{1}} {{2}} {{3}}' -f $p.Threads.Count, $p.TotalProcessorTime.TotalSeconds, $p.WorkingSet64, $p.PeakWorkingSet64",
            std::process::id()
        );
        let out = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).replace(',', ".");
        let v: Vec<f64> = text.split_whitespace().map(|x| x.parse().unwrap()).collect();
        (v[0] as u32, v[1], v[2] / 1e6, v[3] / 1e6)
    } else {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let field = |name: &str| status.lines().find(|l| l.starts_with(name)).unwrap().split_whitespace().nth(1).unwrap().parse::<f64>().unwrap();
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let after = stat.rsplit(')').next().unwrap().split_whitespace().collect::<Vec<_>>();
        let cpu = (after[11].parse::<f64>().unwrap() + after[12].parse::<f64>().unwrap()) / 100.0;
        (field("Threads:") as u32, cpu, field("VmRSS:") / 1e3, field("VmHWM:") / 1e3)
    }
}

fn median_p90(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], v[((v.len() as f64 * 0.9).ceil() as usize).min(v.len()) - 1])
}

fn timed(engine: &mut Engine, lines: &[(&str, &str)], passes: usize) -> (Vec<f64>, Vec<f64>, f64) {
    let (mut walls, mut audio) = (Vec::new(), Vec::new());
    let (_, cpu0, _, _) = stats();
    let start = Instant::now();
    for _ in 0..passes {
        for (lang, text) in lines {
            let t = Instant::now();
            let samples = engine.speak(text, lang, "M1").unwrap();
            walls.push(t.elapsed().as_secs_f64());
            audio.push(samples.len() as f64 / Engine::RATE as f64);
        }
    }
    let wall = start.elapsed().as_secs_f64();
    let (_, cpu1, _, _) = stats();
    (walls, audio, (cpu1 - cpu0) / wall)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, model, threads] = &args[..] else { panic!("usage: bench <model folder> <threads>") };
    let threads: usize = threads.parse().unwrap();
    let before = stats();
    let start = Instant::now();
    let mut engine = Engine::load(model.as_ref(), threads).unwrap();
    let load = start.elapsed().as_secs_f64();
    let loaded = stats();
    println!("threads {threads}: load {load:.2} s; OS threads {} -> {}; working set {:.0} MB after load", before.0, loaded.0, loaded.2);
    engine.speak("Warmup.", "en", "M1").unwrap();

    for (name, lines) in [("short", &SHORT[..]), ("long", &LONG[..])] {
        let (walls, audio, cores) = timed(&mut engine, lines, 2);
        let rtf: Vec<f64> = walls.iter().zip(&audio).map(|(w, a)| w / a).collect();
        let ((w, w90), (r, _)) = (median_p90(walls), median_p90(rtf));
        let words = lines.iter().map(|(_, t)| t.split_whitespace().count()).collect::<Vec<_>>();
        println!("{name} ({words:?} words, 2 passes): median {w:.2} s (p90 {w90:.2}), RTF {r:.2}, CPU {cores:.1} cores");
    }

    let cache = std::env::temp_dir().join(format!("dd-voice-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    for lang in ["de", "en"] {
        let done = render_library(&mut engine, "M1", lang, &cache, |_, _| {}).unwrap();
        println!("library M1 {lang}: {} wordings in {:.1} s", done.rendered.len(), done.elapsed.as_secs_f64());
    }
    let again = render_library(&mut engine, "M1", "de", &cache, |_, _| {}).unwrap();
    println!("library M1 de again: {} rendered, {} cached, {:.3} s", again.rendered.len(), again.cached, again.elapsed.as_secs_f64());
    let names = render_names(&mut engine, "M1", "de", "Jörrn", &cache).unwrap();
    println!("names M1 de \"Jörrn\": {} calls in {:.2} s", names.rendered.len(), names.elapsed.as_secs_f64());
    std::fs::remove_dir_all(&cache).unwrap();
    let end = stats();
    println!("end: OS threads {}, working set {:.0} MB, peak {:.0} MB", end.0, end.2, end.3);
}
