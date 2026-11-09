//! The team radio's voice: Supertonic 3 (Supertone Inc., model under BigScience OpenRAIL-M) speaking German and
//! English on the CPU through ONNX Runtime. Everything that touches ONNX lives here; the words come from
//! `dd_core::radio::phrases`.
//!
//! The pipeline is the Python SDK's (supertonic 1.3.1, MIT), ported step by step: text to chunks of sentences
//! (`text::chunks`), per chunk the normalised text as unicode indices, the duration predictor, the text encoder,
//! eight flow-matching steps of the vector estimator from Gaussian noise, the vocoder, and 0.3 s of silence between
//! chunks. Where it differs:
//! - Deterministic: the SDK draws the starting noise from NumPy's unseeded global generator, so the same sentence
//!   sounds slightly different every time. Here the noise comes from a SplitMix64 generator seeded with [`SEED`] at
//!   the start of every `speak`, so the same text, voice and language give the same samples on one machine.
//! - Each chunk is cut at its predicted duration, as supertone-inc's own Rust example does; the SDK returns the
//!   vocoder's padded output, up to 3,071 samples (70 ms) longer.
//! - Only "de" and "en": the model knows 31 languages, the radio speaks two.

mod library;
mod model;
mod text;

use std::path::Path;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;
use serde::Deserialize;

pub use library::{load_clip, render_library, render_names, Rendered};
pub use model::{verify, ModelFile, LICENCE_NOTICE, MODEL_FILES, MODEL_REVISION};

/// The voices the model ships with.
pub const VOICES: [&str; 10] = ["M1", "M2", "M3", "M4", "M5", "F1", "F2", "F3", "F4", "F5"];
/// The languages the radio speaks.
pub const LANGUAGES: [&str; 2] = ["de", "en"];
/// The SDK's defaults (`DEFAULT_TOTAL_STEPS`, `DEFAULT_SPEED`, `DEFAULT_SILENCE_DURATION`).
pub const STEPS: usize = 8;
pub const SPEED: f32 = 1.05;
const SILENCE_S: f32 = 0.3;
/// The seed of the starting noise of every utterance.
pub const SEED: u64 = 1;
/// What the cached sounds depend on: model revision and the settings above. `dd_core::radio::phrases::MODEL` must
/// say the same (a test checks it), because the cache names are made from it.
pub fn model_id() -> String {
    format!("supertonic-3@{MODEL_REVISION}/steps-{STEPS}/speed-{SPEED}/seed-{SEED}")
}

