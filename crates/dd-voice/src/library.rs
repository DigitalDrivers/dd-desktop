//! The rendered phrase library: every fixed wording spoken once by the chosen voice and kept as a dry 16-bit mono
//! WAV at 44.1 kHz (the radio effect is added at play time), named by `dd_core::radio::phrases::cache_name`, so a
//! changed wording, voice or model renders anew and the old files are simply no longer asked for.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dd_core::radio::phrases;

use crate::{check, Engine, VoiceError};

/// What a render call did.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The texts it spoke, in order.
    pub rendered: Vec<String>,
    /// How many were in the cache already.
    pub cached: usize,
    pub elapsed: Duration,
}

fn io(e: std::io::Error) -> VoiceError {
    VoiceError::Io(e.to_string())
}

fn clip_path(cache_dir: &Path, voice: &str, lang: &str, text: &str) -> PathBuf {
    cache_dir.join(format!("{}.wav", phrases::cache_name(voice, lang, text)))
}

/// Whether the sound of `text` is in `cache_dir` already.
pub fn is_rendered(cache_dir: &Path, voice: &str, lang: &str, text: &str) -> bool {
    clip_path(cache_dir, voice, lang, text).is_file()
}

const HEADER: usize = 44;

fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(HEADER + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    // PCM, mono, 44,100 Hz, 88,200 bytes per second, 2 bytes per frame, 16 bits.
    for (value, bytes) in [(16u32, 4), (1, 2), (1, 2), (Engine::RATE, 4), (Engine::RATE * 2, 4), (2, 2), (16, 2)] {
        out.extend_from_slice(&value.to_le_bytes()[..bytes]);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

/// The samples of a WAV this module wrote.
fn wav_samples(bytes: &[u8]) -> Option<Vec<f32>> {
    let format_ok = bytes.len() >= HEADER && &bytes[..4] == b"RIFF" && &bytes[8..16] == b"WAVEfmt " && &bytes[36..40] == b"data";
    let pcm16_mono = bytes.get(20..24) == Some(&[1, 0, 1, 0]) && bytes.get(34..36) == Some(&[16, 0]);
    let rate = u32::from_le_bytes(bytes.get(24..28)?.try_into().ok()?);
    let data = u32::from_le_bytes(bytes.get(40..44)?.try_into().ok()?) as usize;
    if !format_ok || !pcm16_mono || rate != Engine::RATE || bytes.len() != HEADER + data || !data.is_multiple_of(2) {
        return None;
    }
    Some(bytes[HEADER..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32767.0).collect())
}

/// Speaks every text that has no file in `cache_dir` yet and writes it there (through a temporary file, so an
/// interrupted render leaves no half clip).
fn render(engine: &mut Engine, voice: &str, lang: &str, texts: &[String], cache_dir: &Path, mut on_progress: impl FnMut(usize, usize)) -> Result<Rendered, VoiceError> {
    let start = Instant::now();
    check(voice, lang)?;
    fs::create_dir_all(cache_dir).map_err(io)?;
    let mut done = Rendered { rendered: Vec::new(), cached: 0, elapsed: Duration::ZERO };
    for (i, text) in texts.iter().enumerate() {
        let path = clip_path(cache_dir, voice, lang, text);
        if path.is_file() {
            done.cached += 1;
        } else {
            let samples = engine.speak(text, lang, voice)?;
            let part = path.with_extension("wav.part");
            fs::write(&part, wav_bytes(&samples)).map_err(io)?;
            fs::rename(&part, &path).map_err(io)?;
            done.rendered.push(text.clone());
        }
        on_progress(i + 1, texts.len());
    }
    done.elapsed = start.elapsed();
    Ok(done)
}

/// Renders every wording of every fixed call (`phrases::fixed_keys`) in `lang` that is not in `cache_dir` yet;
/// `on_progress(done, total)` after each wording.
pub fn render_library(engine: &mut Engine, voice: &str, lang: &str, cache_dir: &Path, on_progress: impl FnMut(usize, usize)) -> Result<Rendered, VoiceError> {
    let mut texts: Vec<String> = Vec::new();
    for key in phrases::fixed_keys() {
        for wording in phrases::wordings(key, lang) {
            if !texts.contains(wording) {
                texts.push(wording.clone());
            }
        }
    }
    render(engine, voice, lang, &texts, cache_dir, on_progress)
}

/// Renders the named calls (`phrases::named_keys`, every wording) with `name_text`, the driver's name as it is to
/// be spoken (the respelling if there is one).
pub fn render_names(engine: &mut Engine, voice: &str, lang: &str, name_text: &str, cache_dir: &Path) -> Result<Rendered, VoiceError> {
    if name_text.trim().is_empty() {
        return Err(VoiceError::EmptyText);
    }
    let mut texts = Vec::new();
    for key in phrases::named_keys() {
        for pick in 0..phrases::wordings(key, lang).len() {
            texts.extend(phrases::text(key, lang, Some(name_text), pick));
        }
    }
    render(engine, voice, lang, &texts, cache_dir, |_, _| {})
}

/// The cached clip of `text`, as the engine spoke it (to 16-bit precision); None if it is not rendered yet.
pub fn load_clip(cache_dir: &Path, voice: &str, lang: &str, text: &str) -> Option<Vec<f32>> {
    wav_samples(&fs::read(clip_path(cache_dir, voice, lang, text)).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_reads_back_what_was_written() {
        let samples: Vec<f32> = (0..1000).map(|i| ((i as f32) * 0.05).sin() * 0.8).collect();
        let bytes = wav_bytes(&[samples.as_slice(), &[1.5, -2.0]].concat());
        assert_eq!(bytes.len(), 44 + 2 * 1002);
        let back = wav_samples(&bytes).unwrap();
        assert!(samples.iter().zip(&back).all(|(a, b)| (a - b).abs() < 1.0 / 32767.0));
        assert_eq!(&back[1000..], &[1.0, -1.0], "clipped");
        assert_eq!(wav_samples(&bytes[..100]), None);
        let mut stereo = bytes.clone();
        stereo[22] = 2;
        assert_eq!(wav_samples(&stereo), None);
    }

    #[test]
    fn load_clip_finds_nothing_in_an_empty_cache() {
        assert_eq!(load_clip(Path::new("/nonexistent-dd-voice"), "M1", "de", "Auto links."), None);
    }
}
