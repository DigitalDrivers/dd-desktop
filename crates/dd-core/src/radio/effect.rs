//! The radio sound of team radio: a dry speech clip in, a pit-to-car transmission out. A port of the ffmpeg chain
//! Joe approved by ear on 2026-11-09 (research 2026-11-09-boxenfunk, `samples/make_samples.py::radio_filter`):
//! the voice is band-limited (300-3,200 Hz), lifted around 1.8 kHz, compressed 8:1, soft-clipped and band-limited
//! again; a key-up click, a talk-permit beep and a pink noise bed open the channel, a two-tone roger beep and a
//! squelch tail close it. The parameters live in `effect.json`, which the web's preview reads too. Filters are
//! computed from the input's sample rate (22,050 to 48,000 Hz), and the noise comes from seeded generators, so the
//! same clip always gives the same samples. No audio device here: the shell plays what [`transmit`] returns.

use serde::{Deserialize, Serialize};

/// ffmpeg's default for `highpass` and `lowpass` (two poles, `width_type=q`, `width=0.707`): Butterworth
const BUTTERWORTH_Q: f64 = 0.707;
/// A prepared clip keeps its peaks below this, so it can be stored as a file without clipping.
const MAX_PEAK: f32 = 0.99;
/// Seeds of the three noise sources; any constant works, distinct ones keep the sources independent.
const SEED_BED: u32 = 0x5EED_0001;
const SEED_CLICK: u32 = 0x5EED_0002;
const SEED_TAIL: u32 = 0x5EED_0003;
/// Where the voice's high-passes go as the strength falls to 0
const OPEN_LOW_HZ: f32 = 20.0;

/// Parameters of the effect, from `effect.json`. Levels are in dB (`*Db` relative, `*Dbfs` absolute), frequencies
/// in Hz, times in milliseconds. The noise levels are the RMS of the band-limited noise, measured from the
/// reference's generators with ffmpeg's `volumedetect` (`anoisesrc ... ,highpass,lowpass,volume`), so they do not
/// depend on how a generator scales its noise: the bed's -61 dBFS is the reference's pink noise at amplitude 0.5
/// and -34 dB, the click's -14.5 dBFS its white noise at 0.6, the tail's -30.3 dBFS its white noise at 0.5 and
/// -14 dB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub prepare: Prepare,
    pub voice: Voice,
    pub noise_bed: NoiseBed,
    pub click: Click,
    pub talk_permit_beep: TalkPermitBeep,
    pub roger_beep: RogerBeep,
    pub squelch_tail: SquelchTail,
    pub limiter: Limiter,
    pub engineer: EngineerTiming,
    pub spotter: SpotterTiming,
    pub clean: Clean,
}

/// How [`prepare`] trims and levels a dry clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepare {
    /// A window quieter than this (RMS) is silence; the reference's `silenceremove` uses -45 dB.
    pub silence_threshold_db: f32,
    /// Length of the RMS window, and of the blocks the loudness is measured in
    pub silence_window_ms: f32,
    /// Silence added after the trimmed voice (the reference's `apad=pad_dur=0.03`)
    pub pad_ms: f32,
    /// Loudness of a prepared clip (RMS of its 20 ms blocks, pauses gated out). -24 dBFS is what the Supertonic clips
    /// Joe approved through the reference have (fixture: -23.9), so the compressor works on them the same way.
    pub loudness_dbfs: f32,
}

/// The voice's chain, in the order it runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Voice {
    /// Two second-order high-passes, then two second-order low-passes (fourth order each)
    pub highpass_hz: f32,
    pub lowpass_hz: f32,
    /// Peaking EQ, width in octaves
    pub presence_hz: f32,
    pub presence_octaves: f32,
    pub presence_gain_db: f32,
    /// ffmpeg's `acompressor` (RMS detection, soft knee): threshold -20 dB, 8:1, its default knee of 2.83
    /// (9.03 dB), 3 ms attack, 60 ms release, makeup 3 (9.54 dB)
    pub compressor_threshold_db: f32,
    pub compressor_ratio: f32,
    pub compressor_knee_db: f32,
    pub compressor_attack_ms: f32,
    pub compressor_release_ms: f32,
    pub compressor_makeup_db: f32,
    /// `threshold * tanh(x / threshold)`; lower is dirtier
    pub clip_threshold: f32,
    /// One second-order high-pass and low-pass after the clipper
    pub post_highpass_hz: f32,
    pub post_lowpass_hz: f32,
    /// The reference's `volume=2.0` (6.02 dB)
    pub gain_db: f32,
}

/// Pink noise while the channel is open, from the click to the squelch tail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoiseBed {
    pub level_dbfs: f32,
    pub highpass_hz: f32,
    pub lowpass_hz: f32,
    /// Fade in at the click, fade out into the squelch tail
    pub fade_ms: f32,
}

/// The key-up click: a burst of white noise at the very start, fading out over its length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Click {
    pub level_dbfs: f32,
    pub highpass_hz: f32,
    pub lowpass_hz: f32,
    pub length_ms: f32,
}

/// The tone that tells the driver the channel is granted (engineer messages only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TalkPermitBeep {
    pub hz: f32,
    pub start_ms: f32,
    pub length_ms: f32,
    pub fade_ms: f32,
    pub peak_dbfs: f32,
}

/// The two-tone end beep, `gap_ms` after the voice (engineer messages only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RogerBeep {
    pub first_hz: f32,
    pub second_hz: f32,
    /// Length of each tone
    pub tone_ms: f32,
    pub fade_ms: f32,
    pub peak_dbfs: f32,
    pub gap_ms: f32,
}

/// The burst of white noise when the carrier drops, decaying by `decay_db` over its length; the transmission
/// ends with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquelchTail {
    pub level_dbfs: f32,
    pub highpass_hz: f32,
    pub lowpass_hz: f32,
    pub length_ms: f32,
    pub decay_db: f32,
}

/// A look-ahead peak limiter on the whole transmission: no sample leaves above `limit`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limiter {
    pub limit: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
}

/// Where the voice starts in a full transmission; the talk-permit beep comes before it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineerTiming {
    pub voice_start_ms: f32,
}

/// The urgent call: the voice right after the click, the squelch tail right after the voice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotterTiming {
    pub voice_start_ms: f32,
    pub tail_gap_ms: f32,
}

/// What is left at strength 0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clean {
    /// Gain of the clean voice: a prepared clip at -24 dBFS then sounds about as loud as the compressed radio voice.
    pub voice_gain_db: f32,
    /// Both beeps at strength 0, relative to their level at full strength, so the driver still hears the radio
    /// open and close.
    pub beep_gain_db: f32,
}

