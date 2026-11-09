//! The team radio's ears: OpenAI's Whisper (MIT) turning the driver's push-to-talk question into text on the CPU,
//! through the same ONNX Runtime as the voice. The model is Hugging Face's ONNX export (`onnx-community`, see
//! [`crate::LISTEN_MODEL_FILES`]): an encoder and a merged decoder with past keys and values.
//!
//! What a transcription does, in the order of OpenAI's `whisper/decoding.py` and Hugging Face's `generate`:
//! - the log-mel spectrogram of the recording padded to 30 s (`audio::LogMel`; the export only takes 30 s);
//! - the encoder once;
//! - the decoder on the prompt `<|startoftranscript|> <|de|> <|transcribe|> <|notimestamps|>` (the radio's
//!   language is forced, no detection), then one token per step with the cached keys and values, greedy, until
//!   `<|endoftext|>`; the generation config's `suppress_tokens` are never chosen, its `begin_suppress_tokens`
//!   (blank, end of text) not as the first token;
//! - the text: the tokens' bytes as UTF-8, trimmed, runs of spaces as one.
//!
//! Silence and noise must give no text (Whisper invents "Untertitel der Amara.org-Community" or "Thank you." for
//! them). Three guards, cheapest first, see [`Verdict`]:
//! - [`Verdict::Quiet`]: the loudest 20 ms of the recording are below [`GATE_DBFS`]; the model does not run.
//! - [`Verdict::NoSpeech`]: OpenAI's rule from `transcribe()`: the model's no-speech probability (`<|nocaptions|>`
//!   after the start token) above 0.6 and the average log-probability of the tokens below -1.
//! - [`Verdict::Runaway`]: no end of text within [`max_tokens`] for the recording's length (a loop).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor, TensorRef};
use serde::Deserialize;

use crate::audio::{LogMel, FRAMES, WINDOW};
use crate::model::{verify_listener, LISTEN_DECODER, LISTEN_ENCODER};
use crate::tokens::Vocab;
use crate::{environment, runtime, VoiceError, LANGUAGES};

/// Below this level (dBFS, RMS of the loudest 20 ms) a recording counts as silence without running the model.
pub const GATE_DBFS: f32 = -50.0;
/// OpenAI's `no_speech_threshold` and `logprob_threshold` (`whisper/transcribe.py`).
pub const NO_SPEECH: f32 = 0.6;
pub const LOGPROB: f32 = -1.0;

/// The most tokens a recording of `samples` (16 kHz) may give: 16 plus 10 per second, at most 224 (OpenAI's
/// `sample_len`). Speech gives about 4 per second (German questions of the test set: 3-6), so only a loop reaches it.
pub fn max_tokens(samples: usize) -> usize {
    (16 + samples * 10 / crate::audio::RATE as usize).min(224)
}

/// Why the text is what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Speech,
    /// Too quiet to be speech: the model did not run.
    Quiet,
    /// The model says nobody spoke.
    NoSpeech,
    /// No end of text within the token limit.
    Runaway,
}

/// One transcription with what the decision was based on.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    /// What was said: `heard` for [`Verdict::Speech`], empty otherwise.
    pub text: String,
    /// What the model wrote, before the guards.
    pub heard: String,
    /// The generated tokens after the prompt, without `<|endoftext|>`.
    pub tokens: Vec<u32>,
    /// Probability of `<|nocaptions|>` right after `<|startoftranscript|>`.
    pub no_speech: f32,
    /// Sum of the chosen tokens' log-probabilities (end of text included) over their count plus one, as OpenAI.
    pub avg_logprob: f32,
    /// RMS of the loudest 20 ms, dBFS.
    pub level_dbfs: f32,
    pub verdict: Verdict,
}

#[derive(Deserialize)]
struct ModelConfig {
    num_mel_bins: usize,
    decoder_layers: usize,
    decoder_attention_heads: usize,
    d_model: usize,
}

#[derive(Deserialize)]
struct GenerationConfig {
    suppress_tokens: Vec<u32>,
    begin_suppress_tokens: Vec<u32>,
}

