//! The model files the app downloads when the driver first switches the team radio on: Supertonic 3 from
//! Supertone's Hugging Face repository at the commit the SDK 1.3.1 pins (`MODEL_CONFIGS["supertonic-3"]`), with
//! size and SHA-256 of each file. The SHA-256 values are the repository's own (LFS ids of the ONNX files) and
//! were checked against a download made by the SDK on 2026-11-09; the other files match the commit's git blob ids.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::VoiceError;

/// The commit of `Supertone/supertonic-3` every download URL is pinned to.
pub const MODEL_REVISION: &str = "724fb5abbf5502583fb520898d45929e62f02c0b";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    /// The path below the model folder, with forward slashes, as in the repository.
    pub path: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub url: &'static str,
}

macro_rules! file {
    ($path:literal, $size:literal, $sha256:literal) => {
        ModelFile {
            path: $path,
            size: $size,
            sha256: $sha256,
            url: concat!("https://huggingface.co/Supertone/supertonic-3/resolve/724fb5abbf5502583fb520898d45929e62f02c0b/", $path),
        }
    };
}

/// Everything the engine reads, plus the licence that has to travel with the model (OpenRAIL-M 4.b).
/// 401,291,751 bytes (383 MiB) in all.
pub const MODEL_FILES: [ModelFile; 17] = [
    file!("LICENSE", 15_007, "0d944a9110fed9a9602d60e0423a272903e7bd21ab060490774efc77c2275e9f"),
    file!("onnx/tts.json", 8_253, "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09"),
    file!("onnx/unicode_indexer.json", 277_676, "9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f"),
    file!("onnx/duration_predictor.onnx", 3_700_147, "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db"),
    file!("onnx/text_encoder.onnx", 36_416_150, "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff"),
    file!("onnx/vector_estimator.onnx", 256_534_781, "883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c"),
    file!("onnx/vocoder.onnx", 101_424_195, "085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba"),
    file!("voice_styles/M1.json", 291_748, "e35604687f5d23694b8e91593a93eec0e4eca6c0b02bb8ed69139ab2ea6b0a5b"),
    file!("voice_styles/M2.json", 292_055, "b76cbf62bac707c710cf0ae5aba5e31eea1a6339a9734bfae33ab98499534a50"),
    file!("voice_styles/M3.json", 290_198, "ea1ac35ccb91b0d7ecad533a2fbd0eec10c91513d8951e3b25fbba99954e159b"),
    file!("voice_styles/M4.json", 291_522, "ca8eefad4fcd989c9379032ff3e50738adc547eeb5e221b82593a6d7b3bac303"),
    file!("voice_styles/M5.json", 291_469, "dd22b92740314321f8ae11c5e87f8dd60d060f15dd3a632b5adf77f471f77af2"),
    file!("voice_styles/F1.json", 292_046, "bbdec6ee00231c2c742ad05483df5334cab3b52fda3ba38e6a07059c4563dbc2"),
    file!("voice_styles/F2.json", 292_423, "7c722c6a72707b1a77f035d67f0d1351ba187738e06f7683e8c72b1df3477fc6"),
    file!("voice_styles/F3.json", 290_794, "12f6ef2573baa2defa1128069cb59f203e3ab67c92af77b42df8a0e3a2f7c6ab"),
    file!("voice_styles/F4.json", 291_808, "c2fa764c1225a76dfc3e2c73e8aa4f70d9ee48793860eb34c295fff01c2e032b"),
    file!("voice_styles/F5.json", 291_479, "45966e73316415626cf41a7d1c6f3b4c70dbc1ba2bee5c1978ef0ce33244fc8d"),
];

/// The voice files in the order of [`crate::VOICES`].
pub(crate) const VOICE_FILES: [&str; 10] = [
    "voice_styles/M1.json",
    "voice_styles/M2.json",
    "voice_styles/M3.json",
    "voice_styles/M4.json",
    "voice_styles/M5.json",
    "voice_styles/F1.json",
    "voice_styles/F2.json",
    "voice_styles/F3.json",
    "voice_styles/F4.json",
    "voice_styles/F5.json",
];

/// What the app shows next to the voice (model card and OpenRAIL-M 4.d: keep the copyright and attribution
/// notices). The use restrictions of the licence's Attachment A belong in the app's terms (OpenRAIL-M 4.a), and
/// the `LICENSE` file is downloaded with the model (4.b).
pub const LICENCE_NOTICE: &str = "Radio voices: Supertonic 3, Copyright (c) 2026 Supertone Inc., licensed under the BigScience OpenRAIL-M \
License (https://huggingface.co/Supertone/supertonic-3/blob/724fb5abbf5502583fb520898d45929e62f02c0b/LICENSE). \
Radio voices are AI-generated.";

fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut reader = BufReader::with_capacity(1 << 20, File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match reader.read(&mut buf)? {
            0 => break,
            n => hasher.update(&buf[..n]),
        }
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether every model file is in `model_dir` with its size, and with `sha256` also its SHA-256 (reads all
/// 383 MiB, about a second; worth it once after the download, not on every start).
pub fn verify(model_dir: &Path, sha256: bool) -> Result<(), VoiceError> {
    check(&MODEL_FILES, model_dir, sha256)
}

fn check(files: &[ModelFile], model_dir: &Path, sha256: bool) -> Result<(), VoiceError> {
    for file in files {
        let path = model_dir.join(file.path);
        let size = std::fs::metadata(&path).map_err(|_| VoiceError::ModelMissing(file.path))?.len();
        if size != file.size {
            return Err(VoiceError::ModelDamaged(file.path));
        }
        if sha256 && sha256_of(&path).map_err(|_| VoiceError::ModelMissing(file.path))? != file.sha256 {
            return Err(VoiceError::ModelDamaged(file.path));
        }
    }
    Ok(())
}

/// The commit of `onnx-community/whisper-base` every download URL of the listener is pinned to.
pub const LISTEN_MODEL_REVISION: &str = "27a07b4b52b3a4c5886134bb4320e93e1a4bbc7c";

macro_rules! listen_file {
    ($path:literal, $size:literal, $sha256:literal) => {
        ModelFile {
            path: $path,
            size: $size,
            sha256: $sha256,
            url: concat!("https://huggingface.co/onnx-community/whisper-base/resolve/27a07b4b52b3a4c5886134bb4320e93e1a4bbc7c/", $path),
        }
    };
}

/// The listener's ONNX files.
pub(crate) const LISTEN_ENCODER: &str = "onnx/encoder_model_int8.onnx";
pub(crate) const LISTEN_DECODER: &str = "onnx/decoder_model_merged_int8.onnx";

/// Everything the listener reads: Whisper base (multilingual) as `onnx-community`'s export, the int8-quantised
/// encoder and merged decoder. 77,971,875 bytes (74 MiB) in all. The SHA-256 of the ONNX files are the
/// repository's LFS ids, the JSON files match the commit's git blob ids (checked 2026-11-09).
pub const LISTEN_MODEL_FILES: [ModelFile; 6] = [
    listen_file!("config.json", 2_243, "f4d0608f7d918166da7edb3e188de5ef1bfe70d9802e785d271fd88111e9cf4b"),
    listen_file!("generation_config.json", 3_832, "61070cf8de25b1e9256e8e102ded49d8d24a8369ed36ef84fdf21549e68125a0"),
    listen_file!("added_tokens.json", 34_604, "9715fd2243b6f06a5858b5e32950d2853f73dd5bc201aafcf76f5082a2d8acd1"),
    listen_file!("vocab.json", 1_036_584, "50d6a919f0a0601d56a04eb583c780d18553aa388254ba3158eb6a00f13e2c1a"),
    listen_file!("onnx/encoder_model_int8.onnx", 23_201_297, "ca6177401f86a2c6b4dc5f7fc02fbca680678906bd0c22f6d89f0b80f124253f"),
    listen_file!("onnx/decoder_model_merged_int8.onnx", 53_693_315, "fa3ef9902734ce5ae6f9ef2bdb2ba9a6c4b5785b09f4f420ce036573dc9d090b"),
];

/// What the app shows next to the radio's speech recognition.
pub const LISTEN_LICENCE_NOTICE: &str = "Speech recognition: Whisper by OpenAI, Copyright (c) 2022 OpenAI, MIT License \
(https://github.com/openai/whisper/blob/main/LICENSE); ONNX export by onnx-community on Hugging Face \
(https://huggingface.co/onnx-community/whisper-base, Apache License 2.0).";

/// Whether every listener file is in `model_dir` with its size, and with `sha256` also its SHA-256 (74 MiB).
pub fn verify_listener(model_dir: &Path, sha256: bool) -> Result<(), VoiceError> {
    check(&LISTEN_MODEL_FILES, model_dir, sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_consistent() {
        assert_eq!(MODEL_FILES.iter().map(|f| f.size).sum::<u64>(), 401_291_751);
        for f in &MODEL_FILES {
            assert!(f.url.ends_with(f.path) && f.url.contains(MODEL_REVISION), "{}", f.url);
            assert!(f.sha256.len() == 64 && f.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{}", f.path);
        }
        for (voice, file) in crate::VOICES.iter().zip(VOICE_FILES) {
            assert_eq!(file, format!("voice_styles/{voice}.json"));
            assert!(MODEL_FILES.iter().any(|f| f.path == file));
        }
    }

    #[test]
    fn the_listener_table_is_consistent() {
        assert_eq!(LISTEN_MODEL_FILES.iter().map(|f| f.size).sum::<u64>(), 77_971_875);
        for f in &LISTEN_MODEL_FILES {
            assert!(f.url.ends_with(f.path) && f.url.contains(LISTEN_MODEL_REVISION), "{}", f.url);
            assert!(f.sha256.len() == 64 && f.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{}", f.path);
        }
        for path in [LISTEN_ENCODER, LISTEN_DECODER] {
            assert!(LISTEN_MODEL_FILES.iter().any(|f| f.path == path), "{path}");
        }
    }

    #[test]
    fn verify_names_the_first_missing_or_damaged_file() {
        let dir = std::env::temp_dir().join(format!("dd-voice-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("onnx")).unwrap();
        assert_eq!(verify(&dir, false), Err(VoiceError::ModelMissing("LICENSE")));
        std::fs::write(dir.join("LICENSE"), vec![b'x'; 15_007]).unwrap();
        assert_eq!(verify(&dir, false), Err(VoiceError::ModelMissing("onnx/tts.json")));
        assert_eq!(verify(&dir, true), Err(VoiceError::ModelDamaged("LICENSE")), "right size, wrong content");
        std::fs::write(dir.join("onnx/tts.json"), b"{}").unwrap();
        std::fs::write(dir.join("LICENSE"), b"short").unwrap();
        assert_eq!(verify(&dir, false), Err(VoiceError::ModelDamaged("LICENSE")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