/// The kind of transmission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Full transmission: click, talk-permit beep, voice from 160 ms, roger beep, squelch tail
    Engineer,
    /// Urgent: only the key-up click before the voice (no beep), voice from 20 ms, no roger beep, squelch tail
    /// 20 ms after the voice
    Spotter,
}

/// The built-in effect.json.
pub fn params() -> Params {
    serde_json::from_str(include_str!("effect.json")).expect("effect.json matches Params")
}

/// A dry clip made ready to keep: leading and trailing silence trimmed (an RMS window below -45 dB, like the
/// reference's `silenceremove`), 30 ms of silence after it, and its loudness (the RMS of its 20 ms blocks, pauses
/// gated out) set to a fixed level, so every phrase and name is equally loud. Only a clip with an extreme crest
/// factor (peak more than 24 dB above that level) comes out quieter: its peak stays below 0.99. An all-silent or
/// empty clip gives an empty clip.
pub fn prepare(dry: &[f32], rate: u32) -> Vec<f32> {
    let p = params().prepare;
    if dry.is_empty() {
        return Vec::new();
    }
    let window = samples(p.silence_window_ms, rate).clamp(1, dry.len());
    let silence = (db(p.silence_threshold_db) as f64).powi(2);
    let mut sums = Vec::with_capacity(dry.len() + 1);
    let mut sum = 0.0f64;
    sums.push(sum);
    for x in dry {
        sum += (*x as f64).powi(2);
        sums.push(sum);
    }
    // the window that starts at sample i is louder than the threshold
    let loud = |i: &usize| (sums[i + window] - sums[*i]) / window as f64 > silence;
    let starts = 0..=dry.len() - window;
    let Some(first) = starts.clone().find(loud) else {
        return Vec::new();
    };
    let last = starts.rev().find(loud).unwrap_or(first);
    // from the first to the last sample above the threshold inside those windows
    let above = |x: &f32| (*x as f64).powi(2) > silence;
    let start = first + dry[first..first + window].iter().position(above).unwrap_or(0);
    let end = last + dry[last..last + window].iter().rposition(above).map_or(window, |i| i + 1);
    let mut clip = dry[start..end].to_vec();

    let rms = gated_rms(&clip, window);
    let peak = clip.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let gain = (db(p.loudness_dbfs) / rms).min(MAX_PEAK / peak);
    for x in &mut clip {
        *x *= gain;
    }
    clip.resize(clip.len() + samples(p.pad_ms, rate), 0.0);
    clip
}

/// The RMS of the blocks louder than a tenth of the mean power (EBU R128's relative gate, -10 dB): pauses between
/// words do not count, and the result scales exactly with the clip's level.
fn gated_rms(x: &[f32], block: usize) -> f32 {
    let powers: Vec<f64> = x.chunks(block).map(|b| b.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / b.len() as f64).collect();
    let gate = powers.iter().sum::<f64>() / powers.len() as f64 / 10.0;
    let loud: Vec<f64> = powers.into_iter().filter(|p| *p >= gate).collect();
    (loud.iter().sum::<f64>() / loud.len() as f64).sqrt() as f32
}

/// Several prepared clips as one utterance (a name followed by a phrase), `gap_ms` of silence between them. An
/// empty clip (a name that came out silent) is left out, so it leaves no double gap.
pub fn join(clips: &[&[f32]], rate: u32, gap_ms: u32) -> Vec<f32> {
    let gap = samples(gap_ms as f32, rate);
    let mut out = Vec::new();
    for clip in clips.iter().filter(|c| !c.is_empty()) {
        if !out.is_empty() {
            out.resize(out.len() + gap, 0.0);
        }
        out.extend_from_slice(clip);
    }
    out
}

/// The transmission of a dry (prepared) clip, at the clip's rate, peak-limited at `limiter.limit` (0.89).
///
/// `strength` 1.0 is the full effect of the reference, 0.0 the clean voice: no band limit, no compression or
/// clipping, no click, noise bed or squelch tail, the voice at `clean.voiceGainDb`, and (engineer messages) both
/// beeps 12 dB quieter, so the driver still hears the radio open and close. Between, the voice's chain opens up
/// gradually (see `radio_voice`: band edges on a log scale, ratio, presence, makeup and gain towards neutral, the
/// clipper mixed in), click, noise bed and tail scale in amplitude with `strength`, and the beeps' gain moves
/// linearly in dB. The timing is that of the shape at every strength. An empty clip gives no transmission (an empty
/// vector).
pub fn transmit(dry: &[f32], rate: u32, params: &Params, strength: f32, shape: Shape) -> Vec<f32> {
    if dry.is_empty() {
        return Vec::new();
    }
    let p = params;
    let s = strength.clamp(0.0, 1.0);
    let ms = |v: f32| samples(v, rate);
    let tone_len = ms(p.roger_beep.tone_ms);
    let voice_at = match shape {
        Shape::Engineer => ms(p.engineer.voice_start_ms),
        Shape::Spotter => ms(p.spotter.voice_start_ms),
    };
    let voice_end = voice_at + dry.len();
    let roger_at = voice_end + ms(p.roger_beep.gap_ms);
    let tail_at = match shape {
        Shape::Engineer => roger_at + 2 * tone_len,
        Shape::Spotter => voice_end + ms(p.spotter.tail_gap_ms),
    };
    let mut out = vec![0.0f32; tail_at + ms(p.squelch_tail.length_ms)];

    if s > 0.0 {
        add(&mut out, voice_at, &radio_voice(dry, rate, &p.voice, s, p.clean.voice_gain_db), 1.0);
    } else {
        add(&mut out, voice_at, dry, db(p.clean.voice_gain_db));
    }
    if s > 0.0 {
        let b = &p.noise_bed;
        let mut bed = band_noise(tail_at, rate, SEED_BED, true, b.highpass_hz, b.lowpass_hz, b.level_dbfs);
        fade(&mut bed, ms(b.fade_ms), ms(b.fade_ms));
        add(&mut out, 0, &bed, s);

        let c = &p.click;
        let mut click = band_noise(ms(c.length_ms), rate, SEED_CLICK, false, c.highpass_hz, c.lowpass_hz, c.level_dbfs);
        let n = click.len();
        fade(&mut click, 0, n);
        add(&mut out, 0, &click, s);

        let t = &p.squelch_tail;
        let mut tail = band_noise(ms(t.length_ms), rate, SEED_TAIL, false, t.highpass_hz, t.lowpass_hz, t.level_dbfs);
        // ffmpeg's `afade=t=out:curve=exp`: linear in dB, from 0 down to -decay_db at the end
        let (step, mut gain) = (db(-t.decay_db / tail.len() as f32), 1.0f32);
        for x in &mut tail {
            *x *= gain;
            gain *= step;
        }
        add(&mut out, tail_at, &tail, s);
    }
    if shape == Shape::Engineer {
        let gain = db(p.clean.beep_gain_db * (1.0 - s));
        let b = &p.talk_permit_beep;
        let beep = tone(ms(b.length_ms), rate, b.hz, db(b.peak_dbfs), ms(b.fade_ms));
        add(&mut out, ms(b.start_ms), &beep, gain);
        let r = &p.roger_beep;
        let peak = db(r.peak_dbfs);
        add(&mut out, roger_at, &tone(tone_len, rate, r.first_hz, peak, ms(r.fade_ms)), gain);
        add(&mut out, roger_at + tone_len, &tone(tone_len, rate, r.second_hz, peak, ms(r.fade_ms)), gain);
    }
    limit(&mut out, rate, &p.limiter);
    out
}

