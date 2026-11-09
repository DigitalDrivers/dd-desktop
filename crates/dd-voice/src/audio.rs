//! The listener's front end: samples brought to 16 kHz (`resample`) and the log-mel spectrogram Whisper reads,
//! computed the way OpenAI's `whisper/audio.py` and Hugging Face's `WhisperFeatureExtractor` do: the recording
//! padded with zeros to 30 s, a centred STFT (reflect padding, periodic Hann window of 400 samples, hop 160), the
//! power spectrum through 80 Slaney mel filters (librosa's `filters.mel`), `log10` with a floor of 1e-10, clamped
//! to 8 below its maximum and scaled to `(x + 4) / 4`. The FFT is a mixed-radix Cooley-Tukey (400 = 2^4 x 5^2).

use std::f64::consts::PI;

/// The listener's sample rate.
pub const RATE: u32 = 16_000;
/// Whisper's window: 30 s.
pub const WINDOW: usize = 30 * RATE as usize;
const N_FFT: usize = 400;
const HOP: usize = 160;
const BINS: usize = N_FFT / 2 + 1;
/// Frames in the 30 s window (the STFT gives one more; the reference drops the last).
pub const FRAMES: usize = WINDOW / HOP;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    fn mul(self, o: Complex) -> Complex {
        Complex { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
}

/// `e^(-2 pi i k / n)` for k in 0..n.
fn twiddles(n: usize) -> Vec<Complex> {
    (0..n).map(|k| -2.0 * PI * k as f64 / n as f64).map(|a| Complex { re: a.cos(), im: a.sin() }).collect()
}

/// The DFT of `x[0], x[stride], ..` (`out.len()` values) into `out`. `tw` are the twiddles of the full length,
/// `tw.len() / out.len()` apart for this length. Splits by the smallest factor p into p interleaved DFTs and combines
/// them; a prime length is a plain DFT.
fn fft(x: &[Complex], stride: usize, tw: &[Complex], out: &mut [Complex]) {
    let n = out.len();
    if n == 1 {
        out[0] = x[0];
        return;
    }
    let p = (2..=n).find(|&p| n.is_multiple_of(p)).unwrap();
    let m = n / p;
    for r in 0..p {
        fft(&x[r * stride..], stride * p, tw, &mut out[r * m..(r + 1) * m]);
    }
    let step = tw.len() / n;
    let mut column = [Complex::default(); 8];
    let mut column_vec;
    let column: &mut [Complex] = if p <= 8 {
        &mut column[..p]
    } else {
        column_vec = vec![Complex::default(); p];
        &mut column_vec
    };
    for k in 0..m {
        for (r, c) in column.iter_mut().enumerate() {
            *c = out[r * m + k];
        }
        for q in 0..p {
            let mut sum = Complex::default();
            for (r, c) in column.iter().enumerate() {
                let w = tw[(r * (k + q * m) % n) * step];
                let t = c.mul(w);
                sum.re += t.re;
                sum.im += t.im;
            }
            out[k + q * m] = sum;
        }
    }
}

/// Slaney's mel scale (librosa's default, `htk=False`).
fn hz_to_mel(hz: f64) -> f64 {
    if hz >= 1000.0 {
        15.0 + (hz / 1000.0).ln() * 27.0 / 6.4f64.ln()
    } else {
        3.0 * hz / 200.0
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    if mel >= 15.0 {
        1000.0 * (6.4f64.ln() / 27.0 * (mel - 15.0)).exp()
    } else {
        200.0 * mel / 3.0
    }
}

/// librosa's `filters.mel(sr=16000, n_fft=400, n_mels)` with Slaney normalisation: `n_mels` rows of `BINS` weights.
pub(crate) fn mel_filters(n_mels: usize) -> Vec<f64> {
    let top = hz_to_mel(RATE as f64 / 2.0);
    let edges: Vec<f64> = (0..n_mels + 2).map(|i| mel_to_hz(top * i as f64 / (n_mels + 1) as f64)).collect();
    let mut w = vec![0.0; n_mels * BINS];
    for m in 0..n_mels {
        let (lo, mid, hi) = (edges[m], edges[m + 1], edges[m + 2]);
        let norm = 2.0 / (hi - lo);
        for b in 0..BINS {
            let f = b as f64 * (RATE as f64 / 2.0) / (BINS - 1) as f64;
            let rise = (f - lo) / (mid - lo);
            let fall = (hi - f) / (hi - mid);
            w[m * BINS + b] = rise.min(fall).max(0.0) * norm;
        }
    }
    w
}

/// The log-mel spectrogram of the 30 s window: `n_mels` rows of `FRAMES` values (the encoder's `[1, n_mels, 3000]`).
/// Longer recordings are cut at 30 s.
pub(crate) struct LogMel {
    filters: Vec<f64>,
    n_mels: usize,
    window: Vec<f64>,
    tw: Vec<Complex>,
}

impl LogMel {
    pub fn new(n_mels: usize) -> Self {
        // Periodic Hann (torch.hann_window, HF window_function(periodic=True)).
        let window = (0..N_FFT).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / N_FFT as f64).cos()).collect();
        LogMel { filters: mel_filters(n_mels), n_mels, window, tw: twiddles(N_FFT) }
    }

    pub fn compute(&self, samples: &[f32]) -> Vec<f32> {
        let len = samples.len().min(WINDOW);
        // The zero-padded window with N_FFT / 2 reflected at both ends (numpy's "reflect": the edge sample once).
        let half = N_FFT / 2;
        let at = |i: usize| -> f64 { if i < len { samples[i] as f64 } else { 0.0 } };
        let padded: Vec<f64> = (0..WINDOW + 2 * half)
            .map(|j| match j {
                j if j < half => at(half - j),
                j if j >= half + WINDOW => at(2 * (WINDOW - 1) + half - j),
                j => at(j - half),
            })
            .collect();
        let mut mel = vec![0f64; self.n_mels * FRAMES];
        let (mut frame, mut spectrum, mut power) = (vec![Complex::default(); N_FFT], vec![Complex::default(); N_FFT], vec![0f64; BINS]);
        for t in 0..FRAMES {
            let chunk = &padded[t * HOP..t * HOP + N_FFT];
            // A frame of zero padding has no power: every filter gives 0, which the floor below turns into 1e-10.
            if chunk.iter().all(|&s| s == 0.0) {
                continue;
            }
            for (f, (&s, &w)) in frame.iter_mut().zip(chunk.iter().zip(&self.window)) {
                *f = Complex { re: s * w, im: 0.0 };
            }
            fft(&frame, 1, &self.tw, &mut spectrum);
            for (p, c) in power.iter_mut().zip(&spectrum) {
                *p = c.re * c.re + c.im * c.im;
            }
            for m in 0..self.n_mels {
                let row = &self.filters[m * BINS..(m + 1) * BINS];
                mel[m * FRAMES + t] = row.iter().zip(&power).map(|(w, p)| w * p).sum();
            }
        }
        let mut log: Vec<f64> = mel.iter().map(|&m| m.max(1e-10).log10()).collect();
        let floor = log.iter().fold(f64::MIN, |a, &b| a.max(b)) - 8.0;
        for x in &mut log {
            *x = (x.max(floor) + 4.0) / 4.0;
        }
        log.into_iter().map(|x| x as f32).collect()
    }
}

