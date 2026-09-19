//! Races against the game's own AI, offline on the driver's PC: a way to pass the time while a grid fills.
//! The hosted interface hands over a ticket, the app writes the `race.ini` of a single-player race and starts
//! the game; when the game has closed, the app reads the game's result file and reports it. The result comes
//! from the driver's PC, so the platform treats it as fun: XP and a best list, never skill or the licence.

use serde::{Deserialize, Serialize};

use crate::join::{ini_value, is_content_name, JoinError};

/// What the hosted interface hands over to start a race against bots.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotRaceTicket {
    pub track: String,
    /// Empty for a track without layouts
    pub track_layout: String,
    /// The driver's car; the bots drive the same model
    pub car_model: String,
    pub driver_name: String,
    pub laps: u32,
    pub opponents: u32,
    /// Strength of the bots in percent, as in the game's own menu
    pub ai_level: u32,
}

impl BotRaceTicket {
    pub fn validate(&self) -> Result<(), JoinError> {
        if !is_content_name(&self.track) {
            return Err(JoinError::InvalidTicket("track"));
        }
        if !self.track_layout.is_empty() && !is_content_name(&self.track_layout) {
            return Err(JoinError::InvalidTicket("trackLayout"));
        }
        if !is_content_name(&self.car_model) {
            return Err(JoinError::InvalidTicket("carModel"));
        }
        if one_line(&self.driver_name).is_empty() {
            return Err(JoinError::InvalidTicket("driverName"));
        }
        if !(1..=30).contains(&self.laps) {
            return Err(JoinError::InvalidTicket("laps"));
        }
        if !(1..=23).contains(&self.opponents) {
            return Err(JoinError::InvalidTicket("opponents"));
        }
        if !(70..=100).contains(&self.ai_level) {
            return Err(JoinError::InvalidTicket("aiLevel"));
        }
        Ok(())
    }
}

fn one_line(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).take(100).collect::<String>().trim().to_string()
}

