# dd-voice

The team radio's voice: [Supertonic 3](https://huggingface.co/Supertone/supertonic-3) (Supertone Inc.) speaking German
and English on the CPU through ONNX Runtime, from Rust. The words come from `dd_core::radio::phrases`.

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

## Tests

`cargo test -p dd-voice` runs everywhere without the model. With it:
`DD_SUPERTONIC_MODEL=<model folder> cargo test -p dd-voice` (Windows:
`set DD_SUPERTONIC_MODEL=C:\…\model&& cargo test -p dd-voice`). The model tests return early with a message when the
variable is unset (not `#[ignore]` like `DD_MOTEC_FIXTURE`, so that setting the variable is enough).