/// Why the voice could not speak. The code goes to the hosted interface, which has the words for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceError {
    /// A model file is not there; names it (its path below the model folder).
    ModelMissing(&'static str),
    /// A model file has the wrong size or SHA-256, or cannot be read as the model's; names it.
    ModelDamaged(&'static str),
    UnknownVoice(String),
    UnknownLanguage(String),
    EmptyText,
    /// The text holds characters the model has no index for; lists them.
    UnsupportedText(String),
    /// ONNX Runtime failed to load or run a model.
    Runtime(String),
    /// The cache folder could not be written or read.
    Io(String),
}

impl VoiceError {
    pub fn code(&self) -> String {
        match self {
            VoiceError::ModelMissing(file) => format!("model-missing:{file}"),
            VoiceError::ModelDamaged(file) => format!("model-damaged:{file}"),
            VoiceError::UnknownVoice(_) => "unknown-voice".to_string(),
            VoiceError::UnknownLanguage(_) => "unknown-language".to_string(),
            VoiceError::EmptyText => "empty-text".to_string(),
            VoiceError::UnsupportedText(_) => "unsupported-text".to_string(),
            VoiceError::Runtime(_) => "runtime".to_string(),
            VoiceError::Io(_) => "io".to_string(),
        }
    }
}

fn runtime(e: impl std::fmt::Display) -> VoiceError {
    VoiceError::Runtime(e.to_string())
}

/// A voice as the model takes it: the style vectors of the text-to-latent model and of the duration predictor.
struct Style {
    ttl: Vec<f32>,
    ttl_shape: [usize; 3],
    dp: Vec<f32>,
    dp_shape: [usize; 3],
}

#[derive(Deserialize)]
struct StyleFile {
    style_ttl: StyleTensor,
    style_dp: StyleTensor,
}

#[derive(Deserialize)]
struct StyleTensor {
    dims: Vec<usize>,
    data: Vec<Vec<Vec<f32>>>,
}

impl StyleTensor {
    fn flat(self) -> Option<(Vec<f32>, [usize; 3])> {
        let shape: [usize; 3] = self.dims.try_into().ok()?;
        let flat: Vec<f32> = self.data.into_iter().flatten().flatten().collect();
        (flat.len() == shape.iter().product::<usize>()).then_some((flat, shape))
    }
}

/// The parts of `onnx/tts.json` the pipeline needs.
#[derive(Deserialize)]
struct Config {
    ae: AeConfig,
    ttl: TtlConfig,
}

#[derive(Deserialize)]
struct AeConfig {
    sample_rate: u32,
    base_chunk_size: usize,
}

#[derive(Deserialize)]
struct TtlConfig {
    chunk_compress_factor: usize,
    latent_dim: usize,
}

/// The starting noise: SplitMix64 and Box-Muller, in f64, both values of each pair used.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn fill(&mut self, out: &mut [f32]) {
        for pair in out.chunks_mut(2) {
            let (u1, u2) = (1.0 - self.uniform(), self.uniform());
            let (r, t) = ((-2.0 * u1.ln()).sqrt(), std::f64::consts::TAU * u2);
            pair[0] = (r * t.cos()) as f32;
            if let Some(second) = pair.get_mut(1) {
                *second = (r * t.sin()) as f32;
            }
        }
    }
}

pub struct Engine {
    _env: ort::environment::Environment,
    duration: Session,
    encoder: Session,
    estimator: Session,
    vocoder: Session,
    indexer: Vec<i64>,
    styles: Vec<Style>,
    /// Samples per latent frame (base chunk size x compress factor) and channels of a latent frame.
    frame: usize,
    channels: usize,
}

impl Engine {
    /// The sample rate of what `speak` returns.
    pub const RATE: u32 = 44_100;

