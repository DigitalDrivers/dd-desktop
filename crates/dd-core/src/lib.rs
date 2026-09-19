//! Platform-independent logic of the Digital Drivers desktop app.
//!
//! Everything here is plain Rust without UI or Tauri dependencies, so it can be unit-tested
//! on any machine (including Linux/WSL), while the Tauri shell itself is built on Windows.

pub mod join;
pub mod scrutineering;
pub mod steam;