/// The special tokens the prompt and the guards use, from `added_tokens.json` (`<|endoftext|>` from `vocab.json`).
struct Special {
    start: u32,
    end: u32,
    transcribe: u32,
    no_timestamps: u32,
    no_speech: u32,
    languages: [u32; 2],
}

pub struct Listener {
    _env: ort::environment::Environment,
    encoder: Session,
    decoder: Session,
    mel: LogMel,
    n_mels: usize,
    vocab: Vocab,
    special: Special,
    suppress: Vec<u32>,
    begin_suppress: Vec<u32>,
    layers: usize,
    heads: usize,
    head_dim: usize,
}

/// RMS of the loudest 20 ms (320 samples, hop 160) in dBFS; -inf for digital silence.
fn loudest_dbfs(samples: &[f32]) -> f32 {
    let frame = 320.min(samples.len().max(1));
    let mut loudest = 0f32;
    let mut start = 0;
    while start + frame <= samples.len() {
        let w = &samples[start..start + frame];
        loudest = loudest.max(w.iter().map(|s| s * s).sum::<f32>() / frame as f32);
        start += 160;
    }
    10.0 * loudest.log10()
}

/// `text` trimmed, every run of whitespace one space.
fn tidy(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn log_softmax_at(logits: &[f32], index: usize) -> f32 {
    let max = logits.iter().fold(f32::NEG_INFINITY, |m, &x| m.max(x));
    let sum: f64 = logits.iter().map(|&x| ((x - max) as f64).exp()).sum();
    logits[index] - max - sum.ln() as f32
}

/// The best token of `logits` with `banned` left out, and its log-probability among the others (OpenAI applies
/// the suppression before its `log_softmax`).
fn pick(logits: &mut [f32], banned: &[&[u32]]) -> (u32, f32) {
    for list in banned {
        for &id in *list {
            if let Some(x) = logits.get_mut(id as usize) {
                *x = f32::NEG_INFINITY;
            }
        }
    }
    // The first of equal maxima, as argmax.
    let best = logits.iter().enumerate().fold(0, |b, (i, &x)| if x > logits[b] { i } else { b });
    (best as u32, log_softmax_at(logits, best))
}

impl Listener {
    /// The sample rate `transcribe` takes.
    pub const RATE: u32 = crate::audio::RATE;

    /// Loads the Whisper sessions from `model_dir`; `threads` = intra-op threads (the app uses 2), inter-op 1.
    /// Checks that every model file is there with its size first (not its SHA-256: see [`verify_listener`]).
    pub fn load(model_dir: &Path, threads: usize) -> Result<Self, VoiceError> {
        verify_listener(model_dir, false)?;
        Self::load_with(model_dir, LISTEN_ENCODER, LISTEN_DECODER, threads)
    }

    /// Like [`Listener::load`] with other ONNX files of the same export (`onnx/encoder_model.onnx`, ..) and no size
    /// check: for comparing the export's variants (`examples/listen_eval.rs`).
    #[doc(hidden)]
    pub fn load_with(model_dir: &Path, encoder: &'static str, decoder: &'static str, threads: usize) -> Result<Self, VoiceError> {
        let read = |file: &'static str| std::fs::read(model_dir.join(file)).map_err(|_| VoiceError::ModelMissing(file));
        fn parse<T: serde::de::DeserializeOwned>(bytes: Vec<u8>, file: &'static str) -> Result<T, VoiceError> {
            serde_json::from_slice(&bytes).map_err(|_| VoiceError::ModelDamaged(file))
        }
        let config: ModelConfig = parse(read("config.json")?, "config.json")?;
        let generation: GenerationConfig = parse(read("generation_config.json")?, "generation_config.json")?;
        let added: HashMap<String, u32> = parse(read("added_tokens.json")?, "added_tokens.json")?;
        let vocab: HashMap<String, u32> = parse(read("vocab.json")?, "vocab.json")?;
        let id = |token: &str| added.get(token).copied().ok_or(VoiceError::ModelDamaged("added_tokens.json"));
        let special = Special {
            start: id("<|startoftranscript|>")?,
            // The first special token is the last entry of the vocabulary.
            end: vocab.get("<|endoftext|>").copied().ok_or(VoiceError::ModelDamaged("vocab.json"))?,
            transcribe: id("<|transcribe|>")?,
            no_timestamps: id("<|notimestamps|>")?,
            no_speech: id("<|nocaptions|>")?,
            languages: [id("<|de|>")?, id("<|en|>")?],
        };
        let vocab = Vocab::new(&vocab, special.end).ok_or(VoiceError::ModelDamaged("vocab.json"))?;
        if !config.d_model.is_multiple_of(config.decoder_attention_heads) {
            return Err(VoiceError::ModelDamaged("config.json"));
        }

        let env = environment()?;
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
        Ok(Listener {
            encoder: session(encoder)?,
            decoder: session(decoder)?,
            mel: LogMel::new(config.num_mel_bins),
            n_mels: config.num_mel_bins,
            vocab,
            special,
            suppress: generation.suppress_tokens,
            begin_suppress: generation.begin_suppress_tokens,
            layers: config.decoder_layers,
            heads: config.decoder_attention_heads,
            head_dim: config.d_model / config.decoder_attention_heads,
            _env: env,
        })
    }

    /// What was said in a recording: mono f32 samples at [`Listener::RATE`] (16,000 Hz), up to 30 s (Whisper's
    /// window; anything after is not heard). `lang` is "de" or "en", the radio's language. Empty text for silence
    /// and noise only.
    pub fn transcribe(&mut self, samples: &[f32], lang: &str) -> Result<String, VoiceError> {
        Ok(self.transcript(samples, lang)?.text)
    }

    /// [`Listener::transcribe`] with the numbers behind it.
    pub fn transcript(&mut self, samples: &[f32], lang: &str) -> Result<Transcript, VoiceError> {
        let language = LANGUAGES.iter().position(|l| *l == lang).ok_or_else(|| VoiceError::UnknownLanguage(lang.to_string()))?;
        let samples = &samples[..samples.len().min(WINDOW)];
        let level_dbfs = loudest_dbfs(samples);
        let mut out = Transcript { text: String::new(), heard: String::new(), tokens: Vec::new(), no_speech: 1.0, avg_logprob: f32::NEG_INFINITY, level_dbfs, verdict: Verdict::Quiet };
        // Digital silence is -inf dBFS.
        if level_dbfs < GATE_DBFS {
            return Ok(out);
        }

        let mel = self.mel.compute(samples);
        let mut encoded = self
            .encoder
            .run(ort::inputs! { "input_features" => TensorRef::from_array_view(([1, self.n_mels, FRAMES], &mel[..])).map_err(runtime)? })
            .map_err(runtime)?;
        let hidden = encoded.remove("last_hidden_state").ok_or_else(|| runtime("the encoder gave no last_hidden_state"))?;
        drop(encoded);

        let s = &self.special;
        let prompt = [s.start as i64, s.languages[language] as i64, s.transcribe as i64, s.no_timestamps as i64];
        let (end, no_speech_id) = (s.end, s.no_speech);
        let limit = max_tokens(samples.len());
        // Keys and values: of the decoder's own tokens (grow every step), of the encoder output (fixed after step 1).
        let empty = || Tensor::<f32>::from_array(([1, self.heads, 0, self.head_dim], Vec::<f32>::new())).map(DynValue::from).map_err(runtime);
        let mut own: Vec<DynValue> = (0..2 * self.layers).map(|_| empty()).collect::<Result<_, _>>()?;
        let mut cross: Vec<DynValue> = (0..2 * self.layers).map(|_| empty()).collect::<Result<_, _>>()?;
        let (mut sum_logprob, mut ids) = (0f32, prompt.to_vec());
        let mut ended = false;
        for step in 0..=limit {
            let first = step == 0;
            let fed = if first { &ids[..] } else { &ids[ids.len() - 1..] };
            let mut inputs: Vec<(Cow<str>, SessionInputValue)> = vec![
                ("input_ids".into(), TensorRef::from_array_view(([1, fed.len()], fed)).map_err(runtime)?.into()),
                ("encoder_hidden_states".into(), (&hidden).into()),
                ("use_cache_branch".into(), Tensor::<bool>::from_array(([1], vec![!first])).map_err(runtime)?.into()),
            ];
            for layer in 0..self.layers {
                for (i, kv) in ["key", "value"].iter().enumerate() {
                    inputs.push((format!("past_key_values.{layer}.decoder.{kv}").into(), (&own[2 * layer + i]).into()));
                    inputs.push((format!("past_key_values.{layer}.encoder.{kv}").into(), (&cross[2 * layer + i]).into()));
                }
            }
            let mut result = self.decoder.run(inputs).map_err(runtime)?;
            let (shape, logits) = result["logits"].try_extract_tensor::<f32>().map_err(runtime)?;
            let vocab = shape[2] as usize;
            let mut last = logits[logits.len() - vocab..].to_vec();
            if first {
                let at_start = &logits[..vocab];
                out.no_speech = log_softmax_at(at_start, no_speech_id as usize).exp();
            }
            for layer in 0..self.layers {
                for (i, kv) in ["key", "value"].iter().enumerate() {
                    let missing = || runtime(format!("the decoder gave no present.{layer}.*.{kv}"));
                    own[2 * layer + i] = result.remove(format!("present.{layer}.decoder.{kv}")).ok_or_else(missing)?;
                    if first {
                        cross[2 * layer + i] = result.remove(format!("present.{layer}.encoder.{kv}")).ok_or_else(missing)?;
                    }
                }
            }
            drop(result);
            if step == limit {
                break;
            }
            let banned: &[&[u32]] = if first { &[&self.suppress, &self.begin_suppress] } else { &[&self.suppress] };
            let (token, logprob) = pick(&mut last, banned);
            sum_logprob += logprob;
            if token == end {
                ended = true;
                break;
            }
            ids.push(token as i64);
        }
        out.tokens = ids[prompt.len()..].iter().map(|&t| t as u32).collect();
        out.avg_logprob = sum_logprob / (out.tokens.len() + 1) as f32;
        out.heard = tidy(&self.vocab.decode(&out.tokens));
        out.verdict = if !ended {
            Verdict::Runaway
        } else if out.no_speech > NO_SPEECH && out.avg_logprob < LOGPROB {
            Verdict::NoSpeech
        } else {
            Verdict::Speech
        };
        if out.verdict == Verdict::Speech {
            out.text = out.heard.clone();
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppressed_tokens_are_never_picked() {
        let mut logits = vec![0.0, 5.0, 1.0, 4.0];
        assert_eq!(pick(&mut logits.clone(), &[]).0, 1);
        let (token, logprob) = pick(&mut logits, &[&[1], &[7]]);
        assert_eq!(token, 3, "the best one left; an id outside the vocabulary is ignored");
        let expected = 4.0 - (1f32 + 1f32.exp() + 4f32.exp()).ln();
        assert!((logprob - expected).abs() < 1e-6, "{logprob} vs {expected}: among the tokens left");
        assert_eq!(pick(&mut [2.0, 2.0, 1.0], &[]).0, 0, "the first of equal maxima");
    }

    #[test]
    fn the_level_is_the_loudest_20_ms() {
        assert_eq!(loudest_dbfs(&[0.0; 16_000]), f32::NEG_INFINITY);
        assert_eq!(loudest_dbfs(&[]), f32::NEG_INFINITY);
        let mut x = vec![0.001f32; 16_000];
        x[8000..8320].fill(0.1);
        assert!((loudest_dbfs(&x) + 20.0).abs() < 0.01, "{}", loudest_dbfs(&x));
        assert!((loudest_dbfs(&[0.5; 100]) + 6.02).abs() < 0.01, "shorter than a frame");
    }

    #[test]
    fn text_is_trimmed_and_spaces_collapse() {
        assert_eq!(tidy("  Wie viel   Sprit\nnoch? "), "Wie viel Sprit noch?");
        assert_eq!(tidy(" "), "");
    }

    #[test]
    fn the_token_limit_grows_with_the_recording() {
        assert_eq!(max_tokens(0), 16);
        assert_eq!(max_tokens(3 * 16_000), 46);
        assert_eq!(max_tokens(30 * 16_000), 224);
    }
}