    /// Loads the four ONNX sessions from `model_dir`; `threads` = intra-op threads (the app uses 2), inter-op 1.
    /// Checks that every model file is there with its size first (not its SHA-256: see [`verify`]).
    pub fn load(model_dir: &Path, threads: usize) -> Result<Self, VoiceError> {
        verify(model_dir, false)?;
        let read = |file: &'static str| std::fs::read(model_dir.join(file)).map_err(|_| VoiceError::ModelMissing(file));
        let json = |file: &'static str| -> Result<serde_json::Value, VoiceError> {
            serde_json::from_slice(&read(file)?).map_err(|_| VoiceError::ModelDamaged(file))
        };
        let config: Config = serde_json::from_value(json("onnx/tts.json")?).map_err(|_| VoiceError::ModelDamaged("onnx/tts.json"))?;
        if config.ae.sample_rate != Self::RATE {
            return Err(VoiceError::ModelDamaged("onnx/tts.json"));
        }
        let indexer: Vec<i64> =
            serde_json::from_value(json("onnx/unicode_indexer.json")?).map_err(|_| VoiceError::ModelDamaged("onnx/unicode_indexer.json"))?;
        let mut styles = Vec::with_capacity(VOICES.len());
        for file in model::VOICE_FILES {
            let style: StyleFile = serde_json::from_slice(&read(file)?).map_err(|_| VoiceError::ModelDamaged(file))?;
            let ((ttl, ttl_shape), (dp, dp_shape)) = style.style_ttl.flat().zip(style.style_dp.flat()).ok_or(VoiceError::ModelDamaged(file))?;
            styles.push(Style { ttl, ttl_shape, dp, dp_shape });
        }
        // ONNX Runtime's telemetry (Windows ETW events) off.
        let env = ort::init().with_name("dd-voice").with_telemetry(false).build().map_err(runtime)?;
        let session = |file: &str| -> Result<Session, VoiceError> {
            Session::builder(&env)
                .map_err(runtime)?
                .with_optimization_level(GraphOptimizationLevel::All)
                .map_err(runtime)?
                .with_intra_threads(threads.max(1))
                .map_err(runtime)?
                .with_inter_threads(1)
                .map_err(runtime)?
                .commit_from_file(model_dir.join(file))
                .map_err(runtime)
        };
        Ok(Engine {
            duration: session("onnx/duration_predictor.onnx")?,
            encoder: session("onnx/text_encoder.onnx")?,
            estimator: session("onnx/vector_estimator.onnx")?,
            vocoder: session("onnx/vocoder.onnx")?,
            indexer,
            styles,
            frame: config.ae.base_chunk_size * config.ttl.chunk_compress_factor,
            channels: config.ttl.latent_dim * config.ttl.chunk_compress_factor,
            _env: env,
        })
    }

    /// The built-in voices: "M1".."M5", "F1".."F5".
    pub fn voices() -> &'static [&'static str] {
        &VOICES
    }

    /// One utterance as mono f32 samples at `Engine::RATE` (44,100 Hz). `lang` is "de" or "en". The same text,
    /// voice and language give the same samples.
    pub fn speak(&mut self, text: &str, lang: &str, voice: &str) -> Result<Vec<f32>, VoiceError> {
        let style = check(voice, lang)?;
        let chunks = text::chunks(text, text::MAX_CHUNK);
        if chunks.is_empty() {
            return Err(VoiceError::EmptyText);
        }
        let ids = chunks.iter().map(|c| self.ids(&text::preprocess(c, lang))).collect::<Result<Vec<_>, _>>()?;
        let mut noise = Noise(SEED);
        let mut out = Vec::new();
        for (i, ids) in ids.iter().enumerate() {
            if i > 0 {
                out.resize(out.len() + (SILENCE_S * Self::RATE as f32) as usize, 0.0);
            }
            out.extend(self.infer(ids, style, &mut noise)?);
        }
        Ok(out)
    }

    /// The model's indices of a preprocessed chunk; refuses characters it has no index for.
    fn ids(&self, text: &str) -> Result<Vec<i64>, VoiceError> {
        let index = |c: char| self.indexer.get(c as usize).copied().filter(|&i| i >= 0);
        let unsupported: String = text.chars().filter(|&c| index(c).is_none()).collect();
        if !unsupported.is_empty() {
            return Err(VoiceError::UnsupportedText(unsupported));
        }
        Ok(text.chars().filter_map(index).collect())
    }

    fn infer(&mut self, ids: &[i64], voice: usize, noise: &mut Noise) -> Result<Vec<f32>, VoiceError> {
        let style = &self.styles[voice];
        let n = ids.len();
        let mask = vec![1f32; n];
        let ids_t = || TensorRef::from_array_view(([1, n], ids)).map_err(runtime);
        let mask_t = || TensorRef::from_array_view(([1, 1, n], &mask[..])).map_err(runtime);
        let ttl_t = || TensorRef::from_array_view((style.ttl_shape, &style.ttl[..])).map_err(runtime);
        let dp_t = TensorRef::from_array_view((style.dp_shape, &style.dp[..])).map_err(runtime)?;

        let out = self.duration.run(ort::inputs! { "text_ids" => ids_t()?, "style_dp" => dp_t, "text_mask" => mask_t()? }).map_err(runtime)?;
        let seconds = out["duration"].try_extract_tensor::<f32>().map_err(runtime)?.1[0] / SPEED;
        drop(out);

        let out = self.encoder.run(ort::inputs! { "text_ids" => ids_t()?, "style_ttl" => ttl_t()?, "text_mask" => mask_t()? }).map_err(runtime)?;
        let (shape, emb) = out["text_emb"].try_extract_tensor::<f32>().map_err(runtime)?;
        let (emb_shape, emb) = (shape.iter().map(|&d| d as usize).collect::<Vec<_>>(), emb.to_vec());
        drop(out);

        // As the SDK computes it, in f32: the latent has ceil(samples / frame) frames.
        let samples = seconds * Self::RATE as f32;
        let frames = ((samples + (self.frame - 1) as f32) / self.frame as f32) as usize;
        let valid = (samples as usize).div_ceil(self.frame).min(frames);
        let mut latent_mask = vec![0f32; frames];
        latent_mask[..valid].fill(1.0);
        let mut latent = vec![0f32; self.channels * frames];
        noise.fill(&mut latent);
        for (i, x) in latent.iter_mut().enumerate() {
            *x *= latent_mask[i % frames];
        }

        let total = [STEPS as f32];
        for step in 0..STEPS {
            let current = [step as f32];
            let out = self
                .estimator
                .run(ort::inputs! {
                    "noisy_latent" => TensorRef::from_array_view(([1, self.channels, frames], &latent[..])).map_err(runtime)?,
                    "text_emb" => TensorRef::from_array_view((&emb_shape[..], &emb[..])).map_err(runtime)?,
                    "style_ttl" => ttl_t()?,
                    "text_mask" => mask_t()?,
                    "latent_mask" => TensorRef::from_array_view(([1, 1, frames], &latent_mask[..])).map_err(runtime)?,
                    "current_step" => TensorRef::from_array_view(([1], &current[..])).map_err(runtime)?,
                    "total_step" => TensorRef::from_array_view(([1], &total[..])).map_err(runtime)?,
                })
                .map_err(runtime)?;
            latent = out["denoised_latent"].try_extract_tensor::<f32>().map_err(runtime)?.1.to_vec();
        }

        let out = self
            .vocoder
            .run(ort::inputs! { "latent" => TensorRef::from_array_view(([1, self.channels, frames], &latent[..])).map_err(runtime)? })
            .map_err(runtime)?;
        let wav = out["wav_tts"].try_extract_tensor::<f32>().map_err(runtime)?.1;
        Ok(wav[..(samples as usize).min(wav.len())].to_vec())
    }
}