/// The `race.ini` of a single-player race: the driver starts last behind the bots, who all drive the driver's
/// car model in the liveries given (used in turn). `previous` is the file as it was: the driver's unit of
/// speed and nationality are kept.
pub fn render_bot_race_ini(ticket: &BotRaceTicket, skins: &[String], previous: &str) -> String {
    let kept = |section: &str, key: &str, default: &str| one_line(ini_value(previous, section, key).unwrap_or(default));
    let skin = |index: usize| if skins.is_empty() { String::new() } else { skins[index % skins.len()].clone() };
    let cars = ticket.opponents + 1;
    let mut lines = vec![
        "[HEADER]".to_string(), "VERSION=2".to_string(), String::new(),
        "[RACE]".to_string(),
        format!("TRACK={}", ticket.track), format!("CONFIG_TRACK={}", ticket.track_layout),
        format!("MODEL={}", ticket.car_model), "MODEL_CONFIG=".to_string(), format!("SKIN={}", skin(0)),
        format!("CARS={cars}"), format!("AI_LEVEL={}", ticket.ai_level), format!("RACE_LAPS={}", ticket.laps),
        "FIXED_SETUP=0".to_string(), "PENALTIES=1".to_string(), "JUMP_START_PENALTY=1".to_string(), "DRIFT_MODE=0".to_string(), String::new(),
        "[SESSION_0]".to_string(), "NAME=Race".to_string(), "TYPE=3".to_string(), format!("LAPS={}", ticket.laps),
        "DURATION_MINUTES=0".to_string(), "SPAWN_SET=START".to_string(), format!("STARTING_POSITION={cars}"), String::new(),
        "[CAR_0]".to_string(), format!("MODEL={}", ticket.car_model), "MODEL_CONFIG=".to_string(), format!("SKIN={}", skin(0)),
        "SETUP=".to_string(), "BALLAST=0".to_string(), "RESTRICTOR=0".to_string(), format!("DRIVER_NAME={}", one_line(&ticket.driver_name)),
        format!("NATIONALITY={}", kept("CAR_0", "NATIONALITY", "")), format!("NATION_CODE={}", kept("CAR_0", "NATION_CODE", "")), String::new(),
    ];
    for bot in 1..=ticket.opponents as usize {
        lines.extend([
            format!("[CAR_{bot}]"), format!("MODEL={}", ticket.car_model), "MODEL_CONFIG=".to_string(), format!("SKIN={}", skin(bot)),
            "SETUP=".to_string(), "BALLAST=0".to_string(), "RESTRICTOR=0".to_string(), format!("DRIVER_NAME=Bot {bot}"),
            "NATIONALITY=".to_string(), "NATION_CODE=".to_string(), format!("AI_LEVEL={}", ticket.ai_level), "AI_AGGRESSION=40".to_string(), String::new(),
        ]);
    }
    lines.extend([
        "[REMOTE]".to_string(), "ACTIVE=0".to_string(), String::new(),
        "[REPLAY]".to_string(), "ACTIVE=0".to_string(), String::new(),
        "[BENCHMARK]".to_string(), "ACTIVE=0".to_string(), String::new(),
        "[RESTART]".to_string(), "ACTIVE=0".to_string(), String::new(),
        "[GHOST_CAR]".to_string(), "ENABLED=0".to_string(), "FILE=".to_string(), "LOAD=0".to_string(), "PLAYING=0".to_string(), "RECORDING=0".to_string(), "SECONDS_ADVANTAGE=0".to_string(), String::new(),
        "[LAP_INVALIDATOR]".to_string(), "ALLOWED_TYRES_OUT=-1".to_string(), String::new(),
        "[OPTIONS]".to_string(), format!("USE_MPH={}", if kept("OPTIONS", "USE_MPH", "0") == "1" { "1" } else { "0" }), String::new(),
        "[GROOVE]".to_string(), "VIRTUAL_LAPS=10".to_string(), "MAX_LAPS=30".to_string(), "STARTING_LAPS=0".to_string(), String::new(),
        "[DYNAMIC_TRACK]".to_string(), "SESSION_START=100".to_string(), "SESSION_TRANSFER=100".to_string(), "RANDOMNESS=0".to_string(), "LAP_GAIN=1".to_string(), String::new(),
        "[TEMPERATURE]".to_string(), "AMBIENT=22".to_string(), "ROAD=30".to_string(), String::new(),
        "[WEATHER]".to_string(), "NAME=3_clear".to_string(), String::new(),
        "[LIGHTING]".to_string(), "SUN_ANGLE=16".to_string(), "TIME_MULT=1".to_string(), "CLOUD_SPEED=0.2".to_string(), String::new(),
        "[WIND]".to_string(), "SPEED_KMH_MIN=0".to_string(), "SPEED_KMH_MAX=0".to_string(), "DIRECTION_DEG=0".to_string(), String::new(),
    ]);
    lines.join("\r\n")
}

/// How the driver's race against the bots went, read from the game's result file.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotRaceResult {
    /// The driver's place among all cars; the driver is always car 0
    pub position: u32,
    pub field_size: u32,
    pub laps_done: u32,
    pub total_time_ms: u64,
    /// Best lap without a cut, if there was one
    pub best_lap_ms: Option<u64>,
    pub cuts: u32,
}

#[derive(Deserialize)]
struct RaceOut {
    #[serde(default)]
    players: Vec<serde_json::Value>,
    #[serde(default)]
    sessions: Vec<RaceOutSession>,
}

#[derive(Deserialize)]
struct RaceOutSession {
    #[serde(default, rename = "type")]
    kind: i64,
    #[serde(default)]
    laps: Vec<RaceOutLap>,
    #[serde(default, rename = "raceResult")]
    race_result: Vec<i64>,
}

#[derive(Deserialize)]
struct RaceOutLap {
    car: i64,
    time: i64,
    #[serde(default)]
    cuts: i64,
}

