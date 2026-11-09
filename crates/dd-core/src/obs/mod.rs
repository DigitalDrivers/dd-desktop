//! The OBS helper of the replay page: the app records a replay through the driver's OBS Studio (32 or newer) in its
//! own profile and scene collection "Digital Drivers", with the best settings this PC holds without dropped frames.
//! What is pure lives here and is tested on any machine; the shell (`src-tauri/src/obs.rs`) finds and starts OBS,
//! talks to it and writes the files.
//!
//! - `tiers`: the settings ladder (encoder, preset, output size) per graphics card, down when the test drops frames,
//!   up when it holds.
//! - `profile`: the profile (`basic.ini`, `recordEncoder.json`) and the scene collection OBS loads.
//! - `stats`: the quality test's numbers from obs-websocket's statistics, its verdict and the disk check.
//! - `websocket`: the obs-websocket 5 protocol (login, requests, events) without the connection itself.
//! - `config`: where OBS keeps its files, the driver's own profile and collection, and the websocket switch.
//!
//! Specification: research/2026-11-08-replay-video/A-obs-quality.md; the mechanics were proven against OBS 33.0.1 and
//! obs-websocket 5.7.6 on Joe's PC on 2026-11-09 (research/2026-11-09-testabend/replay).

pub mod config;
pub mod profile;
pub mod stats;
pub mod tiers;
pub mod websocket;