/// The index of `voice` in [`VOICES`], when `lang` is one the radio speaks.
fn check(voice: &str, lang: &str) -> Result<usize, VoiceError> {
    if !LANGUAGES.contains(&lang) {
        return Err(VoiceError::UnknownLanguage(lang.to_string()));
    }
    VOICES.iter().position(|v| *v == voice).ok_or_else(|| VoiceError::UnknownVoice(voice.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_names_of_dd_core_use_this_model_and_these_settings() {
        assert_eq!(model_id(), dd_core::radio::phrases::MODEL);
    }

    #[test]
    fn errors_have_codes() {
        assert_eq!(VoiceError::ModelMissing("onnx/vocoder.onnx").code(), "model-missing:onnx/vocoder.onnx");
        assert_eq!(VoiceError::UnknownVoice("X".into()).code(), "unknown-voice");
        assert_eq!(check("M1", "fr"), Err(VoiceError::UnknownLanguage("fr".into())));
        assert_eq!(check("m1", "de"), Err(VoiceError::UnknownVoice("m1".into())));
        assert_eq!(check("F5", "en"), Ok(9));
    }

    #[test]
    fn the_noise_is_standard_normal_and_repeats() {
        let (mut a, mut b) = (vec![0f32; 100_001], vec![0f32; 100_001]);
        Noise(SEED).fill(&mut a);
        Noise(SEED).fill(&mut b);
        assert_eq!(a, b);
        let mean = a.iter().map(|&x| x as f64).sum::<f64>() / a.len() as f64;
        let var = a.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / a.len() as f64;
        assert!(mean.abs() < 0.01 && (var - 1.0).abs() < 0.02, "mean {mean} var {var}");
        let mut c = vec![0f32; 4];
        Noise(SEED + 1).fill(&mut c);
        assert_ne!(&a[..4], &c[..]);
    }
}
