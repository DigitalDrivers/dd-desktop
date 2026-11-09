//! The team radio's settings on this PC (`radio.json` in the app's config folder): the headset, the voice and what
//! the spotter, the race engineer and the coach say are a matter of the PC the driver sits at, so the platform does
//! not keep them. The driver's radio name is the platform's, because other drivers hear it.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::engineer::Group;
use super::queue::{Call, Priority};

const FILE: &str = "radio.json";

/// The voices the engine brings: five male, five female, each speaks German and English.
pub const VOICES: [&str; 10] = ["M1", "M2", "M3", "M4", "M5", "F1", "F2", "F3", "F4", "F5"];
pub const LANGUAGES: [&str; 2] = ["de", "en"];

/// The coach (push-to-talk, live tips, the stint's summary, the AI's answers) ships later. While this is false the
/// settings page hides the coach's block and the coach stays off whatever a file says; its code stays.
pub const COACH_AVAILABLE: bool = false;

/// The spotter: cars beside the driver and, with `traffic`, cars around him.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Spotter {
    /// Cars beside the driver: left, right, three wide, clear
    pub enabled: bool,
    /// Cars closing from behind, close behind, in the slipstream, slow ahead
    pub traffic: bool,
}

impl Default for Spotter {
    fn default() -> Self {
        Self { enabled: true, traffic: true }
    }
}

/// The race engineer and the kinds of calls he makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Engineer {
    pub enabled: bool,
    /// Lap times, personal bests, invalid laps
    pub laps: bool,
    pub fuel: bool,
    /// Brakes, tyres
    pub car: bool,
    /// The pit lane: entry speed, limiter, the stop's countdown, traffic or clear at the pit exit
    pub pit: bool,
}

