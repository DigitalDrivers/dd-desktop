//! Tests with the real Whisper model: `DD_WHISPER_MODEL=<model folder> cargo test -p dd-voice`. The round trip
//! through the voice also needs `DD_SUPERTONIC_MODEL`. Without the variables each test says so and passes (an
//! early return, as in `tests/model.rs`). One listener is shared by all tests.

#[path = "../examples/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use common::*;
use dd_voice::{resample, verify_listener, Engine, Listener, Verdict, VoiceError, LISTEN_MODEL_FILES};

fn model_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("DD_WHISPER_MODEL").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: DD_WHISPER_MODEL is not set (the folder of the Whisper model, LISTEN_MODEL_FILES)");
    }
    dir
}

fn listener() -> Option<MutexGuard<'static, Listener>> {
    static LISTENER: OnceLock<Mutex<Listener>> = OnceLock::new();
    let dir = model_dir()?;
    Some(LISTENER.get_or_init(|| Mutex::new(Listener::load(&dir, 2).expect("the model loads"))).lock().unwrap_or_else(|e| e.into_inner()))
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dd-voice-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn silence_and_noise_give_no_text() {
    let Some(mut listener) = listener() else { return };
    let mut rng = Rng(5);
    let mut clips = vec![("silence", vec![0f32; 48_000]), ("hiss -60 dBFS", (0..48_000).map(|_| rng.white() * 0.001 * 3f32.sqrt()).collect())];
    for (db, seed) in [(-45.0, 11), (-35.0, 12), (-25.0, 13), (-35.0, 14), (-25.0, 15)] {
        clips.push(("pink", pink(48_000, db_to_amplitude(db), seed)));
    }
    for (name, samples) in &clips {
        for lang in ["de", "en"] {
            let t = listener.transcript(samples, lang).unwrap();
            println!("{name} {lang}: {:?} {:?}, no-speech {:.2}, logprob {:.2}, level {:.0} dBFS", t.verdict, t.heard, t.no_speech, t.avg_logprob, t.level_dbfs);
            assert_eq!(t.text, "", "{name} {lang}: {t:?}");
        }
    }
    assert_eq!(listener.transcript(&clips[0].1, "de").unwrap().verdict, Verdict::Quiet, "silence never reaches the model");
    assert_eq!(listener.transcribe(&[], "en").unwrap(), "");
}

/// The test sentences spoken by three voices, clean and with pink noise at 20 dB SNR: word error rates per language
/// and condition stay below what was measured on 2026-11-09 plus a margin (README, "Listening").
#[test]
fn the_voice_is_understood() {
    let Some(supertonic) = std::env::var_os("DD_SUPERTONIC_MODEL").map(PathBuf::from) else {
        eprintln!("skipped: DD_SUPERTONIC_MODEL is not set (the round trip speaks the questions with the voice)");
        return;
    };
    let Some(mut listener) = listener() else { return };
    let mut engine = Engine::load(&supertonic, 2).unwrap();
    let mut wer: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (i, (lang, text)) in sentences().into_iter().enumerate() {
        for (v, voice) in TEST_VOICES.iter().enumerate() {
            let speech = resample(&engine.speak(text, lang, voice).unwrap(), Engine::RATE);
            for (condition, snr) in [("clean", None), ("noisy", Some(SNR_DB))] {
                let heard = listener.transcribe(&recording(&speech, snr, 1000 + (i * 3 + v) as u64), lang).unwrap();
                let errors = word_errors(&words(text), &words(&heard));
                if errors > 0 {
                    println!("{voice} {condition}: {text:?} -> {heard:?} ({errors})");
                }
                let e = wer.entry(format!("{lang} {condition}")).or_default();
                e.0 += errors;
                e.1 += words(text).len();
            }
        }
    }
    for (key, (errors, n)) in &wer {
        let rate = *errors as f64 / *n as f64;
        println!("WER {key}: {:.1} % ({errors}/{n})", 100.0 * rate);
        assert!(rate < 0.12, "{key}: {:.1} % (measured 6.4-6.9 %)", 100.0 * rate);
    }
}

#[test]
fn wrong_input_gives_the_right_error() {
    let Some(mut listener) = listener() else { return };
    assert_eq!(listener.transcribe(&[0.1; 16_000], "fr"), Err(VoiceError::UnknownLanguage("fr".into())));
}

#[test]
fn a_missing_model_file_is_named() {
    let dir = temp_dir("no-listener");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(Listener::load(&dir, 2).err(), Some(VoiceError::ModelMissing("config.json")));
    std::fs::remove_dir_all(&dir).unwrap();
    let Some(model) = model_dir() else { return };
    let partial = temp_dir("partial-listener");
    for file in LISTEN_MODEL_FILES.iter().filter(|f| f.size < 10_000_000) {
        let to = partial.join(file.path);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(model.join(file.path), to).unwrap();
    }
    assert_eq!(Listener::load(&partial, 2).err(), Some(VoiceError::ModelMissing("onnx/encoder_model_int8.onnx")));
    assert_eq!(verify_listener(&model, true), Ok(()));
    std::fs::remove_dir_all(&partial).unwrap();
}