/// The voice through the radio: band limit, presence lift, compressor, soft clipper, band limit, gain. At strength
/// 1 the reference's chain; below it the same chain opened up, so there is only one path and nothing cancels (a mix
/// of the filtered and the dry voice lost up to 6 dB around strength 0.5, where the filters turn the phase): the
/// band's edges move out on a log scale towards 20 Hz and 0.45 times the rate, presence, makeup and ratio towards
/// neutral, the clipper's share falls with the strength, and the gain moves in dB towards the clean voice's.
fn radio_voice(dry: &[f32], rate: u32, v: &Voice, strength: f32, clean_gain_db: f32) -> Vec<f32> {
    let s = strength;
    let low = |hz: f32| OPEN_LOW_HZ * (hz / OPEN_LOW_HZ).powf(s);
    let open_high = 0.45 * rate as f32;
    let high = |hz: f32| open_high * (hz / open_high).powf(s);
    let hp = Biquad::highpass(rate, low(v.highpass_hz));
    let lp = Biquad::lowpass(rate, high(v.lowpass_hz));
    let presence = Biquad::peaking(rate, v.presence_hz, v.presence_octaves, v.presence_gain_db * s);
    let mut pre = [hp, hp, lp, lp, presence];
    let mut compressor = Compressor::new(rate, v, s);
    let (t, s) = (v.clip_threshold as f64, s as f64);
    let mut post = [Biquad::highpass(rate, low(v.post_highpass_hz)), Biquad::lowpass(rate, high(v.post_lowpass_hz))];
    let gain = db(clean_gain_db + (v.gain_db - clean_gain_db) * strength) as f64;
    // one loop for the whole chain, so the processor overlaps the filters' recursions (five filters in one loop
    // took 1.0 ms for 3 s at 48 kHz, one pass per filter 2.5 ms)
    dry.iter()
        .map(|x| {
            let mut x = *x as f64;
            for f in &mut pre {
                x = f.run(x);
            }
            x = compressor.run(x);
            x = (1.0 - s) * x + s * t * tanh(x / t);
            for f in &mut post {
                x = f.run(x);
            }
            (x * gain) as f32
        })
        .collect()
}

/// ffmpeg's `acompressor` with its defaults (RMS detection, soft knee by Hermite interpolation in the log domain),
/// ported from libavfilter/af_sidechaincompress.c so the voice is squeezed the same way. Below strength 1 the
/// ratio moves towards 1:1 and the makeup towards 0 dB.
struct Compressor {
    thres: f64,
    ratio: f64,
    knee: f64,
    knee_start: f64,
    knee_stop: f64,
    compressed_knee_stop: f64,
    /// Mean power above which the compressor works (the knee's start)
    detect: f64,
    attack: f64,
    release: f64,
    makeup: f64,
    power: f64,
}