/// Reads `Documents/Assetto Corsa/out/race_out.json` as the game writes it when it closes. None when the file
/// holds no race (the game was closed before the start, or it is the result of something else).
pub fn parse_race_out(json: &str) -> Option<BotRaceResult> {
    let out: RaceOut = serde_json::from_str(json).ok()?;
    let race = out.sessions.iter().rev().find(|s| s.kind == 3)?;
    let place = race.race_result.iter().position(|car| *car == 0)?;
    let mine: Vec<&RaceOutLap> = race.laps.iter().filter(|lap| lap.car == 0 && lap.time > 0).collect();
    Some(BotRaceResult {
        position: place as u32 + 1,
        field_size: out.players.len().max(race.race_result.len()) as u32,
        laps_done: mine.len() as u32,
        total_time_ms: mine.iter().map(|lap| lap.time as u64).sum(),
        best_lap_ms: mine.iter().filter(|lap| lap.cuts == 0).map(|lap| lap.time as u64).min(),
        cuts: mine.iter().map(|lap| lap.cuts.max(0) as u32).sum(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket() -> BotRaceTicket {
        BotRaceTicket {
            track: "ks_nurburgring".to_string(), track_layout: "layout_gp_a".to_string(), car_model: "ks_porsche_911_gt3_cup_2017".to_string(),
            driver_name: "Anna Example".to_string(), laps: 5, opponents: 3, ai_level: 90,
        }
    }

    #[test]
    fn accepts_a_ticket_of_the_platform_and_refuses_what_could_break_the_ini_file() {
        assert_eq!(ticket().validate(), Ok(()));
        let sent = r#"{"track":"ks_nurburgring","trackLayout":"","carModel":"abarth500","driverName":"Anna","laps":3,"opponents":11,"aiLevel":95}"#;
        assert_eq!(serde_json::from_str::<BotRaceTicket>(sent).unwrap().validate(), Ok(()));
        for (bad, field) in [
            (BotRaceTicket { track: "..".to_string(), ..ticket() }, "track"),
            (BotRaceTicket { car_model: "car\r\n[REMOTE]".to_string(), ..ticket() }, "carModel"),
            (BotRaceTicket { driver_name: "\r\n".to_string(), ..ticket() }, "driverName"),
            (BotRaceTicket { laps: 0, ..ticket() }, "laps"),
            (BotRaceTicket { laps: 31, ..ticket() }, "laps"),
            (BotRaceTicket { opponents: 0, ..ticket() }, "opponents"),
            (BotRaceTicket { opponents: 24, ..ticket() }, "opponents"),
            (BotRaceTicket { ai_level: 69, ..ticket() }, "aiLevel"),
            (BotRaceTicket { ai_level: 101, ..ticket() }, "aiLevel"),
        ] {
            assert_eq!(bad.validate(), Err(JoinError::InvalidTicket(field)));
        }
    }

    #[test]
    fn writes_a_single_player_race_with_the_driver_last_on_the_grid_behind_the_bots() {
        let skins = ["0_cup_a".to_string(), "0_cup_b".to_string()];
        let ini = render_bot_race_ini(&ticket(), &skins, "[OPTIONS]\r\nUSE_MPH=1\r\n[REMOTE]\r\nACTIVE=1\r\nSERVER_IP=203.0.113.7\r\n");
        assert_eq!(ini_value(&ini, "RACE", "CARS"), Some("4"));
        assert_eq!(ini_value(&ini, "RACE", "RACE_LAPS"), Some("5"));
        assert_eq!(ini_value(&ini, "RACE", "AI_LEVEL"), Some("90"));
        assert_eq!(ini_value(&ini, "SESSION_0", "TYPE"), Some("3"));
        assert_eq!(ini_value(&ini, "SESSION_0", "STARTING_POSITION"), Some("4"));
        assert_eq!(ini_value(&ini, "CAR_0", "DRIVER_NAME"), Some("Anna Example"));
        assert_eq!(ini_value(&ini, "CAR_0", "SKIN"), Some("0_cup_a"));
        assert_eq!(ini_value(&ini, "CAR_1", "SKIN"), Some("0_cup_b"));
        assert_eq!(ini_value(&ini, "CAR_2", "SKIN"), Some("0_cup_a"));
        assert_eq!(ini_value(&ini, "CAR_3", "DRIVER_NAME"), Some("Bot 3"));
        assert_eq!(ini_value(&ini, "CAR_3", "AI_LEVEL"), Some("90"));
        assert_eq!(ini_value(&ini, "CAR_4", "MODEL"), None);
        // Offline: whatever server the last session was on is gone. The driver's unit of speed stays.
        assert_eq!(ini_value(&ini, "REMOTE", "ACTIVE"), Some("0"));
        assert!(!ini.contains("203.0.113.7"));
        assert_eq!(ini_value(&ini, "OPTIONS", "USE_MPH"), Some("1"));
        // A car without liveries on disk still gets a file the game can read.
        assert_eq!(ini_value(&render_bot_race_ini(&ticket(), &[], ""), "CAR_0", "SKIN"), Some(""));
    }

    // The shape of Documents/Assetto Corsa/out/race_out.json after a single-player race: the driver is car 0,
    // `raceResult` lists the cars in finishing order.
    const RACE_OUT: &str = r#"{"track":"ks_nurburgring","number_of_sessions":1,
        "players":[{"name":"Anna Example","car":"ks_porsche_911_gt3_cup_2017","skin":"0_cup_a"},{"name":"Bot 1","car":"ks_porsche_911_gt3_cup_2017","skin":"0_cup_b"},{"name":"Bot 2","car":"ks_porsche_911_gt3_cup_2017","skin":"0_cup_a"}],
        "sessions":[{"event":0,"name":"Race","type":3,"lapsCount":3,"duration":0,
          "laps":[{"lap":0,"car":1,"sectors":[1,2,3],"time":121000,"cuts":0,"tyre":"SM"},{"lap":0,"car":0,"sectors":[1,2,3],"time":123400,"cuts":1,"tyre":"SM"},
                  {"lap":1,"car":0,"sectors":[1,2,3],"time":119800,"cuts":0,"tyre":"SM"},{"lap":2,"car":0,"sectors":[1,2,3],"time":120100,"cuts":0,"tyre":"SM"},
                  {"lap":1,"car":1,"sectors":[1,2,3],"time":120500,"cuts":0,"tyre":"SM"},{"lap":2,"car":1,"sectors":[1,2,3],"time":122900,"cuts":0,"tyre":"SM"}],
          "lapstotal":[3,3,2],"bestLaps":[{"car":0,"time":119800,"lap":1}],"raceResult":[0,1,2]}],
        "extras":[]}"#;

    #[test]
    fn reads_the_drivers_race_from_the_games_result_file() {
        assert_eq!(
            parse_race_out(RACE_OUT),
            Some(BotRaceResult { position: 1, field_size: 3, laps_done: 3, total_time_ms: 363_300, best_lap_ms: Some(119_800), cuts: 1 })
        );
        let second = RACE_OUT.replace("\"raceResult\":[0,1,2]", "\"raceResult\":[1,0,2]");
        assert_eq!(parse_race_out(&second).unwrap().position, 2);
    }

    #[test]
    fn has_no_result_without_a_race_in_the_file() {
        // The file of an online session: players, no sessions.
        assert_eq!(parse_race_out(r#"{"track":"ks_nordschleife","number_of_sessions":0,"players":[{"name":"x","car":"y","skin":"z"}],"sessions":[],"extras":[]}"#), None);
        assert_eq!(parse_race_out(&RACE_OUT.replace("\"type\":3", "\"type\":1")), None);
        assert_eq!(parse_race_out("not json"), None);
    }
}
