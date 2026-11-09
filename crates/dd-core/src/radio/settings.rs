//! The team radio's settings on this PC (`radio.json` in the app's config folder): the headset, the voice and what
//! the coach says are a matter of the PC the driver sits at, so the platform does not keep them. The driver's radio
//! name is the platform's, because other drivers hear it.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

const FILE: &str = "radio.json";

/// The voices the engine brings: five male, five female, each speaks German and English.
pub const VOICES: [&str; 10] = ["M1", "M2", "M3", "M4", "M5", "F1", "F2", "F3", "F4", "F5"];
pub const LANGUAGES: [&str; 2] = ["de", "en"];

/// Which kinds of calls the driver wants to hear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Groups {
    /// Cars beside the driver: left, right, three wide, clear
    pub spotter: bool,
    /// Cars closing from behind, close behind, in the slipstream, slow ahead
    pub traffic: bool,
    pub fuel: bool,
    /// Lap times, personal bests, invalid laps
    pub laps: bool,
    /// Brakes, tyres, pit limiter
    pub car: bool,
}

impl Default for Groups {
    fn default() -> Self {
        Self { spotter: true, traffic: true, fuel: true, laps: true, car: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Off until the driver switches the team radio on
    pub enabled: bool,
    pub voice: String,
    /// The language the coach speaks, "de" or "en"
    pub language: String,
    /// 0 to 100
    pub volume: u8,
    /// How much of the radio sound lies on the voice, 0 (the clean voice) to 100
    pub effect: u8,
    /// The output device's id as the sound system names it; None for the PC's default device
    pub device: Option<String>,
    pub groups: Groups,
}

impl Default for Settings {
    fn default() -> Self {
        Self { enabled: false, voice: VOICES[0].to_string(), language: LANGUAGES[0].to_string(), volume: 80, effect: 100, device: None, groups: Groups::default() }
    }
}

impl Settings {
    /// What a file or a page may have put in, brought back into what exists.
    fn sane(mut self) -> Self {
        let defaults = Self::default();
        if !VOICES.contains(&self.voice.as_str()) {
            self.voice = defaults.voice;
        }
        if !LANGUAGES.contains(&self.language.as_str()) {
            self.language = defaults.language;
        }
        self.volume = self.volume.min(100);
        self.effect = self.effect.min(100);
        self.device = self.device.filter(|device| !device.is_empty() && device.len() <= 300);
        self
    }
}

/// The settings on this PC; the defaults while the driver has set nothing or the file is unreadable.
pub fn load(dir: &Path) -> Settings {
    fs::read(dir.join(FILE)).ok().and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok()).unwrap_or_default().sane()
}

/// Changes the settings the page names (a JSON object with any of the settings' fields) and answers what holds now.
pub fn change(dir: &Path, changes: &serde_json::Value) -> io::Result<Settings> {
    let mut merged = serde_json::to_value(load(dir))?;
    merge(&mut merged, changes);
    let settings = serde_json::from_value::<Settings>(merged).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?.sane();
    fs::create_dir_all(dir)?;
    fs::write(dir.join(FILE), serde_json::to_vec_pretty(&settings)?)?;
    Ok(settings)
}

/// Objects merge field by field; anything else replaces.
fn merge(into: &mut serde_json::Value, from: &serde_json::Value) {
    match (into, from) {
        (serde_json::Value::Object(into), serde_json::Value::Object(from)) => {
            for (key, value) in from {
                merge(into.entry(key.clone()).or_insert(serde_json::Value::Null), value);
            }
        }
        (into, from) => *into = from.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dd-radio-settings-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_radio_is_off_with_every_call_group_on_until_the_driver_chooses() {
        let dir = dir("defaults");
        let settings = load(&dir);
        assert_eq!(settings, Settings::default());
        assert!(!settings.enabled);
        assert_eq!((settings.voice.as_str(), settings.language.as_str(), settings.volume, settings.effect), ("M1", "de", 80, 100));
        assert_eq!(settings.groups, Groups { spotter: true, traffic: true, fuel: true, laps: true, car: true });
    }

    #[test]
    fn a_change_touches_only_what_it_names_and_stays() {
        let dir = dir("change");
        let settings = change(&dir, &json!({ "enabled": true, "voice": "F3", "groups": { "fuel": false } })).unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.voice, "F3");
        assert_eq!(settings.groups, Groups { fuel: false, ..Groups::default() });
        let settings = change(&dir, &json!({ "volume": 55, "device": "wasapi:{0.0.0.00000000}.{abc}" })).unwrap();
        assert_eq!((settings.enabled, settings.voice.as_str(), settings.volume), (true, "F3", 55));
        assert_eq!(settings.device.as_deref(), Some("wasapi:{0.0.0.00000000}.{abc}"));
        assert!(!settings.groups.fuel);
        assert_eq!(load(&dir), settings);
        assert_eq!(change(&dir, &json!({ "device": null })).unwrap().device, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_does_not_exist_falls_back_and_a_wrong_type_is_refused() {
        let dir = dir("sane");
        let settings = change(&dir, &json!({ "voice": "X9", "language": "fr", "volume": 250, "effect": 101, "unknown": 1 })).unwrap();
        assert_eq!((settings.voice.as_str(), settings.language.as_str(), settings.volume, settings.effect), ("M1", "de", 100, 100));
        assert!(change(&dir, &json!({ "enabled": "yes" })).is_err());
        assert!(change(&dir, &json!({ "volume": 300 })).is_err(), "more than a byte holds");
        // a broken file counts as no file
        fs::write(dir.join(FILE), b"{").unwrap();
        assert_eq!(load(&dir), Settings::default());
        fs::remove_dir_all(&dir).unwrap();
    }
}