impl Compressor {
    fn new(rate: u32, v: &Voice, strength: f32) -> Self {
        let threshold = db(v.compressor_threshold_db) as f64;
        let knee = db(v.compressor_knee_db) as f64;
        let ratio = 1.0 + (v.compressor_ratio as f64 - 1.0) * strength as f64;
        let thres = threshold.ln();
        let knee_stop = (threshold * knee.sqrt()).ln();
        Compressor {
            thres,
            ratio,
            knee,
            knee_start: (threshold / knee.sqrt()).ln(),
            knee_stop,
            compressed_knee_stop: (knee_stop - thres) / ratio + thres,
            detect: (threshold / knee.sqrt()).powi(2),
            attack: (4000.0 / (v.compressor_attack_ms as f64 * rate as f64)).min(1.0),
            release: (4000.0 / (v.compressor_release_ms as f64 * rate as f64)).min(1.0),
            makeup: db(v.compressor_makeup_db * strength) as f64,
            power: 0.0,
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let p = x * x;
        self.power += (p - self.power) * if p > self.power { self.attack } else { self.release };
        let mut gain = 1.0;
        if self.power > self.detect {
            // ln and exp in f32 are cheaper and precise enough for a gain
            let level = 0.5 * (self.power as f32).ln() as f64;
            let out = if self.knee > 1.0 && level < self.knee_stop {
                let (start, stop) = (self.knee_start, self.knee_stop);
                hermite(level, start, stop, start, self.compressed_knee_stop, 1.0, 1.0 / self.ratio)
            } else {
                (level - self.thres) / self.ratio + self.thres
            };
            gain = ((out - level) as f32).exp() as f64;
        }
        x * gain * self.makeup
    }
}

/// tanh by its Padé approximant of order [7/6]: within 1e-6 of the true value up to |x| = 3 (the clipper's input
/// rarely goes beyond 2.5), within 1e-4 (-80 dB) up to 4.97, and ±1 beyond. Cheaper than the library's (with it
/// and f64 ln/exp in the compressor the voice took 3.5 ms for 3 s at 48 kHz, with both changes 2.2 ms).
fn tanh(x: f64) -> f64 {
    if x.abs() >= 4.97 {
        return x.signum();
    }
    let x2 = x * x;
    x * (135135.0 + x2 * (17325.0 + x2 * (378.0 + x2))) / (135135.0 + x2 * (62370.0 + x2 * (3150.0 + 28.0 * x2)))
}

fn hermite(x: f64, x0: f64, x1: f64, p0: f64, p1: f64, m0: f64, m1: f64) -> f64 {
    let width = x1 - x0;
    let t = (x - x0) / width;
    let (m0, m1) = (m0 * width, m1 * width);
    let c2 = -3.0 * p0 - 2.0 * m0 + 3.0 * p1 - m1;
    let c3 = 2.0 * p0 + m0 - 2.0 * p1 + m1;
    ((c3 * t + c2) * t + m0) * t + p0
}

/// Look-ahead peak limiter on a whole buffer: the gain each sample needs, lowered ahead of a peak (backwards, with
/// the attack time) and recovering after it (forwards, with the release time). It never lets a sample above the
/// limit. Unlike ffmpeg's `alimiter` (whose default `level=1` scales the result back up to 0 dBFS) it leaves the
/// level at the limit.
fn limit(x: &mut [f32], rate: u32, l: &Limiter) {
    let coeff = |ms: f32| 1.0 - (-1000.0 / (ms * rate as f32)).exp();
    let (attack, release) = (coeff(l.attack_ms), coeff(l.release_ms));
    // kept as gain reduction (1 - gain), which recovers by one multiplication per sample
    let mut cuts: Vec<f32> = x.iter().map(|s| 1.0 - (l.limit / s.abs()).min(1.0)).collect();
    let mut cut = 0.0f32;
    for need in cuts.iter_mut().rev() {
        cut = (cut * (1.0 - attack)).max(*need);
        *need = cut;
    }
    let mut cut = 0.0f32;
    for (s, need) in x.iter_mut().zip(&cuts) {
        cut = (cut * (1.0 - release)).max(*need);
        *s *= 1.0 - cut;
    }
}

/// Noise of `len` samples, band-limited by one second-order high-pass and low-pass, at `level_dbfs` RMS.
fn band_noise(len: usize, rate: u32, seed: u32, pink: bool, highpass: f32, lowpass: f32, level_dbfs: f32) -> Vec<f32> {
    let mut random = Xorshift(seed);
    let mut state = [0.0f64; 7];
    let (mut hp, mut lp) = (Biquad::highpass(rate, highpass), Biquad::lowpass(rate, lowpass));
    let mut x: Vec<f32> = (0..len)
        .map(|_| {
            let white = random.next();
            let noise = if pink { pink_filter(white, &mut state) } else { white };
            lp.run(hp.run(noise)) as f32
        })
        .collect();
    let rms = (x.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / len.max(1) as f64).sqrt() as f32;
    if rms > 0.0 {
        let gain = db(level_dbfs) / rms;
        for s in &mut x {
            *s *= gain;
        }
    }
    x
}

/// Paul Kellet's pink noise filter, the one ffmpeg's `anoisesrc=color=pink` uses.
fn pink_filter(white: f64, b: &mut [f64; 7]) -> f64 {
    b[0] = 0.99886 * b[0] + white * 0.0555179;
    b[1] = 0.99332 * b[1] + white * 0.0750759;
    b[2] = 0.96900 * b[2] + white * 0.1538520;
    b[3] = 0.86650 * b[3] + white * 0.3104856;
    b[4] = 0.55000 * b[4] + white * 0.5329522;
    b[5] = -0.7616 * b[5] - white * 0.0168980;
    let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + white * 0.5362;
    b[6] = white * 0.115926;
    pink * 0.11
}

/// A small deterministic generator (xorshift32), so the noise is the same on every run.
struct Xorshift(u32);

impl Xorshift {
    /// Uniform in [-1, 1]
    fn next(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x as f64 * (2.0 / u32::MAX as f64) - 1.0
    }
}

/// A sine of `len` samples at `peak`, with linear fades in and out of `fade_len` samples (ffmpeg's `afade`).
fn tone(len: usize, rate: u32, hz: f32, peak: f32, fade_len: usize) -> Vec<f32> {
    let w = std::f64::consts::TAU * hz as f64 / rate as f64;
    let mut x: Vec<f32> = (0..len).map(|i| peak * (w * i as f64).sin() as f32).collect();
    fade(&mut x, fade_len, fade_len);
    x
}

/// Linear fade in over the first `fade_in` samples and out over the last `fade_out`.
fn fade(x: &mut [f32], fade_in: usize, fade_out: usize) {
    let n = x.len();
    for i in 0..fade_in.min(n) {
        x[i] *= i as f32 / fade_in as f32;
    }
    for i in 0..fade_out.min(n) {
        x[n - 1 - i] *= i as f32 / fade_out as f32;
    }
}

/// Mixes `part` times `gain` into `out` from sample `at` on.
fn add(out: &mut [f32], at: usize, part: &[f32], gain: f32) {
    for (o, p) in out[at..].iter_mut().zip(part) {
        *o += p * gain;
    }
}

fn db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn samples(ms: f32, rate: u32) -> usize {
    (ms as f64 * rate as f64 / 1000.0).round() as usize
}

/// A second-order filter after the Audio EQ Cookbook (R. Bristow-Johnson), the formulas ffmpeg's biquads use;
/// transposed direct form II with f64 state.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Biquad { b0: b[0] / a[0], b1: b[1] / a[0], b2: b[2] / a[0], a1: a[1] / a[0], a2: a[2] / a[0], z1: 0.0, z2: 0.0 }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// The angular frequency; kept below Nyquist so a low rate cannot make the filter unstable.
    fn omega(rate: u32, hz: f32) -> f64 {
        std::f64::consts::TAU * (hz as f64).min(0.45 * rate as f64) / rate as f64
    }

