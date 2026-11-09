//! Tests with the real Supertonic 3 model: `DD_SUPERTONIC_MODEL=<model folder> cargo test -p dd-voice`.
//! Without the variable each test says so and passes (an early return, not `#[ignore]`: setting the variable is
//! enough to run them), so CI stays green without the 383 MiB download. One engine is shared by all tests.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use dd_core::radio::phrases;
use dd_voice::{load_clip, render_library, render_names, Engine, VoiceError};

fn model_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("DD_SUPERTONIC_MODEL").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: DD_SUPERTONIC_MODEL is not set (the folder of the Supertonic 3 model)");
    }
    dir
}

fn engine() -> Option<MutexGuard<'static, Engine>> {
    static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();
    let dir = model_dir()?;
    Some(ENGINE.get_or_init(|| Mutex::new(Engine::load(&dir, 2).expect("the model loads"))).lock().unwrap_or_else(|e| e.into_inner()))
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dd-voice-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Length in seconds, peak and RMS, after checking every sample is a number.
fn measure(samples: &[f32]) -> (f32, f32, f32) {
    assert!(samples.iter().all(|s| s.is_finite()), "NaN or infinite samples");
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    (samples.len() as f32 / Engine::RATE as f32, peak, rms)
}

#[test]
fn short_calls_sound_plausible_in_m1_and_f1() {
    let Some(mut engine) = engine() else { return };
    for voice in ["M1", "F1"] {
        for (text, lang) in [("Auto links.", "de"), ("Car left.", "en")] {
            let (seconds, peak, rms) = measure(&engine.speak(text, lang, voice).unwrap());
            println!("{voice} {lang} {text:?}: {seconds:.2} s, peak {peak:.2}, rms {rms:.3}");
            assert!((0.4..1.5).contains(&seconds), "{voice} {text}: {seconds} s");
            assert!((0.05..=1.0).contains(&peak), "{voice} {text}: peak {peak}");
            assert!(rms > 0.005, "{voice} {text}: silent, rms {rms}");
        }
    }
}

#[test]
fn the_same_text_gives_the_same_samples() {
    let Some(mut engine) = engine() else { return };
    let a = engine.speak("Sprit für noch vier Runden.", "de", "M1").unwrap();
    let b = engine.speak("Sprit für noch vier Runden.", "de", "M1").unwrap();
    assert_eq!(a, b);
}

#[test]
fn every_voice_speaks() {
    let Some(mut engine) = engine() else { return };
    for voice in Engine::voices() {
        let (seconds, peak, rms) = measure(&engine.speak("Frei rechts.", "de", voice).unwrap());
        println!("{voice}: {seconds:.2} s, peak {peak:.2}, rms {rms:.3}");
        assert!((0.4..2.0).contains(&seconds) && peak > 0.05 && rms > 0.005, "{voice}");
    }
}

#[test]
fn long_text_is_spoken_in_chunks_with_silence_between() {
    let Some(mut engine) = engine() else { return };
    let sentence = "Deine Reifen sind jetzt warm, du kannst in Kurve drei später bremsen und früher aufs Gas gehen.";
    let one = engine.speak(sentence, "de", "M1").unwrap().len();
    let four = engine.speak(&[sentence; 4].join(" "), "de", "M1").unwrap();
    // 4 x 96 characters: two chunks of up to 300, 0.3 s of silence between them.
    assert!(four.len() > 3 * one && four.len() < 5 * one, "{} vs {one}", four.len());
}

#[test]
fn the_library_renders_once_and_comes_from_the_cache_after() {
    let Some(mut engine) = engine() else { return };
    let dir = temp_dir("library");
    let mut wordings: Vec<&String> = phrases::fixed_keys().iter().flat_map(|k| phrases::wordings(k, "de")).collect();
    wordings.dedup();
    let mut progress = Vec::new();
    let first = render_library(&mut engine, "F1", "de", &dir, |done, total| progress.push((done, total))).unwrap();
    println!("rendered {} wordings in {:.1} s", first.rendered.len(), first.elapsed.as_secs_f32());
    assert_eq!(first.rendered.len(), wordings.len());
    assert_eq!(first.cached, 0);
    assert_eq!(progress.last(), Some(&(wordings.len(), wordings.len())));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), wordings.len(), "one file per wording, no leftovers");

    let clip = load_clip(&dir, "F1", "de", "Auto links.").unwrap();
    let spoken = engine.speak("Auto links.", "de", "F1").unwrap();
    assert_eq!(clip.len(), spoken.len());
    assert!(clip.iter().zip(&spoken).all(|(a, b)| (a - b).abs() <= 1.0 / 32767.0 + 1e-6), "the dry voice, to 16 bits");
    assert_eq!(load_clip(&dir, "M1", "de", "Auto links."), None, "another voice is not in the cache");

    let second = render_library(&mut engine, "F1", "de", &dir, |_, _| {}).unwrap();
    assert!(second.rendered.is_empty());
    assert_eq!(second.cached, wordings.len());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_named_calls_render_for_one_driver() {
    let Some(mut engine) = engine() else { return };
    let dir = temp_dir("names");
    let first = render_names(&mut engine, "M2", "en", "Yern", &dir).unwrap();
    println!("rendered {:?} in {:.2} s", first.rendered, first.elapsed.as_secs_f32());
    assert_eq!(first.rendered.len(), phrases::named_keys().len());
    assert!(first.rendered.iter().all(|t| t.contains("Yern")));
    assert!(load_clip(&dir, "M2", "en", "Yern on your left.").is_some());
    assert!(render_names(&mut engine, "M2", "en", "Yern", &dir).unwrap().rendered.is_empty());
    assert_eq!(render_names(&mut engine, "M2", "en", " ", &dir), Err(VoiceError::EmptyText));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn wrong_input_gives_the_right_error() {
    let Some(mut engine) = engine() else { return };
    let code = |r: Result<Vec<f32>, VoiceError>| r.unwrap_err().code();
    assert_eq!(code(engine.speak("Car left.", "en", "M6")), "unknown-voice");
    assert_eq!(code(engine.speak("Voiture à gauche.", "fr", "M1")), "unknown-language");
    assert_eq!(code(engine.speak("  \n ", "de", "M1")), "empty-text");
    assert_eq!(engine.speak("Auto \u{e000} links.", "de", "M1"), Err(VoiceError::UnsupportedText("\u{e000}".into())));
    let dir = temp_dir("render-errors");
    assert_eq!(render_library(&mut engine, "X1", "de", &dir, |_, _| {}).unwrap_err().code(), "unknown-voice");
    assert!(!dir.exists(), "nothing written for a refused voice");
}

#[test]
fn a_missing_model_file_is_named() {
    let dir = temp_dir("no-model");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(Engine::load(&dir, 2).err(), Some(VoiceError::ModelMissing("LICENSE")));
    std::fs::remove_dir_all(&dir).unwrap();
    let Some(model) = model_dir() else { return };
    // The real model with one file "missing": a copy of the small files only.
    let partial = temp_dir("partial-model");
    for file in dd_voice::MODEL_FILES.iter().filter(|f| f.size < 1_000_000) {
        let to = partial.join(file.path);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(Path::new(&model).join(file.path), to).unwrap();
    }
    assert_eq!(Engine::load(&partial, 2).err(), Some(VoiceError::ModelMissing("onnx/duration_predictor.onnx")));
    assert_eq!(dd_voice::verify(&model, false), Ok(()));
    std::fs::remove_dir_all(&partial).unwrap();
}
