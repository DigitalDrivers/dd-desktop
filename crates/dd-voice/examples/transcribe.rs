//! What the listener hears in a WAV file (any rate, mono or stereo, 16/24/32-bit PCM or 32-bit float):
//! `DD_WHISPER_MODEL=<model folder> cargo run --release -p dd-voice --example transcribe -- <wav> <de|en>`

#[path = "common/mod.rs"]
mod common;

use std::path::PathBuf;
use std::time::Instant;

use dd_voice::{resample, Listener};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, wav, lang] = &args[..] else { panic!("usage: transcribe <wav> <de|en>   (model folder in DD_WHISPER_MODEL)") };
    let model = PathBuf::from(std::env::var_os("DD_WHISPER_MODEL").expect("DD_WHISPER_MODEL: the folder of the Whisper model"));
    let (samples, rate) = common::read_wav(&std::fs::read(wav).expect("the WAV file")).expect("a WAV file");
    let samples = resample(&samples, rate);
    let start = Instant::now();
    let mut listener = Listener::load(&model, 2).unwrap();
    println!("loaded in {:.2} s; {:.2} s of audio at {rate} Hz", start.elapsed().as_secs_f32(), samples.len() as f32 / 16_000.0);
    let start = Instant::now();
    let t = listener.transcript(&samples, lang).unwrap();
    println!("{:?} in {:.2} s: {:?}", t.verdict, start.elapsed().as_secs_f32(), t.text);
    println!("model heard {:?}; no-speech {:.2}, avg logprob {:.2}, loudest 20 ms {:.0} dBFS", t.heard, t.no_speech, t.avg_logprob, t.level_dbfs);
}