    fn lowpass(rate: u32, hz: f32) -> Self {
        let (sin, cos) = Self::omega(rate, hz).sin_cos();
        let alpha = sin / (2.0 * BUTTERWORTH_Q);
        Self::new([(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0], [1.0 + alpha, -2.0 * cos, 1.0 - alpha])
    }

    fn highpass(rate: u32, hz: f32) -> Self {
        let (sin, cos) = Self::omega(rate, hz).sin_cos();
        let alpha = sin / (2.0 * BUTTERWORTH_Q);
        Self::new([(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0], [1.0 + alpha, -2.0 * cos, 1.0 - alpha])
    }

    /// Peaking EQ with its width in octaves (ffmpeg's `equalizer=t=o`)
    fn peaking(rate: u32, hz: f32, octaves: f32, gain_db: f32) -> Self {
        let w = Self::omega(rate, hz);
        let (sin, cos) = w.sin_cos();
        let alpha = sin * (std::f64::consts::LN_2 / 2.0 * octaves as f64 * w / sin).sinh();
        let a = 10f64.powf(gain_db as f64 / 40.0);
        Self::new([1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a], [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a])
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Joe, Max ist links neben dir." by Supertonic 3 voice M1 (samples-open), mono f32 at 22,050 Hz, trimmed like
    /// the reference. Made with:
    /// `ffmpeg -i samples-open/supertonic3-m1-de-04-joe-max-links.dry.mp3 -ac 1 -af "aresample=22050,
    /// silenceremove=start_periods=1:start_threshold=-45dB,areverse,silenceremove=start_periods=1:start_threshold=-45dB,
    /// areverse,apad=pad_dur=0.03" -f f32le -ar 22050 radio-dry-joe-max-links-22050.f32`
    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/radio-dry-joe-max-links-22050.f32");
    const RATE: u32 = 22_050;

    fn fixture() -> Vec<f32> {
        FIXTURE.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect()
    }

    fn seconds(x: &[f32], rate: u32) -> f32 {
        x.len() as f32 / rate as f32
    }

    fn at(t: f32, rate: u32) -> usize {
        (t * rate as f32).round() as usize
    }

    fn rms_db(x: &[f32]) -> f32 {
        let p = x.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64;
        (10.0 * p.max(1e-20).log10()) as f32
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// Energy below 200 Hz and above 4 kHz relative to the total, in dB (fourth-order Butterworth splits).
    fn bands_db(x: &[f32], rate: u32) -> (f32, f32) {
        let split = |[mut a, mut b]: [Biquad; 2]| {
            let y: Vec<f32> = x.iter().map(|s| b.run(a.run(*s as f64)) as f32).collect();
            rms_db(&y) - rms_db(x)
        };
        let low = split([Biquad::lowpass(rate, 200.0); 2]);
        let high = split([Biquad::highpass(rate, 4000.0); 2]);
        (low, high)
    }

    /// Amplitude (peak, dBFS) of the sine at `hz` in a window, by Goertzel.
    fn tone_db(x: &[f32], rate: u32, hz: f32) -> f32 {
        let w = std::f64::consts::TAU * hz as f64 / rate as f64;
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for v in x {
            let s = *v as f64 + 2.0 * w.cos() * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let power = s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2;
        (20.0 * (2.0 * power.sqrt() / x.len() as f64).max(1e-10).log10()) as f32
    }

    /// First and last 5 ms step (in ms) whose 10 ms window carries the tone at `hz` within 6 dB of its strongest
    /// window, searched in `from..to` seconds.
    fn tone_span_ms(x: &[f32], rate: u32, hz: f32, from: f32, to: f32) -> (f32, f32) {
        let (step, len) = (at(0.005, rate), at(0.010, rate));
        let windows: Vec<(usize, f32)> = (at(from, rate)..at(to, rate).min(x.len() - len))
            .step_by(step)
            .map(|i| (i, tone_db(&x[i..i + len], rate, hz)))
            .collect();
        let best = windows.iter().fold(f32::MIN, |m, w| m.max(w.1));
        let on: Vec<usize> = windows.iter().filter(|w| w.1 > best - 6.0).map(|w| w.0).collect();
        let ms = |i: usize| i as f32 * 1000.0 / rate as f32;
        (ms(on[0]), ms(on[on.len() - 1] + len))
    }

    /// What a listener hears of an engineer transmission of a clip of `d` seconds.
    #[derive(Debug)]
    struct Heard {
        length_s: f32,
        mean_db: f32,
        peak: f32,
        low_db: f32,
        high_db: f32,
        /// Strongest 10 ms window of 1,250 Hz in the first 160 ms, and where it is on (ms)
        beep_db: f32,
        beep_ms: (f32, f32),
        /// The roger beep's tones in their 50 ms each, and the 1,000 Hz tone's span from the voice's end (ms)
        roger_first_db: f32,
        roger_second_db: f32,
        roger_ms: (f32, f32),
        /// Noise bed in the gap between voice and roger beep (5-75 ms after the voice)
        bed_db: f32,
        /// Squelch tail: its first 30 ms, and the last 10 ms of the transmission
        tail_db: f32,
        end_db: f32,
    }

    /// The windows follow the talk-permit beep's onset: ffmpeg's `alimiter` delays the reference by its 5 ms
    /// look-ahead (and `atrim` cuts the last 5 ms of the tail).
    fn heard(x: &[f32], rate: u32, d: f32) -> Heard {
        let beep_ms = tone_span_ms(x, rate, 1250.0, 0.0, 0.16);
        let delay = beep_ms.0 / 1000.0 - 0.03;
        let w = |a: f32, b: f32| &x[at(a + delay, rate)..at(b + delay, rate).min(x.len())];
        let voice_end = 0.16 + d;
        let (low_db, high_db) = bands_db(x, rate);
        let r = voice_end + 0.08;
        let roger = tone_span_ms(x, rate, 1000.0, voice_end + delay, r + 0.1 + delay);
        Heard {
            length_s: seconds(x, rate),
            mean_db: rms_db(x),
            peak: peak(x),
            low_db,
            high_db,
            beep_db: (0..15).map(|i| tone_db(w(i as f32 * 0.01, i as f32 * 0.01 + 0.01), rate, 1250.0)).fold(f32::MIN, f32::max),
            beep_ms,
            roger_first_db: tone_db(w(r + 0.005, r + 0.045), rate, 1000.0),
            roger_second_db: tone_db(w(r + 0.055, r + 0.095), rate, 1500.0),
            roger_ms: (roger.0 - (voice_end + delay) * 1000.0, roger.1 - (voice_end + delay) * 1000.0),
            bed_db: rms_db(w(voice_end + 0.005, voice_end + 0.075)),
            tail_db: rms_db(w(r + 0.1, r + 0.13)),
            end_db: rms_db(&x[x.len() - at(0.01, rate)..]),
        }
    }

    // The reference, measured with `heard` on the ffmpeg render of the fixture (ffmpeg 6.1.1):
    //   python3 research/2026-11-09-testabend/effect/render_ref.py radio-dry-joe-max-links-22050.f32 ref-engineer.f32
    //   (in the workspace), which runs
    //     ffmpeg -f f32le -ar 22050 -ac 1 -i <fixture> -filter_complex <make_samples.radio_filter(1.37868)>
    //     -map "[out]" -ac 1 -f f32le ref-engineer.f32
    //   DD_EFFECT_MEASURE=ref-engineer.f32 cargo test -p dd-core measures_a_render -- --ignored --nocapture
    // ffmpeg's own volumedetect on it: mean -16.1 dB, max 0.0 dB. Its alimiter scales the result back up from 0.89
    // to full scale (`level=1`), so every reference level is 1 dB above ours; we limit at 0.89 and stay there.
    const REF_LENGTH_S: f32 = 1.8590;
    const REF_MEAN_DB: f32 = -16.06;
    const REF_LOW_DB: f32 = -25.70;
    const REF_HIGH_DB: f32 = -29.38;
    const REF_BEEP_DB: f32 = -11.02;
    const REF_ROGER_DB: f32 = -13.04;
    const REF_BED_DB: f32 = -59.77;
    const REF_TAIL_DB: f32 = -36.41;
    /// ffmpeg's `alimiter` level: the reference's levels are this much above ours.
    const REF_AUTO_LEVEL_DB: f32 = 1.01;

    #[test]
    fn effect_json_is_every_parameter_in_camel_case() {
        use serde_json::Value;
        fn same(a: &Value, b: &Value, path: &str) {
            match (a, b) {
                (Value::Object(a), Value::Object(b)) => {
                    assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>(), "{path}");
                    for (key, value) in a {
                        same(value, &b[key], &format!("{path}.{key}"));
                    }
                }
                _ => assert_eq!(a.as_f64().map(|v| v as f32), b.as_f64().map(|v| v as f32), "{path}"),
            }
        }
        let file: Value = serde_json::from_str(include_str!("effect.json")).unwrap();
        // nothing in the file that Params does not read, nothing missing: the web's preview reads the same file
        same(&serde_json::to_value(params()).unwrap(), &file, "");
        assert_eq!(file["talkPermitBeep"]["hz"], 1250.0);
        assert_eq!(file["voice"]["compressorRatio"], 8.0);
    }

    fn close(what: &str, ours: f32, reference: f32, tolerance: f32) {
        assert!((ours - reference).abs() <= tolerance, "{what}: ours {ours:.2}, reference {reference:.2}");
    }

    #[test]
    fn an_engineer_message_sounds_like_the_reference() {
        let dry = fixture();
        let d = seconds(&dry, RATE);
        let ours = heard(&transmit(&dry, RATE, &params(), 1.0, Shape::Engineer), RATE, d);
        close("length d + 0.48 s", ours.length_s, d + 0.48, 0.001);
        close("length vs reference (rounded to ms there)", ours.length_s, REF_LENGTH_S, 0.002);
        close("mean level", ours.mean_db, REF_MEAN_DB, 1.5);
        assert!(ours.peak <= 0.89 + 1e-6, "peak {}", ours.peak);
        close("energy below 200 Hz", ours.low_db, REF_LOW_DB, 1.0);
        close("energy above 4 kHz", ours.high_db, REF_HIGH_DB, 1.0);
        close("talk-permit beep", ours.beep_db + REF_AUTO_LEVEL_DB, REF_BEEP_DB, 0.5);
        close("beep on at 30 ms", ours.beep_ms.0, 30.0, 5.0);
        close("beep off at 100 ms", ours.beep_ms.1, 100.0, 5.0);
        close("roger beep 1,000 Hz", ours.roger_first_db + REF_AUTO_LEVEL_DB, REF_ROGER_DB, 0.5);
        close("roger beep 1,500 Hz", ours.roger_second_db + REF_AUTO_LEVEL_DB, REF_ROGER_DB, 0.5);
        close("roger beep from 80 ms after the voice", ours.roger_ms.0, 80.0, 5.0);
        close("1,000 Hz until 130 ms after the voice", ours.roger_ms.1, 130.0, 5.0);
        close("noise bed while the channel is open", ours.bed_db + REF_AUTO_LEVEL_DB, REF_BED_DB, 1.5);
        close("squelch tail", ours.tail_db + REF_AUTO_LEVEL_DB, REF_TAIL_DB, 1.5);
        assert!(ours.end_db < -90.0, "the tail has died away at the end: {}", ours.end_db);
    }

    #[test]
    fn the_squelch_tail_closes_the_channel_and_lasts_140_ms() {
        let dry = fixture();
        let x = transmit(&dry, RATE, &params(), 1.0, Shape::Engineer);
        let tail_at = at(0.16, RATE) + dry.len() + at(0.08, RATE) + 2 * at(0.05, RATE);
        assert_eq!(x.len(), tail_at + at(0.14, RATE));
        let voice_end = at(0.16, RATE) + dry.len();
        let bed = rms_db(&x[voice_end + at(0.005, RATE)..voice_end + at(0.075, RATE)]);
        let tail = rms_db(&x[tail_at..tail_at + at(0.01, RATE)]);
        assert!(tail > bed + 20.0, "tail {tail} over the faded bed {bed}");
        // decaying by 100 dB over its length, about 7 dB every 10 ms
        let later = rms_db(&x[tail_at + at(0.05, RATE)..tail_at + at(0.06, RATE)]);
        close("tail 50 ms in", later - tail, -36.0, 3.0);
    }

    #[test]
    fn a_spotter_call_starts_at_once_and_has_no_beeps() {
        let dry = fixture();
        let p = params();
        let x = transmit(&dry, RATE, &p, 1.0, Shape::Spotter);
        close("length d + 0.18 s", seconds(&x, RATE), seconds(&dry, RATE) + 0.18, 0.001);
        // the voice is there 40 ms after the key-up click at the latest
        let five = at(0.005, RATE);
        let onset = (at(0.007, RATE)..x.len()).step_by(five).find(|i| rms_db(&x[*i..*i + five]) > -40.0).unwrap();
        assert!(onset as f32 / RATE as f32 <= 0.040, "voice from {} ms", onset as f32 * 1000.0 / RATE as f32);
        // no talk-permit beep (the engineer's is at -12 dBFS), tried with a 220 Hz voice that has nothing at 1,250 Hz
        let hum: Vec<f32> = (0..RATE).map(|i| 0.1 * (std::f32::consts::TAU * 220.0 * i as f32 / RATE as f32).sin()).collect();
        let beep = |shape| tone_db(&transmit(&hum, RATE, &p, 1.0, shape)[at(0.03, RATE)..at(0.10, RATE)], RATE, 1250.0);
        assert!(beep(Shape::Engineer) > -13.0);
        assert!(beep(Shape::Spotter) < -40.0, "{}", beep(Shape::Spotter));
        // and no roger beep after the voice (the engineer's is at -14 dBFS)
        let voice_end = at(0.02, RATE) + dry.len();
        for hz in [1000.0, 1500.0] {
            assert!(tone_db(&x[voice_end..], RATE, hz) < -40.0, "{hz} Hz after the voice");
        }
        // the squelch tail 20 ms after the voice
        let tail_at = voice_end + at(0.02, RATE);
        assert!(rms_db(&x[tail_at..tail_at + at(0.01, RATE)]) > rms_db(&x[voice_end + at(0.005, RATE)..tail_at]) + 20.0);
        assert!(rms_db(&x[x.len() - at(0.01, RATE)..]) < -90.0);
    }

    /// The voice's part of an engineer transmission
    fn voice_part(x: &[f32], d: f32) -> &[f32] {
        &x[at(0.16, RATE)..at(0.16 + d, RATE)]
    }

    #[test]
    fn strength_zero_keeps_the_voice_band_and_adds_no_noise() {
        let dry = prepare(&fixture(), RATE);
        let d = seconds(&dry, RATE);
        let x = transmit(&dry, RATE, &params(), 0.0, Shape::Engineer);
        let (low, high) = bands_db(&dry, RATE);
        let (clean_low, clean_high) = bands_db(voice_part(&x, d), RATE);
        close("below 200 Hz", clean_low, low, 1.0);
        close("above 4 kHz", clean_high, high, 1.0);
        let heard = heard(&x, RATE, d);
        assert!(heard.bed_db < -150.0, "no noise bed: {}", heard.bed_db);
        assert!(rms_db(&x[at(0.10, RATE)..at(0.16, RATE)]) < -150.0, "no noise before the voice");
        assert!(rms_db(&x[..at(0.006, RATE)]) < -150.0, "no click");
        let roger_end = at(0.16, RATE) + dry.len() + at(0.08, RATE) + 2 * at(0.05, RATE);
        assert!(x[roger_end..].iter().all(|s| *s == 0.0), "no squelch tail");
        // the beeps stay, 12 dB quieter
        close("quiet talk-permit beep", heard.beep_db, -24.0, 0.5);
        close("quiet roger beep", heard.roger_first_db, -26.0, 0.5);
        // about as loud as the radio voice
        let radio = transmit(&dry, RATE, &params(), 1.0, Shape::Engineer);
        close("clean voice vs radio voice", rms_db(voice_part(&x, d)), rms_db(voice_part(&radio, d)), 3.0);
    }

    #[test]
    fn strength_half_lies_between_clean_and_full() {
        let dry = prepare(&fixture(), RATE);
        let d = seconds(&dry, RATE);
        let [clean, half, full] = [0.0, 0.5, 1.0].map(|s| transmit(&dry, RATE, &params(), s, Shape::Engineer));
        let between = |what: &str, f: &dyn Fn(&[f32]) -> f32, margin: f32| {
            let (a, b, c) = (f(&clean), f(&half), f(&full));
            assert!((a.min(c) - margin..=a.max(c) + margin).contains(&b), "{what}: {a} / {b} / {c}");
        };
        // At 0.5 the high-passes are at 77 Hz, so the low band is still about the clean voice's; the half-strength
        // compressor lowers the loud middle band and so lifts the low band's share by up to 1 dB (measured
        // 2026-11-09: -11.7 / -11.0 / -25.2 dB at 0 / 0.5 / 1).
        between("below 200 Hz", &|x| bands_db(voice_part(x, d), RATE).0, 1.5);
        between("above 4 kHz", &|x| bands_db(voice_part(x, d), RATE).1, 0.0);
        between("talk-permit beep", &|x| heard(x, RATE, d).beep_db, 0.0);
        let bed = heard(&half, RATE, d).bed_db;
        close("noise bed at half the amplitude", bed, heard(&full, RATE, d).bed_db - 6.0, 0.5);
        // no dip in loudness on the way (a mix of the radio voice and the clean voice lost 6 dB at 0.5)
        let (a, c) = (rms_db(voice_part(&clean, d)), rms_db(voice_part(&full, d)));
        for s in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let v = rms_db(voice_part(&transmit(&dry, RATE, &params(), s, Shape::Engineer), d));
            assert!(v >= a.min(c) - 1.0 && v <= a.max(c) + 1.0, "voice at strength {s}: {v} (0: {a}, 1: {c})");
        }
    }

    /// The loudness `prepare` sets
    fn loudness_db(x: &[f32]) -> f32 {
        20.0 * gated_rms(x, at(0.02, RATE)).log10()
    }

    #[test]
    fn prepare_trims_the_silence_pads_30_ms_and_sets_the_loudness() {
        let voice = fixture();
        let quiet = |len: usize| -> Vec<f32> {
            let mut noise = Xorshift(7);
            (0..len).map(|_| noise.next() as f32 * 0.0003).collect() // -70 dBFS hiss
        };
        let dry = [quiet(at(0.3, RATE)), voice.clone(), quiet(at(0.5, RATE))].concat();
        let clip = prepare(&dry, RATE);
        // the fixture was trimmed by ffmpeg already, its last 30 ms are padding
        close("length", seconds(&clip, RATE), seconds(&voice, RATE), 0.025);
        assert!(clip[clip.len() - at(0.03, RATE)..].iter().all(|s| *s == 0.0), "30 ms of silence at the end");
        assert!(rms_db(&clip[..at(0.02, RATE)]) > -45.0, "starts with the voice");
        close("loudness", loudness_db(&clip), -24.0, 0.3);
    }

    #[test]
    fn prepare_makes_a_quiet_and_a_loud_clip_equally_loud() {
        let voice = fixture();
        let scaled = |gain: f32| voice.iter().map(|s| s * gain).collect::<Vec<f32>>();
        let (quiet, loud) = (prepare(&scaled(0.3), RATE), prepare(&scaled(3.0), RATE));
        close("quiet vs loud", loudness_db(&quiet), loudness_db(&loud), 0.3);
        close("lengths", seconds(&quiet, RATE), seconds(&loud, RATE), 0.03);
    }

    #[test]
    fn prepare_gives_an_empty_clip_for_silence() {
        assert!(prepare(&[], RATE).is_empty());
        assert!(prepare(&vec![0.0; 10_000], RATE).is_empty());
        let mut noise = Xorshift(3);
        let hiss: Vec<f32> = (0..10_000).map(|_| noise.next() as f32 * 0.001).collect(); // -60 dBFS
        assert!(prepare(&hiss, RATE).is_empty());
        let few = prepare(&[0.5, -0.5, 0.25], RATE);
        assert_eq!(few.len(), 3 + at(0.03, RATE));
        assert!(few.iter().all(|s| s.is_finite() && s.abs() <= 0.99));
    }

    #[test]
    fn join_puts_the_clips_in_order_with_a_gap() {
        let (name, phrase) = (vec![0.1f32; 100], vec![0.2f32; 50]);
        let joined = join(&[&name, &[], &phrase], RATE, 20);
        let gap = at(0.02, RATE);
        assert_eq!(joined.len(), 100 + gap + 50);
        assert!(joined[..100].iter().all(|s| *s == 0.1));
        assert!(joined[100..100 + gap].iter().all(|s| *s == 0.0));
        assert!(joined[100 + gap..].iter().all(|s| *s == 0.2));
        assert_eq!(join(&[&name], RATE, 20), name);
        assert!(join(&[], RATE, 20).is_empty());
    }

    #[test]
    fn odd_clips_give_finite_limited_samples() {
        let p = params();
        let square: Vec<f32> = (0..RATE).map(|i| if i / 50 % 2 == 0 { 1.0 } else { -1.0 }).collect();
        for shape in [Shape::Engineer, Shape::Spotter] {
            assert!(transmit(&[], RATE, &p, 1.0, shape).is_empty(), "no transmission without a voice");
            for strength in [0.0, 0.5, 1.0] {
                for dry in [&[0.5f32, -0.5, 0.25][..], &square] {
                    let x = transmit(dry, RATE, &p, strength, shape);
                    assert!(!x.is_empty());
                    assert!(x.iter().all(|s| s.is_finite() && s.abs() <= 0.89 + 1e-6), "{shape:?} {strength}");
                }
            }
        }
        let dry = fixture();
        assert_eq!(transmit(&dry, RATE, &p, 2.0, Shape::Engineer), transmit(&dry, RATE, &p, 1.0, Shape::Engineer));
        assert_eq!(transmit(&dry, RATE, &p, -1.0, Shape::Engineer), transmit(&dry, RATE, &p, 0.0, Shape::Engineer));
    }

    #[test]
    fn the_fast_tanh_is_the_library_s() {
        for i in -8000..=8000 {
            let x = i as f64 / 1000.0;
            assert!((tanh(x) - x.tanh()).abs() < 1e-4, "{x}");
            if x.abs() <= 3.0 {
                assert!((tanh(x) - x.tanh()).abs() < 1e-6, "{x}");
            }
        }
    }

    #[test]
    fn the_same_clip_gives_the_same_samples() {
        let dry = fixture();
        for shape in [Shape::Engineer, Shape::Spotter] {
            assert_eq!(transmit(&dry, RATE, &params(), 0.7, shape), transmit(&dry, RATE, &params(), 0.7, shape));
        }
    }

    /// A voice-like test signal: a 120 Hz pulse train with harmonics up to 8 kHz, syllables of 150 ms.
    fn buzz(rate: u32, seconds: f32) -> Vec<f32> {
        let n = (seconds * rate as f32) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                let syllable = (std::f32::consts::PI * t / 0.15).sin().abs();
                let harmonics: f32 = (1..=66).map(|k| (std::f32::consts::TAU * 120.0 * k as f32 * t).sin() / k as f32).sum();
                0.05 * syllable * harmonics
            })
            .collect()
    }

    #[test]
    fn sounds_the_same_at_every_rate_from_22050_to_48000() {
        let reference = {
            let dry = buzz(RATE, 1.0);
            heard(&transmit(&dry, RATE, &params(), 1.0, Shape::Engineer), RATE, 1.0)
        };
        for rate in [32_000, 44_100, 48_000] {
            let x = transmit(&buzz(rate, 1.0), rate, &params(), 1.0, Shape::Engineer);
            let at_rate = heard(&x, rate, 1.0);
            close(&format!("{rate}: length"), at_rate.length_s, 1.48, 0.001);
            close(&format!("{rate}: mean"), at_rate.mean_db, reference.mean_db, 0.5);
            close(&format!("{rate}: below 200 Hz"), at_rate.low_db, reference.low_db, 1.0);
            close(&format!("{rate}: beep"), at_rate.beep_db, reference.beep_db, 0.5);
            close(&format!("{rate}: beep on"), at_rate.beep_ms.0, 30.0, 5.0);
            close(&format!("{rate}: noise bed"), at_rate.bed_db, reference.bed_db, 1.5);
            close(&format!("{rate}: tail"), at_rate.tail_db, reference.tail_db, 1.5);
            assert!(at_rate.peak <= 0.89 + 1e-6);
        }
    }

    #[test]
    #[ignore = "timing: cargo test --release -p dd-core transmits_three_seconds -- --ignored --nocapture"]
    fn transmits_three_seconds_in_well_under_ten_milliseconds() {
        let p = params();
        for rate in [22_050, 44_100, 48_000] {
            let dry = buzz(rate, 3.0);
            let runs = 20;
            let start = std::time::Instant::now();
            for _ in 0..runs {
                std::hint::black_box(transmit(std::hint::black_box(&dry), rate, &p, 1.0, Shape::Engineer));
            }
            let each = start.elapsed() / runs;
            println!("{rate} Hz: {:.2} ms for 3 s", each.as_secs_f64() * 1000.0);
            assert!(each.as_secs_f64() < 0.010);
        }
    }

    fn write_wav(path: &std::path::Path, x: &[f32], rate: u32) {
        let mut b = Vec::with_capacity(44 + 2 * x.len());
        let data = 2 * x.len() as u32;
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        for v in [16u32, 1 | 1 << 16, rate, 2 * rate, 2 | 16 << 16] {
            b.extend_from_slice(&v.to_le_bytes()); // PCM, mono, 16 bit
        }
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        for s in x {
            b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    #[ignore = "writes WAVs for listening into DD_EFFECT_OUT"]
    fn writes_the_renders_for_listening() {
        let dir = std::path::PathBuf::from(std::env::var("DD_EFFECT_OUT").expect("DD_EFFECT_OUT"));
        let dry = fixture();
        let p = params();
        for (name, strength, shape) in [
            ("engineer-strength-1.0", 1.0, Shape::Engineer),
            ("spotter-strength-1.0", 1.0, Shape::Spotter),
            ("engineer-strength-0.5", 0.5, Shape::Engineer),
            ("engineer-strength-0.0", 0.0, Shape::Engineer),
        ] {
            let path = dir.join(format!("joe-max-links.rust-{name}.wav"));
            write_wav(&path, &transmit(&dry, RATE, &p, strength, shape), RATE);
        }
    }

    #[test]
    #[ignore = "measures a raw f32 render at 22,050 Hz named by DD_EFFECT_MEASURE"]
    fn measures_a_render() {
        let path = std::env::var("DD_EFFECT_MEASURE").expect("DD_EFFECT_MEASURE");
        let bytes = std::fs::read(path).unwrap();
        let x: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
        println!("{:#?}", heard(&x, RATE, seconds(&fixture(), RATE)));
    }

    #[test]
    #[ignore = "prints what the effect gives on the fixture"]
    fn measures_the_effect() {
        let dry = fixture();
        let d = seconds(&dry, RATE);
        println!("{:#?}", heard(&transmit(&dry, RATE, &params(), 1.0, Shape::Engineer), RATE, d));
    }


}
