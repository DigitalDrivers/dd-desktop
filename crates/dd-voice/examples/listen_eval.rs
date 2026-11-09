//! The listener's test set and model comparison (numbers for the README):
//! - `listen_eval render <supertonic model> <clips dir>`: the test sentences spoken by three voices, at 16 kHz as
//!   push-to-talk recordings, clean and with pink noise at 20 dB SNR, plus silence and noise without speech; 16-bit
//!   WAVs and `manifest.tsv` (name, lang, spoken text).
//! - `listen_eval run <whisper model> <encoder.onnx> <decoder.onnx> <clips dir> <label>`: every clip through that
//!   variant with 2 threads; `<clips dir>/results-<label>.tsv` per clip and word error rates per language and
//!   condition.
//! - `listen_eval mel <clips dir> <out dir>`: the log-mel of every clip as raw little-endian f32 (80 x 3000), for
//!   comparing with the reference implementation.

#[path = "common/mod.rs"]
mod common;

use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

use common::*;
use dd_voice::{resample, Engine, Listener};

fn render(supertonic: &Path, clips: &Path) {
    std::fs::create_dir_all(clips).unwrap();
    let mut engine = Engine::load(supertonic, 2).unwrap();
    let mut manifest = String::new();
    for (i, (lang, text)) in sentences().into_iter().enumerate() {
        for (v, voice) in TEST_VOICES.iter().enumerate() {
            let speech = resample(&engine.speak(text, lang, voice).unwrap(), Engine::RATE);
            for (condition, snr) in [("clean", None), ("noisy", Some(SNR_DB))] {
                let name = format!("q{i:02}-{voice}-{lang}-{condition}");
                let seed = 1000 + (i * TEST_VOICES.len() + v) as u64;
                std::fs::write(clips.join(format!("{name}.wav")), wav_bytes(&recording(&speech, snr, seed), 16_000)).unwrap();
                writeln!(manifest, "{name}\t{lang}\t{text}").unwrap();
            }
        }
        println!("{lang}: {text}");
    }
    // No speech: digital silence, a quiet mic's hiss, pink noise up to a loud PC (RMS -25 dBFS), 3 s each.
    let mut quiet = vec![("silence", vec![0f32; 48_000])];
    let mut rng = Rng(5);
    quiet.push(("hiss-60dB", (0..48_000).map(|_| rng.white() * db_to_amplitude(-60.0) * 3f32.sqrt()).collect()));
    for (db, seed) in [(-45.0, 11), (-35.0, 12), (-25.0, 13), (-35.0, 14), (-25.0, 15)] {
        quiet.push((Box::leak(format!("pink{db:.0}dB-s{seed}").into_boxed_str()), pink(48_000, db_to_amplitude(db), seed)));
    }
    for (name, samples) in quiet {
        std::fs::write(clips.join(format!("noise-{name}.wav")), wav_bytes(&samples, 16_000)).unwrap();
        for lang in ["de", "en"] {
            writeln!(manifest, "noise-{name}\t{lang}\t").unwrap();
        }
    }
    std::fs::write(clips.join("manifest.tsv"), manifest).unwrap();
}

fn load_clip(clips: &Path, name: &str) -> Vec<f32> {
    let (samples, rate) = read_wav(&std::fs::read(clips.join(format!("{name}.wav"))).unwrap()).unwrap();
    assert_eq!(rate, 16_000);
    samples
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() { f64::NAN } else { v[v.len() / 2] }
}

