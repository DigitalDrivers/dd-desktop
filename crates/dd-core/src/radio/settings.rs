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

/// The button the driver holds to ask the coach: a key of the keyboard or a button of a game controller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Button {
    /// `code` is the key's Windows virtual-key code, `name` how the keyboard names it
    Key { code: u32, name: String },
    /// `device` is the controller's id as the app lists it, `button` its button's number from 0
    #[serde(rename_all = "camelCase")]
    Controller { device: String, device_name: String, button: u32 },
}

impl Button {
    /// Whether it names something that can exist: a key that is not a mouse button, one of a controller's 128
    /// buttons, ids and names of a sane length.
    fn sane(&self) -> bool {
        let text = |text: &str, max: usize| !text.is_empty() && text.len() <= max;
        match self {
            Button::Key { code, name } => (0x08..=0xFE).contains(code) && text(name, 100),
            Button::Controller { device, device_name, button } => *button < 128 && text(device, 300) && text(device_name, 300),
        }
    }
}

/// The coach the driver asks by holding a button: the microphone is open only while it is held.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Coach {
    /// Off until the driver switches the coach's microphone on
    pub enabled: bool,
    pub button: Option<Button>,
    /// The input device's id as the sound system names it; None for the PC's default microphone
    pub microphone: Option<String>,
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
    pub coach: Coach,
}

impl Default for Settings {
    fn default() -> Self {
        Self { enabled: false, voice: VOICES[0].to_string(), language: LANGUAGES[0].to_string(), volume: 80, effect: 100, device: None, groups: Groups::default(), coach: Coach::default() }
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
        self.coach.button = self.coach.button.filter(Button::sane);
        self.coach.microphone = self.coach.microphone.filter(|device| !device.is_empty() && device.len() <= 300);
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
    // a button replaces the one before as a whole: a key's fields and a controller's do not mix
    if changes.pointer("/coach/button").is_some() {
        merged["coach"]["button"] = serde_json::Value::Null;
    }
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

    #[test]
    fn the_coach_s_microphone_is_off_without_a_button_until_the_driver_chooses() {
        let dir = dir("coach-defaults");
        assert_eq!(load(&dir).coach, Coach { enabled: false, button: None, microphone: None });
        // a file from before the coach keeps its settings and gets the coach's defaults
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE), br#"{ "enabled": true, "voice": "F2", "volume": 40 }"#).unwrap();
        let settings = load(&dir);
        assert_eq!((settings.enabled, settings.voice.as_str(), settings.volume), (true, "F2", 40));
        assert_eq!(settings.coach, Coach::default());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_button_is_kept_with_its_kind_and_a_new_one_replaces_it_whole() {
        let dir = dir("coach-button");
        let wheel = json!({ "kind": "controller", "device": "C29B046D-0000-0000-0000-504944564944/1A2B", "deviceName": "Logitech G25 Racing Wheel USB", "button": 4 });
        let settings = change(&dir, &json!({ "coach": { "enabled": true, "button": wheel, "microphone": "wasapi:{0.0.1.00000000}.{mic}" } })).unwrap();
        assert_eq!(settings.coach.button, Some(Button::Controller { device: "C29B046D-0000-0000-0000-504944564944/1A2B".into(), device_name: "Logitech G25 Racing Wheel USB".into(), button: 4 }));
        assert_eq!(settings.coach.microphone.as_deref(), Some("wasapi:{0.0.1.00000000}.{mic}"));
        let file: serde_json::Value = serde_json::from_slice(&fs::read(dir.join(FILE)).unwrap()).unwrap();
        assert_eq!(file["coach"]["button"], wheel, "as the page sends it");
        // the key takes the controller's place: nothing of the controller stays
        let settings = change(&dir, &json!({ "coach": { "button": { "kind": "key", "code": 0x7C, "name": "F13" } } })).unwrap();
        assert_eq!(settings.coach.button, Some(Button::Key { code: 0x7C, name: "F13".into() }));
        let file: serde_json::Value = serde_json::from_slice(&fs::read(dir.join(FILE)).unwrap()).unwrap();
        assert_eq!(file["coach"]["button"], json!({ "kind": "key", "code": 0x7C, "name": "F13" }));
        assert!(settings.coach.enabled && settings.coach.microphone.is_some(), "the rest of the coach stays");
        assert_eq!(load(&dir), settings);
        assert_eq!(change(&dir, &json!({ "coach": { "button": null, "microphone": null } })).unwrap().coach, Coach { enabled: true, button: None, microphone: None });
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_button_of_a_wrong_shape_is_refused_and_one_that_cannot_exist_is_dropped() {
        let dir = dir("coach-sane");
        assert!(change(&dir, &json!({ "coach": { "button": { "kind": "mouse", "code": 1 } } })).is_err());
        assert!(change(&dir, &json!({ "coach": { "button": { "kind": "key", "name": "F13" } } })).is_err(), "no code");
        assert!(change(&dir, &json!({ "coach": { "button": "F13" } })).is_err());
        assert!(change(&dir, &json!({ "coach": { "enabled": "yes" } })).is_err());
        for button in [
            json!({ "kind": "key", "code": 1, "name": "Left mouse button" }),
            json!({ "kind": "key", "code": 0x7C, "name": "" }),
            json!({ "kind": "key", "code": 0x7C, "name": "x".repeat(101) }),
            json!({ "kind": "controller", "device": "id", "deviceName": "G25", "button": 128 }),
            json!({ "kind": "controller", "device": "", "deviceName": "G25", "button": 0 }),
        ] {
            assert_eq!(change(&dir, &json!({ "coach": { "button": button } })).unwrap().coach.button, None, "{button}");
        }
        assert_eq!(change(&dir, &json!({ "coach": { "microphone": "" } })).unwrap().coach.microphone, None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