impl Default for Engineer {
    fn default() -> Self {
        Self { enabled: true, laps: true, fuel: true, car: true, pit: true }
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
    /// Off until the driver switches the coach on; off while the coach is not available ([`COACH_AVAILABLE`])
    pub enabled: bool,
    /// Live tips: after a corner the coach says where it cost time against the driver's best lap. Off until the
    /// driver switches it on.
    pub live_tips: bool,
    pub button: Option<Button>,
    /// The input device's id as the sound system names it; None for the PC's default microphone
    pub microphone: Option<String>,
    /// The coach sums up a stint when the car is back in its box; off until the driver switches it on
    pub stints: bool,
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
    pub spotter: Spotter,
    pub engineer: Engineer,
    pub coach: Coach,
}

impl Default for Settings {
    fn default() -> Self {
        Self { enabled: false, voice: VOICES[0].to_string(), language: LANGUAGES[0].to_string(), volume: 80, effect: 100, device: None, spotter: Spotter::default(), engineer: Engineer::default(), coach: Coach::default() }
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
        if !COACH_AVAILABLE {
            self.coach.enabled = false;
            self.coach.live_tips = false;
        }
        self
    }
}

/// Whether the driver wants to hear a spotter's call. The pit exit's traffic or clear belongs to the engineer's pit
/// lane.
pub fn wanted_spotter(settings: &Settings, call: &Call) -> bool {
    match (call.key, call.priority) {
        ("pit_exit_traffic" | "pit_exit_clear", _) => wanted_engineer(settings, Group::Pit),
        (_, Priority::Spotter) => settings.spotter.enabled,
        (_, Priority::Safety | Priority::Tactical) => settings.spotter.enabled && settings.spotter.traffic,
        _ => true,
    }
}

/// Whether the driver wants to hear the engineer's calls of a group.
pub fn wanted_engineer(settings: &Settings, group: Group) -> bool {
    let engineer = &settings.engineer;
    engineer.enabled
        && match group {
            Group::Laps => engineer.laps,
            Group::Fuel => engineer.fuel,
            Group::Car => engineer.car,
            Group::Pit => engineer.pit,
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
    use crate::radio::queue;
    use serde_json::json;

    fn dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dd-radio-settings-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_radio_is_off_with_the_spotter_and_the_engineer_on_until_the_driver_chooses() {
        let dir = dir("defaults");
        let settings = load(&dir);
        assert_eq!(settings, Settings::default());
        assert!(!settings.enabled);
        assert_eq!((settings.voice.as_str(), settings.language.as_str(), settings.volume, settings.effect), ("M1", "de", 80, 100));
        assert_eq!(settings.spotter, Spotter { enabled: true, traffic: true });
        assert_eq!(settings.engineer, Engineer { enabled: true, laps: true, fuel: true, car: true, pit: true });
        assert_eq!(settings.coach, Coach::default());
        // a file of a version before the three blocks still loads: its call groups and live tips are left out, the
        // blocks take their defaults
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE), br#"{ "enabled": true, "voice": "F2", "groups": { "spotter": false, "fuel": false }, "liveTips": true }"#).unwrap();
        assert_eq!(load(&dir), Settings { enabled: true, voice: "F2".into(), ..Settings::default() });
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_change_touches_only_what_it_names_and_stays() {
        let dir = dir("change");
        let settings = change(&dir, &json!({ "enabled": true, "voice": "F3", "engineer": { "fuel": false } })).unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.voice, "F3");
        assert_eq!(settings.engineer, Engineer { fuel: false, ..Engineer::default() });
        assert_eq!(settings.spotter, Spotter::default());
        let settings = change(&dir, &json!({ "volume": 55, "device": "wasapi:{0.0.0.00000000}.{abc}", "spotter": { "traffic": false } })).unwrap();
        assert_eq!((settings.enabled, settings.voice.as_str(), settings.volume), (true, "F3", 55));
        assert_eq!(settings.device.as_deref(), Some("wasapi:{0.0.0.00000000}.{abc}"));
        assert!(!settings.engineer.fuel);
        assert_eq!(settings.spotter, Spotter { enabled: true, traffic: false });
        assert_eq!(load(&dir), settings);
        assert_eq!(change(&dir, &json!({ "device": null })).unwrap().device, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_coach_stays_off_while_it_is_not_available() {
        let dir = dir("coach-available");
        let settings = change(&dir, &json!({ "coach": { "enabled": true, "liveTips": true, "stints": true, "microphone": "wasapi:{0.0.1.00000000}.{mic}" } })).unwrap();
        assert_eq!((settings.coach.enabled, settings.coach.live_tips), (COACH_AVAILABLE, COACH_AVAILABLE));
        assert!(settings.coach.stints && settings.coach.microphone.is_some(), "the rest of the coach is kept");
        // a file that has the coach on
        fs::write(dir.join(FILE), br#"{ "coach": { "enabled": true, "liveTips": true } }"#).unwrap();
        let coach = load(&dir).coach;
        assert_eq!((coach.enabled, coach.live_tips), (COACH_AVAILABLE, COACH_AVAILABLE));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_block_and_group_silences_its_own_calls() {
        use crate::radio::spotter::Event;
        let car = (1, 2);
        let calls = [
            Event::CarLeft { car },
            Event::ClearAll,
            Event::SlowCarAhead { car },
            Event::InTow { car },
            Event::PitExitTraffic { car },
            Event::PitExitClear,
        ]
        .map(|event| queue::call(&event, 0.0));
        let heard = |settings: &Settings| calls.iter().filter(|call| wanted_spotter(settings, call)).map(|call| call.key).collect::<Vec<_>>();
        let on = Settings::default();
        assert_eq!(heard(&on), ["car_left", "clear", "slow_car_ahead", "in_tow", "pit_exit_traffic", "pit_exit_clear"]);
        let traffic_off = Settings { spotter: Spotter { traffic: false, ..Spotter::default() }, ..Settings::default() };
        assert_eq!(heard(&traffic_off), ["car_left", "clear", "pit_exit_traffic", "pit_exit_clear"]);
        let spotter_off = Settings { spotter: Spotter { enabled: false, traffic: true }, ..Settings::default() };
        assert_eq!(heard(&spotter_off), ["pit_exit_traffic", "pit_exit_clear"], "the pit exit is the engineer's");
        let pit_off = Settings { engineer: Engineer { pit: false, ..Engineer::default() }, ..Settings::default() };
        assert_eq!(heard(&pit_off), ["car_left", "clear", "slow_car_ahead", "in_tow"]);
        let engineer_off = Settings { engineer: Engineer { enabled: false, ..Engineer::default() }, ..Settings::default() };
        assert_eq!(heard(&engineer_off), ["car_left", "clear", "slow_car_ahead", "in_tow"]);

        let groups = [Group::Laps, Group::Fuel, Group::Car, Group::Pit];
        let wanted = |settings: &Settings| groups.map(|group| wanted_engineer(settings, group));
        assert_eq!(wanted(&on), [true; 4]);
        assert_eq!(wanted(&engineer_off), [false; 4]);
        assert_eq!(wanted(&pit_off), [true, true, true, false]);
        let some_off = Settings { engineer: Engineer { laps: false, car: false, ..Engineer::default() }, ..Settings::default() };
        assert_eq!(wanted(&some_off), [false, true, false, true]);
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
        assert_eq!(load(&dir).coach, Coach { enabled: false, live_tips: false, button: None, microphone: None, stints: false });
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
        assert!(settings.coach.enabled == COACH_AVAILABLE && settings.coach.microphone.is_some(), "the rest of the coach stays");
        assert_eq!(load(&dir), settings);
        assert_eq!(change(&dir, &json!({ "coach": { "button": null, "microphone": null } })).unwrap().coach, Coach { enabled: COACH_AVAILABLE, live_tips: false, button: None, microphone: None, stints: false });
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