fn run(model: &Path, encoder: &'static str, decoder: &'static str, clips: &Path, label: &str) {
    let before = stats();
    let start = Instant::now();
    let mut listener = Listener::load_with(model, encoder, decoder, 2).unwrap();
    let load = start.elapsed().as_secs_f64();
    let loaded = stats();
    println!("{label}: load {load:.2} s, working set {:.0} -> {:.0} MB", before.2, loaded.2);
    let manifest = std::fs::read_to_string(clips.join("manifest.tsv")).unwrap();
    let rows: Vec<Vec<&str>> = manifest.lines().map(|l| l.split('\t').collect()).collect();
    listener.transcript(&load_clip(clips, rows[0][0]), rows[0][1]).unwrap();

    let mut out = String::from("name\tlang\tseconds\tms\tverdict\tno_speech\tavg_logprob\terrors\tref_words\ttokens\theard\ttext\n");
    // (errors, words) per "lang condition"; times per length class.
    let mut wer: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let (mut short, mut long, mut noise) = (Vec::new(), Vec::new(), Vec::new());
    for row in &rows {
        let (name, lang, spoken) = (row[0], row[1], row[2]);
        let samples = load_clip(clips, name);
        let seconds = samples.len() as f64 / 16_000.0;
        let t = Instant::now();
        let heard = listener.transcript(&samples, lang).unwrap();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let (reference, got) = (words(spoken), words(&heard.text));
        let errors = word_errors(&reference, &got);
        if name.starts_with('q') {
            let condition = name.rsplit('-').next().unwrap();
            let e = wer.entry(format!("{lang} {condition}")).or_default();
            e.0 += errors;
            e.1 += reference.len();
            if condition == "clean" {
                if seconds < 3.6 { short.push((seconds, ms)) } else if seconds > 5.0 { long.push((seconds, ms)) }
            }
            if errors > 0 {
                println!("  {name}: {spoken:?} -> {:?} ({errors} errors)", heard.heard);
            }
        } else {
            noise.push(format!("{name} {lang}: {:?} {:?} (no-speech {:.2}, logprob {:.2}, level {:.0} dBFS)", heard.verdict, heard.heard, heard.no_speech, heard.avg_logprob, heard.level_dbfs));
        }
        let tokens = heard.tokens.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(" ");
        writeln!(out, "{name}\t{lang}\t{seconds:.2}\t{ms:.0}\t{:?}\t{:.4}\t{:.4}\t{errors}\t{}\t{tokens}\t{}\t{}", heard.verdict, heard.no_speech, heard.avg_logprob, reference.len(), heard.heard, heard.text).unwrap();
    }
    std::fs::write(clips.join(format!("results-{label}.tsv")), out).unwrap();
    for line in noise {
        println!("  {line}");
    }
    for (key, (errors, n)) in &wer {
        println!("WER {label} {key}: {:.1} % ({errors}/{n})", 100.0 * *errors as f64 / *n as f64);
    }
    let class = |v: &[(f64, f64)]| (median(v.iter().map(|x| x.0).collect()), median(v.iter().map(|x| x.1).collect()), v.len());
    let ((s_len, s_ms, s_n), (l_len, l_ms, l_n)) = (class(&short), class(&long));
    let end = stats();
    println!(
        "{label}: short clips ({s_n}, median {s_len:.1} s): {s_ms:.0} ms; long clips ({l_n}, median {l_len:.1} s): {l_ms:.0} ms; working set {:.0} MB, peak {:.0} MB; CPU {:.0} s",
        end.2, end.3, end.1 - loaded.1
    );
}

fn mel(clips: &Path, out: &Path) {
    std::fs::create_dir_all(out).unwrap();
    let manifest = std::fs::read_to_string(clips.join("manifest.tsv")).unwrap();
    let mut names: Vec<&str> = manifest.lines().map(|l| l.split('\t').next().unwrap()).collect();
    names.dedup();
    for name in names {
        let mel = dd_voice::log_mel(&load_clip(clips, name), 80);
        std::fs::write(out.join(format!("{name}.f32")), mel.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match &args[1..] {
        [cmd, supertonic, clips] if cmd == "render" => render(supertonic.as_ref(), clips.as_ref()),
        [cmd, model, encoder, decoder, clips, label] if cmd == "run" => {
            let leak = |s: &String| -> &'static str { Box::leak(s.clone().into_boxed_str()) };
            run(model.as_ref(), leak(encoder), leak(decoder), clips.as_ref(), label)
        }
        [cmd, clips, out] if cmd == "mel" => mel(clips.as_ref(), out.as_ref()),
        _ => panic!("usage: listen_eval render <supertonic model> <clips dir> | run <whisper model> <encoder> <decoder> <clips dir> <label> | mel <clips dir> <out dir>"),
    }
}
