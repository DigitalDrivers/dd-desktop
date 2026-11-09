//! What the listener's examples and tests share: WAV files, the test sentences, mild pink noise and the word error
//! rate. Included with `#[path]` (examples and integration tests cannot share a module otherwise).
#![allow(dead_code)]

use std::f32::consts::LN_10;

/// The test sentences, `(lang, text)`: 12 German and 12 English driver questions.
pub const SENTENCES: &str = include_str!("../../tests/listen_sentences.tsv");
/// The voices that speak them.
pub const TEST_VOICES: [&str; 3] = ["M1", "F2", "M4"];
/// Silence around each spoken question, as a push-to-talk recording has it (s before, s after).
pub const PAD_S: (f32, f32) = (0.3, 0.4);
/// Signal-to-noise ratio of the noisy variant: pink noise 20 dB below the speech.
pub const SNR_DB: f32 = 20.0;

pub fn sentences() -> Vec<(&'static str, &'static str)> {
    SENTENCES.lines().filter(|l| !l.trim().is_empty()).map(|l| l.split_once('\t').expect("lang<TAB>text")).collect()
}

/// A 16-bit PCM mono WAV.
pub fn wav_bytes(samples: &[f32], rate: u32) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    for (value, bytes) in [(16u32, 4), (1, 2), (1, 2), (rate, 4), (rate * 2, 4), (2, 2), (16, 2)] {
        out.extend_from_slice(&value.to_le_bytes()[..bytes]);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

/// The samples (channels averaged) and the rate of a PCM 16/24/32-bit or float 32-bit WAV, as recorders write them.
pub fn read_wav(bytes: &[u8]) -> Result<(Vec<f32>, u32), String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let (mut format, mut pos) = (None, 12);
    while pos + 8 <= bytes.len() {
        let (id, size) = (&bytes[pos..pos + 4], u32_at(pos + 4) as usize);
        let body = pos + 8;
        let end = (body + size).min(bytes.len());
        if id == b"fmt " && size >= 16 {
            let mut tag = u16_at(body);
            if tag == 0xfffe && size >= 26 {
                tag = u16_at(body + 24); // WAVE_FORMAT_EXTENSIBLE: the sub-format's first two bytes
            }
            format = Some((tag, u16_at(body + 2) as usize, u32_at(body + 4), u16_at(body + 14)));
        } else if id == b"data" {
            let (tag, channels, rate, bits) = format.ok_or("data before fmt")?;
            let data = &bytes[body..end];
            let frame_samples: Vec<f32> = match (tag, bits) {
                (1, 16) => data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect(),
                (1, 24) => data.chunks_exact(3).map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0).collect(),
                (1, 32) => data.chunks_exact(4).map(|b| i32::from_le_bytes(b.try_into().unwrap()) as f32 / 2_147_483_648.0).collect(),
                (3, 32) => data.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect(),
                _ => return Err(format!("unsupported WAV format {tag}, {bits} bits")),
            };
            let mono = frame_samples.chunks_exact(channels.max(1)).map(|f| f.iter().sum::<f32>() / f.len() as f32).collect();
            return Ok((mono, rate));
        }
        pos = body + size + (size & 1);
    }
    Err("no data chunk".into())
}

/// SplitMix64.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [-1, 1).
    pub fn white(&mut self) -> f32 {
        ((self.next() >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| (s * s) as f64).sum::<f64>() / x.len().max(1) as f64).sqrt() as f32
}

/// `n` samples of pink noise (Paul Kellet's filter on white noise) with the given RMS.
pub fn pink(n: usize, level: f32, seed: u64) -> Vec<f32> {
    let mut rng = Rng(seed);
    let mut b = [0f32; 7];
    let raw: Vec<f32> = (0..n)
        .map(|_| {
            let w = rng.white();
            b[0] = 0.99886 * b[0] + w * 0.055_517_9;
            b[1] = 0.99332 * b[1] + w * 0.075_075_9;
            b[2] = 0.96900 * b[2] + w * 0.153_852;
            b[3] = 0.86650 * b[3] + w * 0.310_485_6;
            b[4] = 0.55000 * b[4] + w * 0.532_952_2;
            b[5] = -0.7616 * b[5] - w * 0.016_898;
            let p = b[..6].iter().sum::<f32>() + b[6] + w * 0.5362;
            b[6] = w * 0.115_926;
            p
        })
        .collect();
    let gain = level / rms(&raw);
    raw.into_iter().map(|x| x * gain).collect()
}

