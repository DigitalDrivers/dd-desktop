# dd-voice

The team radio's voice: [Supertonic 3](https://huggingface.co/Supertone/supertonic-3) (Supertone Inc.) speaking German
and English on the CPU through ONNX Runtime, from Rust. The words come from `dd_core::radio::phrases`. And its ears:
[Whisper](https://github.com/openai/whisper) base turning the driver's push-to-talk question into text, see
[Listening](#listening).

```rust
let mut engine = Engine::load(&model_dir, 2)?;           // four ONNX sessions, 2 intra-op threads, inter-op 1
let samples = engine.speak("Auto links.", "de", "M1")?;  // mono f32 at Engine::RATE (44,100 Hz)
render_library(&mut engine, "M1", "de", &cache, |done, total| ..)?; // every fixed wording, once
render_names(&mut engine, "M1", "de", "Jörrn", &cache)?;            // the six named spotter calls
let clip = load_clip(&cache, "M1", "de", "Auto links.");            // Option<Vec<f32>>
verify(&model_dir, true)?;                                // sizes, and SHA-256 on request
```

Errors are `VoiceError` with `code()`: `model-missing:<file>`, `model-damaged:<file>`, `unknown-voice`,
`unknown-language`, `empty-text`, `unsupported-text`, `runtime`, `io`.

## Pipeline

A port of the Python SDK (supertonic 1.3.1, MIT): chunks of whole sentences (300 characters), NFKD and the SDK's
text clean-up, `<lang>…</lang>`, duration predictor, text encoder, 8 flow-matching steps of the vector estimator,
vocoder, speed 1.05, 0.3 s of silence between chunks. Differences, on purpose:

- **Deterministic.** The SDK starts from NumPy's unseeded `np.random.randn`, so every call sounds a little
  different. Here the noise is SplitMix64 + Box-Muller seeded with `SEED` at the start of each `speak`.
- Each chunk is cut at its predicted duration (as supertone-inc's Rust example does); the SDK returns the
  vocoder's padded output, up to 70 ms longer.
- Only `de` and `en`.

What came from where: the pipeline, defaults and text clean-up from the SDK's `core.py`, `pipeline.py` and `utils.py`;
the ONNX input and output names and the duration cut from supertone-inc/supertonic `rust/src/helper.rs` (MIT,
Copyright (c) 2025 Supertone Inc.). No code is copied verbatim; neither `ndarray`, `rand`, `regex` nor
`unicode-normalization` from that example are used.

## Fidelity against the SDK

8 sentences (de and en, short and long, umlauts and ß, the respelled name "Jörrn", two sentences in one call), 6
different voices. Windows, release build, SDK 1.3.1 with onnxruntime 1.31.1 from the venv
(`research/2026-11-09-testabend/voice-rust/fidelity.py`):

- **Same starting noise fed into the SDK:** identical length to the sample, SNR 89-100 dB, correlation 1.0000.
  The port matches the SDK; the only differences are floating-point ones between the two ONNX Runtime builds.
- **The SDK as it is (its own random noise):** identical lengths (the duration predictor does not depend on the
  noise). The band-energy envelope correlation Rust vs SDK is 0.81-0.91, two SDK runs of the same sentence
  against each other 0.76-0.94: the same spread.

## Numbers in the radio

Supertonic reads digits itself, but not the way the radio needs them. Duration predictor, M1 and F1:

| Text | Seconds | Spelled out | Seconds |
|---|---|---|---|
| Neue Bestzeit, 8:12,4. | 3.03 | Neue Bestzeit, acht zwölf vier. | 2.50 |
| New personal best, 8:12.4. | 3.20 | New personal best, eight twelve four. | 2.66 |
| Sprit für noch 21 Runden. | 2.46 | Sprit für noch einundzwanzig Runden. | 2.71 |
| Sprit für noch 4 Runden. | 2.27 | Sprit für noch vier Runden. | 2.12 |

A lap time comes out 20 % longer than the race-engineer form (it says more words), "21" shorter than
"einundzwanzig". So `phrases` spells every number out (`fuel_laps`, `lap_time`, `personal_best`), and the phrase
file has no digits (a test checks that).

## Measured (Windows 11, i7-11700, power plan Balanced, AC EVO not running, Blender open)

`cargo run --release -p dd-voice --example bench -- <model folder> <threads>`. Medians of two passes over 9 short
calls (2-5 words) and 3 coach answers (17, 21, 23 words), voice M1; two warm runs per thread setting.

| | 2 threads | 4 threads |
|---|---|---|
| Model load | 0.82-0.91 s | 0.81-0.82 s |
| Short call | 0.40-0.41 s (p90 0.43-0.45) | 0.40-0.44 s (p90 0.42-0.47) |
| Answer of 17-23 words | 1.22-1.48 s (p90 1.43-1.84) | 1.23-1.32 s (p90 1.39-1.52) |
| Library, one voice + language (27 wordings) | 11.1-13.0 s | 11.7-13.3 s |
| Six named calls of one driver | 2.7-2.9 s | 2.9 s |
| OS threads before → after load | 4 → 8 (+1 per session) | 4 → 16 (+3 per session) |
| CPU while speaking | 1.1-1.7 cores | 1.2-1.5 cores |
| RAM (working set) after load / peak | 446 / 497 MB | 446 / 498 MB |

4 threads bring nothing over 2, as with the SDK: the app uses 2. The first run right after a build was about twice
as slow (load 2.6 s, short call 0.78 s, library 21.6 s; a freshly written exe and a cold model file), the next runs
were the numbers above. The SDK on the same PC: load 1.3-1.8 s, short calls 0.38-0.44 s, long 1.18-1.31 s
(`research/2026-11-09-testabend/supertonic/README.md`).

## ONNX Runtime in the app

`ort = "=2.0.0-rc.15"` (MIT OR Apache-2.0) without default features, with `std`, `download-binaries`, `tls-native`,
`api-31`. Its build script downloads pyke's prebuilt **ONNX Runtime 1.31.0** once (Windows 32 MB, Linux 11 MB, from
`cdn.pyke.io`, SHA-256 pinned inside ort-sys) into `%LOCALAPPDATA%\ort.pyke.io` / `~/.cache/ort.pyke.io` and links
it **statically**: no `onnxruntime.dll` ships, and Windows' own older `System32\onnxruntime.dll` cannot get in the
way. Build-time network is needed (GitHub runners have it); offline builds can point `ORT_LIB_LOCATION` at a local
ONNX Runtime build instead. On a Linux host the download goes through OpenSSL (`libssl-dev` at build time).

Only the CPU execution provider is used. pyke's Windows build always contains DirectML, so the exe imports
`DirectML.dll`, `d3d12.dll` and `dxgi.dll`; Windows 10 1903+ has them in System32 (here DirectML 1.8.0) and they are
never used. The exe grows by about **23 MB** (example binaries: 23.1 MB; the app today 7.9 MB); compressed
(`xz -9`, close to NSIS's LZMA) about **5.2 MB**, so the installer would go from 2.9 MB to about 8.3 MB (estimate,
the shell was not built).

**The VC++ runtime.** pyke's ONNX Runtime is built with MSVC 14.44 against the DLL runtime, so the exe imports
`MSVCP140.dll` and `MSVCP140_1.dll` (which pull in `VCRUNTIME140.dll`). The app has needed no VC++ runtime so far
(tauri-build links vcruntime statically). Linking with tauri-build's `static_vcruntime` settings works (tested with a
throwaway crate using its exact link arguments: links, loads, speaks). Every PC with AC EVO has a VC++ 2015-2022
runtime (`AssettoCorsaEVO.exe` imports `MSVCP140.dll` and ships none), but an older one than 14.40 is the known
`std::mutex` crash of programs built with MSVC 17.10+. So ship the four DLLs next to the exe (tested: Windows loads
the app-local copies first, 14.44.35211, and the voice works):

- `msvcp140.dll`, `msvcp140_1.dll`, `vcruntime140.dll`, `vcruntime140_1.dll` from
  `<Visual Studio>\VC\Redist\MSVC\14.44.*\x64\Microsoft.VC143.CRT\` (760 KB, 0.2 MB compressed). The Visual Studio
  licence lists these files as redistributable; Microsoft prefers its Redistributable installer, because app-local
  copies are not updated by Windows Update.

What the shell needs:

```toml
# src-tauri/Cargo.toml
dd-voice = { path = "../crates/dd-voice" }
```

```jsonc
// src-tauri/tauri.conf.json, "bundle": the four runtime DLLs into the install folder next to the exe
"resources": {
  "vcruntime/msvcp140.dll": "msvcp140.dll",
  "vcruntime/msvcp140_1.dll": "msvcp140_1.dll",
  "vcruntime/vcruntime140.dll": "vcruntime140.dll",
  "vcruntime/vcruntime140_1.dll": "vcruntime140_1.dll"
}
```

(copied into `src-tauri/vcruntime/` by a build step from the Visual Studio found with `vswhere`; not verified in the
real NSIS installer yet). The model is not part of the installer: the app downloads it on first activation.

`Engine` is `Send`; run it on a worker thread (`speak` takes 0.4-1.5 s). ONNX Runtime's telemetry is switched off.
ort allows one ONNX Runtime environment per process, and no second one even after the first is dropped: the voice
and the listener share one, made on the first load and kept (before, a second `Engine::load` in one process failed
with "only one environment is allowed per process").

## Model files

Downloaded when the driver first switches the radio on, from `MODEL_FILES` (pinned to Supertone's commit
`724fb5abbf5502583fb520898d45929e62f02c0b`, the one SDK 1.3.1 pins), 401,291,751 bytes (383 MiB). The SHA-256 of the
four ONNX files are the repository's LFS ids; all files were checked against the copy the SDK downloaded and against
the commit's git blob ids; every URL answers (ranged GET, 206).

## Licence of the model

BigScience OpenRAIL-M (`LICENSE`, downloaded with the model). Commercial use is allowed. The app has to:

- put the use restrictions of Attachment A (paragraph 5) into its terms as enforceable provisions, and tell users the
  model is subject to them (4.a);
- give users a copy of the licence (4.b; the `LICENSE` file is one of the model files);
- keep the notices (4.d): `LICENCE_NOTICE`, "Supertonic 3, Copyright (c) 2026 Supertone Inc., licensed under the
  BigScience OpenRAIL-M License";
- say that the voices are machine generated (Attachment A (e)): "Radio voices are AI-generated." /
  "Funkstimmen sind KI-generiert."

# Listening

```rust
let mut listener = Listener::load(&model_dir, 2)?;           // Whisper base: encoder + merged decoder, 2 threads
let samples = resample(&mic_samples, mic_rate);              // mono f32 at any rate to Listener::RATE (16 kHz)
let text = listener.transcribe(&samples, "de")?;             // "Wie viel Sprit habe ich noch?"; "" for silence/noise
let t = listener.transcript(&samples, "de")?;                // the same with verdict, tokens, no-speech probability
verify_listener(&model_dir, true)?;                          // sizes, and SHA-256 on request
```

Push-to-talk recordings up to 30 s (Whisper's window; anything after is not heard). The language is the radio's,
forced: no detection. Errors: `model-missing:<file>`, `model-damaged:<file>`, `unknown-language`, `runtime`.

## Pipeline

As OpenAI's `whisper/decoding.py` and Hugging Face's `generate` do it, written in Rust (`audio.rs`, `tokens.rs`,
`listen.rs`), on Hugging Face's ONNX export:

- **Log-mel**: the recording padded with zeros to 30 s (the export takes nothing shorter: its positional embedding
  is a fixed 1500 frames), centred STFT with reflect padding, periodic Hann window 400, hop 160, power spectrum,
  80 Slaney mel filters (librosa's `filters.mel`), `log10` with a 1e-10 floor, clamped to 8 below the maximum,
  `(x + 4) / 4`. The FFT is a 40-line mixed-radix Cooley-Tukey (400 = 2^4 x 5^2); frames that are all padding are
  skipped (they are exactly zero). 13 ms for a 3 s recording, 25 ms for 7 s, 90 ms for 30 s (Linux, release).
- **Encoder** once, **decoder** on `<|startoftranscript|> <|de|> <|transcribe|> <|notimestamps|>`, then one token per
  step with the cached keys and values (the cross-attention ones from the first step), greedy. Never chosen: the
  generation config's 88 `suppress_tokens` (symbols, the task tokens); not as the first token: its
  `begin_suppress_tokens` (blank, end of text). At most `max_tokens`: 16 + 10 per second of recording (the test
  questions give at most 5.3 per second), at most 224.
- **Text**: GPT-2 byte-level BPE decoding (stand-in characters back to bytes, UTF-8), special tokens dropped, trimmed,
  whitespace runs as one space. Nothing else (no number formatting, no casing).
- **Resampler** (`resample`): windowed sinc (Hann, 16 zero crossings), cut-off at 95 % of the lower Nyquist, one
  coefficient table per phase of the rational ratio (44.1 kHz to 16 kHz: 160 phases of 89 taps). A 12 kHz tone at
  44.1 kHz (it would fold to 4 kHz) comes out below -40 dB.

**Silence and noise** must give no text: Whisper invents text for them (known for "Untertitel der Amara.org-Community"
and "Thank you."; the reference wrote "und dann kommt das ganze.", "SWR 2020" and "you" on this test set's digital
silence and hiss). Three guards, `Transcript::verdict`:

| Verdict | When | Cost |
|---|---|---|
| `Quiet` | the loudest 20 ms are below `GATE_DBFS` (-50 dBFS RMS) | the model does not run |
| `NoSpeech` | OpenAI's rule (`transcribe.py`): no-speech probability (`<|nocaptions|>` after the start token) > 0.6 **and** average log-probability < -1 | a full run |
| `Runaway` | no end of text within `max_tokens` (a loop) | a full run plus the token limit |

On the test set: all 144 speech clips are `Speech` (no-speech at most 0.36, average log-probability at least -0.52);
digital silence and -60 dBFS hiss are `Quiet`; pink noise at -45, -35 and -25 dBFS (5 clips x de/en) is `NoSpeech`
every time (no-speech at least 0.88, log-probability at most -1.69), although the model wrote "I'm going to use the
same color as the other one." for one of them. The -50 dBFS gate is a guess for a real microphone (a quiet headset
with low gain may speak below it): check it with the driver's own recordings.

## Model choice

All candidates are `onnx-community`'s exports of the multilingual models (the format Transformers.js and optimum
use: `encoder_model*.onnx` + `decoder_model_merged*.onnx` with past keys/values). Test set: 24 questions (12 de,
12 en: fuel, tyres, brakes, lap time, gap, "wo verliere ich Zeit", box, Hatzenbach, Schwedenkreuz, Döttinger Höhe,
numbers; `tests/listen_sentences.tsv`) spoken by Supertonic M1, F2 and M4, resampled to 16 kHz, with 0.3 s of
silence before and 0.4 s after, clean and with pink noise at 20 dB SNR: 144 clips. Word error rate after lower
case, punctuation off, number words as digits, "tyres" as "tires", "ß" as "ss". Windows 11, i7-11700, 2 intra-op
threads, release build, CPU load 4-8 % (Blender open, other builds in WSL at times):

| Model | Download | Working set | Load | 2.8 s question | 6.2 s question | WER de clean / noisy | WER en clean / noisy | Noise without text |
|---|---|---|---|---|---|---|---|---|
| **base int8** | **74 MiB** | **460 MB** | **0.6 s** | **0.80 s** | **1.11 s** | **6.5 / 6.9 %** | **6.4 / 6.7 %** | **14/14** |
| base fp32 | 277 MiB | 1036 MB | 1.5 s | 0.72 s | 0.93 s | 5.7 / 5.4 % | 6.4 / 6.7 % | 14/14 |
| base fp32 encoder + int8 decoder | 131 MiB | 548 MB | 0.7 s | 0.74 s | 1.51 s | 8.4 / 5.4 % | 6.4 / 6.7 % | 14/14 |
| small int8 | 238 MiB | 743 MB | 3.4 s | 2.56 s | 2.15 s | 3.8 / 3.8 % | 7.7 / 7.1 % | 13/14 |
| small fp32 | 924 MiB | 2135 MB | 2.9 s | 2.27 s | 2.63 s | 3.4 / 3.8 % | 6.7 / 6.4 % | 13/14 |

(Times: medians over the 58 clean clips under 3.6 s and the 12 over 5 s, one pass each; see "Measured" for repeated
runs.) **Chosen: base int8.** Small is better in German (the corner names: "Döttinger Höhe" right where base writes
"Dürtinger Höhe"), not in English, but takes 2.2-2.6 s: the 30 s window makes its encoder cost 2.2 s whatever the
question's length, far over the 0.5-1 s budget. Small also wrote "you" on quiet pink noise with an average
log-probability of -0.94, which passes OpenAI's rule, and looped ("Rettungs-Rettungs-...") on louder noise. base fp32
is a little better in German but 3.7 times the download and twice the memory. The errors base int8 makes are
mostly proper names ("Hatzenbach" as "Herzenbach"/"Hatsenbach", "Schwedenkreuz" in English as "Sweden crews",
"Döttinger Höhe"), "braking" as "breaking", "verliere" as "verlier", "Wie viel" as "Wieviel": an answer model that
knows the track will read them right; a prompt with the corner names (Whisper's `<|startofprev|>`) could help later.

## Fidelity against the reference

`research/2026-11-09-testabend/listen/reference.py` (workspace) runs the same clips and the same ONNX files through
Hugging Face's own code: `WhisperFeatureExtractor` and optimum-onnx 0.1.0's `ORTModelForSpeechSeq2Seq.generate`
(transformers 4.57.6, onnxruntime 1.31.1, Linux), greedy, language and task forced, `max_new_tokens` = `max_tokens`.

- **Log-mel**: max |Rust - HF numpy| = **1.2e-7** over 151 clips (float32 rounding). Against HF's torch path (= OpenAI's
  `audio.py`, torch.stft in float32) 5.5e-5; HF documents those two as 1e-5 apart.
- **Tokens, base fp32: 154/158 clips identical**; the 4 others are digital silence and -60 dBFS hiss, which the level
  gate keeps from the model (the reference wrote "Ich bin ein bisschen mehr.", "SWR 2020", "you").
- **Tokens, base int8**: 151/158 identical: the same 4, plus 2 speech clips and 1 noise clip one near-tied token apart
  ("Dürtinger" vs "Dürtlinger", a final "." vs "?"). Fed the Rust log-mel bits, the reference agrees on more of them;
  the rest is the int8 kernels of two ONNX Runtime builds (pyke's 1.31.0 here, PyPI's 1.31.1 there). Same WER.

## Measured: listener (Windows 11, i7-11700, power plan Balanced, AC EVO not running)

`cargo run --release -p dd-voice --example listen_bench -- <whisper model> <supertonic model> 2 <wav>..`: five runs
per question after a warm-up, two passes, CPU load 0-3 % before each:

| | Listener alone | Listener + voice in the same process |
|---|---|---|
| Load | 0.55-0.61 s | voice 0.8-1.0 s on top |
| 3 s question (3.1 s de, 2.9 s en) | 0.66-0.82 s (min 0.65) | 0.92-0.94 s |
| 6.6 s question (de, en) | 0.86-1.18 s | 1.14-1.20 s |
| CPU while listening | 1.2-1.4 cores | 1.3-1.4 cores |
| Working set after the first question / peak | 449-460 MB / 457-467 MB | 886-887 MB / 894 MB |
| OS threads | 4 -> 6 | 12 |

Four passes (two at 14:20, two at 15:05); the 6.6 s questions took 0.86-0.94 s in the first two and 1.16-1.18 s in
the last two, so the spread is the PC's state (turbo, WSL builds of other sessions), not the listener. The text was
the same in every run. With the voice loaded a question is about 0.15-0.25 s slower (both share the cores and the
memory bandwidth; the voice was idle while listening). Most of the time is the encoder on the 30 s window,
the same for every length; the tokens add the rest (8-9 for the 3 s questions, 20 for the 6.6 s ones).

ONNX Runtime keeps what a run allocated for the next one (its arena): the encoder's activations for the 30 s window
stay with the process. Without the arena (`ep::CPU::with_arena_allocator(false)` + no memory pattern) the working
set stays at 190-203 MB (peak 338-349 MB), but every question pays for fresh pages: 0.87-1.16 s for 3 s questions,
1.21-1.49 s for 6.6 s ones. The listener keeps ONNX Runtime's default (as the voice does); switching is two lines
in `Listener::load_with` if memory matters more than 0.2-0.5 s.

## Model files and licence

Downloaded when the driver first switches the coach's microphone on, from `LISTEN_MODEL_FILES` (pinned to
`onnx-community/whisper-base` commit `27a07b4b52b3a4c5886134bb4320e93e1a4bbc7c`), 77,971,875 bytes (74 MiB):
`config.json`, `generation_config.json`, `added_tokens.json`, `vocab.json`, `onnx/encoder_model_int8.onnx`,
`onnx/decoder_model_merged_int8.onnx`. SHA-256 of the ONNX files = the repository's LFS ids, the JSON files match the
commit's git blob ids; every URL answers (ranged GET, 206).

Whisper's code and weights are MIT (Copyright (c) 2022 OpenAI, `github.com/openai/whisper`); Hugging Face's
`openai/whisper-base` and the `onnx-community/whisper-base` export are marked Apache-2.0 (the export's model card;
no licence file in the repository). Both allow commercial use with the notice. The app shows `LISTEN_LICENCE_NOTICE`:
"Speech recognition: Whisper by OpenAI, Copyright (c) 2022 OpenAI, MIT License (…); ONNX export by onnx-community on
Hugging Face (…, Apache License 2.0)." No use restrictions as with the voice's OpenRAIL-M.

## Tests

`cargo test -p dd-voice` runs everywhere without the model. With it:
`DD_SUPERTONIC_MODEL=<model folder> cargo test -p dd-voice` (Windows:
`set DD_SUPERTONIC_MODEL=C:\…\model&& cargo test -p dd-voice`). The model tests return early with a message when the
variable is unset (not `#[ignore]` like `DD_MOTEC_FIXTURE`, so that setting the variable is enough).

The listener's tests read `DD_WHISPER_MODEL` (the folder with `LISTEN_MODEL_FILES`): silence and noise give no text,
missing files are named, wrong languages refused; with `DD_SUPERTONIC_MODEL` as well, the round trip speaks the 24
questions with three voices, clean and noisy, and checks every WER stays below 12 % (measured 6.4-7.7 %). The test
audio is made in the test, not kept. Pure tests (FFT against a DFT, mel filters against the reference's values,
resampler, BPE decoding on a tiny vocabulary, suppression, level, token limit) need no model.

`listen_eval` renders the test set as WAVs and runs any export variant over it (`results-<label>.tsv` per clip, the
WER table), `listen_eval mel` dumps the log-mels for `reference.py`; `transcribe` prints what the listener hears in
a WAV of any rate: `DD_WHISPER_MODEL=<folder> cargo run --release -p dd-voice --example transcribe -- <wav> de`.

TTS speech is easier than a driver in a rig: clean diction, no accent, no headset or room colouring, no wheel base or
engine noise, no speaking while braking. These numbers are an upper bound until real recordings are tested.