/// The log-mel spectrogram of a recording at 16 kHz, `n_mels` x 3000 (for comparing with the reference).
#[doc(hidden)]
pub fn log_mel(samples: &[f32], n_mels: usize) -> Vec<f32> {
    LogMel::new(n_mels).compute(samples)
}

/// Mono samples at `from_rate` brought to 16 kHz: windowed-sinc interpolation (Hann window, 16 zero crossings each
/// side), low-passed at 95 % of the lower Nyquist frequency so a 44.1 kHz voice does not fold above 8 kHz back into
/// the band. One coefficient table per output phase of the rational ratio (44.1 kHz: 160 phases of 89 taps).
pub fn resample(samples: &[f32], from_rate: u32) -> Vec<f32> {
    if from_rate == RATE || samples.is_empty() {
        return samples.to_vec();
    }
    let gcd = |mut a: u64, mut b: u64| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let g = gcd(from_rate as u64, RATE as u64);
    let (up, down) = (RATE as u64 / g, from_rate as u64 / g);
    // Cut-off in cycles per input sample, and the filter's half length in input samples.
    let cutoff = 0.5 * 0.95 * (RATE as f64 / from_rate as f64).min(1.0);
    const ZEROS: f64 = 16.0;
    let half = (ZEROS / (2.0 * cutoff)).ceil() as i64;
    let taps = (2 * half + 1) as usize;
    let sinc = |x: f64| if x == 0.0 { 1.0 } else { (PI * x).sin() / (PI * x) };
    // Output sample i sits at input position i * down / up = base + phase / up; phase in 0..up.
    let table: Vec<f32> = (0..up)
        .flat_map(|phase| {
            let frac = phase as f64 / up as f64;
            (0..taps).map(move |k| {
                let d = (k as i64 - half) as f64 - frac;
                let window = if d.abs() < half as f64 { 0.5 + 0.5 * (PI * d / half as f64).cos() } else { 0.0 };
                (2.0 * cutoff * sinc(2.0 * cutoff * d) * window) as f32
            })
        })
        .collect();
    let out_len = (samples.len() as u64 * up).div_ceil(down) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as u64 * down;
            let (base, phase) = ((pos / up) as i64, (pos % up) as usize);
            let coefs = &table[phase * taps..(phase + 1) * taps];
            let mut sum = 0f32;
            for (k, c) in coefs.iter().enumerate() {
                let j = base + k as i64 - half;
                if j >= 0 && (j as usize) < samples.len() {
                    sum += samples[j as usize] * c;
                }
            }
            sum
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dft(x: &[Complex]) -> Vec<Complex> {
        let n = x.len();
        (0..n)
            .map(|k| {
                x.iter().enumerate().fold(Complex::default(), |acc, (j, v)| {
                    let a = -2.0 * PI * (j * k % n) as f64 / n as f64;
                    let t = v.mul(Complex { re: a.cos(), im: a.sin() });
                    Complex { re: acc.re + t.re, im: acc.im + t.im }
                })
            })
            .collect()
    }

    #[test]
    fn the_fft_is_the_dft_for_mixed_and_prime_lengths() {
        for n in [1, 2, 7, 12, 25, 400, 22] {
            let x: Vec<Complex> = (0..n).map(|i| Complex { re: ((i * 7 + 3) % 11) as f64 - 5.0, im: ((i * 5) % 3) as f64 }).collect();
            let mut out = vec![Complex::default(); n];
            fft(&x, 1, &twiddles(n), &mut out);
            let max = dft(&x).iter().zip(&out).map(|(a, b)| (a.re - b.re).abs().max((a.im - b.im).abs())).fold(0.0, f64::max);
            assert!(max < 1e-9, "n {n}: {max}");
        }
    }

    /// Values of Hugging Face's `mel_filter_bank(201, 80, 0, 8000, 16000, "slaney", "slaney")` (librosa's
    /// `filters.mel(sr=16000, n_fft=400, n_mels=80)`, OpenAI's `mel_filters.npz`), printed with transformers 4.57.6.
    #[test]
    fn the_mel_filters_are_the_references() {
        let w = mel_filters(80);
        let expected = [
            ((0, 0), 0.0),
            ((0, 1), 0.024862593984),
            ((1, 1), 0.001990821888),
            ((1, 2), 0.022871772096),
            ((10, 11), 0.004954375103),
            ((10, 12), 0.0),
            ((40, 42), 0.005411105099),
            ((40, 43), 0.014735565741),
            ((40, 44), 0.006518189695),
            ((79, 186), 0.000366741680),
            ((79, 196), 0.001795036110),
            ((79, 199), 0.000448759028),
            ((79, 200), 0.0),
        ];
        for ((m, b), value) in expected {
            assert!((w[m * BINS + b] - value).abs() < 1e-11, "filter {m} bin {b}: {} vs {value}", w[m * BINS + b]);
        }
        assert_eq!(w.iter().filter(|&&x| x > 0.0).count(), 391);
    }

    #[test]
    fn silence_gives_the_floor_everywhere() {
        let mel = LogMel::new(80).compute(&[0.0; 16_000]);
        assert_eq!(mel.len(), 80 * FRAMES);
        assert!(mel.iter().all(|&x| x == ((-10.0f64 + 4.0) / 4.0) as f32));
    }

    #[test]
    fn a_tone_lands_in_its_mel_band() {
        let tone: Vec<f32> = (0..16_000).map(|i| (2.0 * PI * 1000.0 * i as f64 / 16_000.0).sin() as f32 * 0.5).collect();
        let mel = LogMel::new(80).compute(&tone);
        let frame = 50;
        let band = (0..80).max_by(|&a, &b| mel[a * FRAMES + frame].total_cmp(&mel[b * FRAMES + frame])).unwrap();
        let centre = |m: usize| mel_to_hz(hz_to_mel(8000.0) * (m + 1) as f64 / 81.0);
        assert!((centre(band) - 1000.0).abs() < 60.0, "band {band} centred at {} Hz", centre(band));
        let (min, max) = mel.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
        assert_eq!(mel[band * FRAMES + 200], min, "after the tone: the floor");
        assert!((max - min - 2.0).abs() < 1e-6, "the floor is 8 below the maximum, scaled by 1/4");
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn sine(rate: u32, hz: f64, seconds: f64) -> Vec<f32> {
        (0..(rate as f64 * seconds) as usize).map(|i| (2.0 * PI * hz * i as f64 / rate as f64).sin() as f32).collect()
    }

    #[test]
    fn resampling_keeps_speech_tones_and_removes_what_would_fold() {
        for rate in [44_100, 48_000, 22_050, 8_000, 16_000] {
            let out = resample(&sine(rate, 1000.0, 1.0), rate);
            assert_eq!(out.len(), 16_000, "{rate}");
            let mid = &out[2000..14_000];
            // The 1 kHz tone comes out as a 1 kHz tone at 16 kHz with its level (RMS of a sine: 0.707).
            let expected = sine(RATE, 1000.0, 1.0);
            let lag = (0..16).min_by(|&a, &b| {
                let err = |l: usize| mid.iter().zip(&expected[2000 - l..]).map(|(x, y)| (x - y).abs()).sum::<f32>();
                err(a).total_cmp(&err(b))
            });
            assert_eq!(lag, Some(0), "{rate}: no delay");
            assert!((rms(mid) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "{rate}: rms {}", rms(mid));
            assert!(mid.iter().zip(&expected[2000..]).all(|(x, y)| (x - y).abs() < 0.01), "{rate}");
        }
        // 12 kHz at 44.1 kHz would fold to 4 kHz: it must be gone (below -40 dB).
        let folded = resample(&sine(44_100, 12_000.0, 1.0), 44_100);
        assert!(rms(&folded[2000..14_000]) < 0.01 * std::f32::consts::FRAC_1_SQRT_2, "rms {}", rms(&folded[2000..14_000]));
        assert_eq!(resample(&[], 44_100), Vec::<f32>::new());
    }
}