pub fn db_to_amplitude(db: f32) -> f32 {
    (db / 20.0 * LN_10).exp()
}

/// A spoken question as a push-to-talk recording: silence around it, and with `snr_db` pink noise at that distance
/// below the speech's RMS over the whole recording.
pub fn recording(speech_16k: &[f32], snr_db: Option<f32>, seed: u64) -> Vec<f32> {
    let before = vec![0f32; (PAD_S.0 * 16_000.0) as usize];
    let after = vec![0f32; (PAD_S.1 * 16_000.0) as usize];
    let mut out = [&before[..], speech_16k, &after[..]].concat();
    if let Some(snr) = snr_db {
        let noise = pink(out.len(), rms(speech_16k) / db_to_amplitude(snr), seed);
        out.iter_mut().zip(noise).for_each(|(s, n)| *s += n);
    }
    out
}

/// Words for scoring: lower case, punctuation off, number words as digits, British spellings as Whisper writes
/// them, "ß" as "ss". Applied to the spoken text and the transcript alike.
pub fn words(text: &str) -> Vec<String> {
    let numbers = [
        ("null", "0"), ("zero", "0"), ("eins", "1"), ("one", "1"), ("zwei", "2"), ("two", "2"), ("drei", "3"), ("three", "3"),
        ("vier", "4"), ("four", "4"), ("fünf", "5"), ("five", "5"), ("sechs", "6"), ("six", "6"), ("sieben", "7"),
        ("seven", "7"), ("acht", "8"), ("eight", "8"), ("neun", "9"), ("nine", "9"), ("zehn", "10"), ("ten", "10"),
    ];
    let spelling = [("tyres", "tires"), ("tyre", "tire")];
    text.to_lowercase()
        .replace('ß', "ss")
        .split(|c: char| c.is_whitespace() || c == '-')
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty())
        .map(|w| numbers.iter().chain(&spelling).find(|(a, _)| *a == w).map(|(_, b)| b.to_string()).unwrap_or(w))
        .collect()
}

/// Word-level edit distance (substitutions + insertions + deletions) of the transcript against the reference.
pub fn word_errors(reference: &[String], heard: &[String]) -> usize {
    let mut row: Vec<usize> = (0..=heard.len()).collect();
    for (i, r) in reference.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, h) in heard.iter().enumerate() {
            let cost = if r == h { prev } else { prev + 1 };
            prev = row[j + 1];
            row[j + 1] = cost.min(row[j] + 1).min(prev + 1);
        }
    }
    row[heard.len()]
}

/// Threads, CPU seconds, working set and peak working set (MB) of this process (as `bench.rs`).
pub fn stats() -> (u32, f64, f64, f64) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_errors_count_edits() {
        let w = |s: &str| words(s);
        assert_eq!(w("Wie viel Sprit, habe ich noch?"), ["wie", "viel", "sprit", "habe", "ich", "noch"]);
        assert_eq!(w("How are my tyres? Five laps."), ["how", "are", "my", "tires", "5", "laps"]);
        assert_eq!(w("Döttinger-Höhe heiß"), ["döttinger", "höhe", "heiss"]);
        assert_eq!(word_errors(&w("a b c d"), &w("a x c d e")), 2);
        assert_eq!(word_errors(&w("a b c"), &w("")), 3);
        assert_eq!(word_errors(&w(""), &w("a")), 1);
    }

    #[test]
    fn a_wav_reads_back() {
        let x: Vec<f32> = (0..100).map(|i| (i as f32 / 50.0) - 1.0).collect();
        let (y, rate) = read_wav(&wav_bytes(&x, 16_000)).unwrap();
        assert_eq!(rate, 16_000);
        assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-4));
    }

    #[test]
    fn pink_noise_has_its_level() {
        let n = pink(48_000, 0.01, 7);
        assert!((rms(&n) - 0.01).abs() < 1e-5);
    }
}
