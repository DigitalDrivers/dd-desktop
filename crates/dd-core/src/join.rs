//! Joining a race server with one click: what the hosted interface asks for (the ticket), what the race
//! server says about the driver's slot, and the `race.ini` that makes the game connect on start.
//!
//! The game reads `Documents/Assetto Corsa/cfg/race.ini` when it starts; every launcher rewrites that file
//! for every session, and so does this one.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde::Deserialize;

/// Steam's app id of Assetto Corsa, the content of `steam_appid.txt`. With that file in the game folder the
/// game can be started directly; without it Steam starts the game's own launcher instead.
pub const STEAM_APP_ID: &str = "244210";

/// SteamID64 of the first individual account; an account's id is added to it.
const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;

/// Why a join did not happen. The code goes to the hosted interface, which has the words for it.
#[derive(Debug, PartialEq, Eq)]
pub enum JoinError {
    /// A value of the ticket is not what it should be; names the field.
    InvalidTicket(&'static str),
    AssettoCorsaNotFound,
    /// Steam is not running or nobody is signed in: the race server could not check who is driving.
    SteamNotRunning,
    /// Steam on this PC is signed in with another account than the platform.
    WrongSteamAccount,
    /// The race server did not answer.
    ServerUnreachable,
    /// The race server has no free slot with this car for this driver.
    NoOpenSlot,
    /// The car or the track is not installed; names the folder.
    ContentMissing(String),
    /// A file could not be written or the game could not be started.
    Failed(String),
}

impl JoinError {
    pub fn code(&self) -> String {
        match self {
            JoinError::InvalidTicket(field) => format!("invalid-ticket:{field}"),
            JoinError::AssettoCorsaNotFound => "assetto-corsa-not-found".to_string(),
            JoinError::SteamNotRunning => "steam-not-running".to_string(),
            JoinError::WrongSteamAccount => "wrong-steam-account".to_string(),
            JoinError::ServerUnreachable => "server-unreachable".to_string(),
            JoinError::NoOpenSlot => "no-open-slot".to_string(),
            JoinError::ContentMissing(folder) => format!("content-missing:{folder}"),
            JoinError::Failed(_) => "failed".to_string(),
        }
    }
}

/// What the hosted interface hands over to join a race server.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinTicket {
    pub host: String,
    pub game_port: u16,
    pub http_port: u16,
    pub server_name: String,
    pub track: String,
    /// Empty for a track without layouts
    pub track_layout: String,
    pub car_model: String,
    pub driver_name: String,
    /// The SteamID64 the driver is signed in with on the platform: the slot is reserved for it.
    pub steam_id: String,
}

/// Folder names of the game's content, and nothing that could leave the content folder or break the ini file.
pub(crate) fn is_content_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Free text for the ini file: one line, no control characters, cut to a sane length.
fn ini_text(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).take(100).collect::<String>().trim().to_string()
}

impl JoinTicket {
    pub fn validate(&self) -> Result<(), JoinError> {
        let host_ok = !self.host.is_empty()
            && self.host.len() <= 253
            && self.host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'));
        if !host_ok {
            return Err(JoinError::InvalidTicket("host"));
        }
        if self.game_port == 0 {
            return Err(JoinError::InvalidTicket("gamePort"));
        }
        if self.http_port == 0 {
            return Err(JoinError::InvalidTicket("httpPort"));
        }
        if !is_content_name(&self.track) {
            return Err(JoinError::InvalidTicket("track"));
        }
        if !self.track_layout.is_empty() && !is_content_name(&self.track_layout) {
            return Err(JoinError::InvalidTicket("trackLayout"));
        }
        if !is_content_name(&self.car_model) {
            return Err(JoinError::InvalidTicket("carModel"));
        }
        if self.steam_id.len() != 17 || !self.steam_id.chars().all(|c| c.is_ascii_digit()) {
            return Err(JoinError::InvalidTicket("steamId"));
        }
        if ini_text(&self.driver_name).is_empty() {
            return Err(JoinError::InvalidTicket("driverName"));
        }
        Ok(())
    }
}

/// SteamID64 of the account Steam reports as active (`ActiveProcess\ActiveUser` in the registry);
/// 0 there means Steam is not running or nobody is signed in.
pub fn steam_id64(active_user: u32) -> Option<String> {
    (active_user != 0).then(|| (STEAM_ID64_BASE + u64::from(active_user)).to_string())
}

/// The driver's slot as the race server describes it.
#[derive(Debug, PartialEq, Eq)]
pub struct Slot {
    pub skin: String,
    /// The Custom Shaders Patch features the server announces; the game has to be told them to be let in.
    pub features: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EntryList {
    cars: Vec<EntryListCar>,
    #[serde(default)]
    features: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EntryListCar {
    model: String,
    skin: String,
    /// Whether the slot is open to the driver who asked
    is_entry_list: bool,
    is_connected: bool,
}

/// Reads the answer of the race server's `/JSON|<SteamID>` and picks the free slot with the ticket's car.
pub fn slot_of(entry_list_json: &str, car_model: &str) -> Result<Slot, JoinError> {
    let list: EntryList = serde_json::from_str(entry_list_json).map_err(|_| JoinError::ServerUnreachable)?;
    let car = list
        .cars
        .iter()
        .find(|car| car.model == car_model && car.is_entry_list && !car.is_connected)
        .ok_or(JoinError::NoOpenSlot)?;
    // Both end up in the ini file: they have to be plain names.
    if !is_content_name(&car.skin) || !list.features.iter().all(|f| is_content_name(f)) {
        return Err(JoinError::ServerUnreachable);
    }
    Ok(Slot { skin: car.skin.clone(), features: list.features })
}

#[derive(Deserialize)]
struct ServerInfo {
    track: String,
}

/// The track name as the race server's `/INFO` gives it, the way the game has to be told it. A server that
/// asks for a minimum Custom Shaders Patch build names its track `csp/<build>/../<track>` (the patch reads the
/// build and loads the track, the game without it finds no such folder), and lets the game in only with
/// exactly that name. The layout, joined with a dash in `/INFO`, stays out: it goes into `CONFIG_TRACK`.
pub fn server_track(info_json: &str, track: &str, layout: &str) -> Result<String, JoinError> {
    let info: ServerInfo = serde_json::from_str(info_json).map_err(|_| JoinError::ServerUnreachable)?;
    let name = if layout.is_empty() { info.track.as_str() } else { info.track.strip_suffix(&format!("-{layout}")).unwrap_or(&info.track) };
    let prefix = name.strip_suffix(track).ok_or(JoinError::ServerUnreachable)?;
    let gate = prefix.strip_prefix("csp/").and_then(|rest| rest.strip_suffix("/../"));
    match gate {
        None if prefix.is_empty() => Ok(track.to_string()),
        Some(build) if !build.is_empty() && build.chars().all(|c| c.is_ascii_digit()) => Ok(format!("{prefix}{track}")),
        _ => Err(JoinError::ServerUnreachable),
    }
}

/// Asks the race server what it runs (`/INFO`): the track name it expects from the game.
pub fn fetch_info(host: &str, http_port: u16, timeout: Duration) -> Result<String, JoinError> {
    fetch(host, http_port, "/INFO", timeout)
}

/// Asks the race server for its entry list as the given driver sees it.
pub fn fetch_entry_list(host: &str, http_port: u16, steam_id: &str, timeout: Duration) -> Result<String, JoinError> {
    fetch(host, http_port, &format!("/JSON%7C{steam_id}"), timeout)
}

/// One request to the race server's HTTP port. Plain HTTP/1.0 over a socket: the answer comes in one piece and
/// the connection closes, which is all this needs.
fn fetch(host: &str, http_port: u16, path: &str, timeout: Duration) -> Result<String, JoinError> {
    let address = (host, http_port).to_socket_addrs().ok().and_then(|mut found| found.next()).ok_or(JoinError::ServerUnreachable)?;
    let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(|_| JoinError::ServerUnreachable)?;
    stream.set_read_timeout(Some(timeout)).and_then(|_| stream.set_write_timeout(Some(timeout))).map_err(|_| JoinError::ServerUnreachable)?;
    let request = format!("GET {path} HTTP/1.0\r\nHost: {host}:{http_port}\r\nAccept: application/json\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|_| JoinError::ServerUnreachable)?;
    let mut response = Vec::new();
    stream.take(1_000_000).read_to_end(&mut response).map_err(|_| JoinError::ServerUnreachable)?;

    let response = String::from_utf8_lossy(&response);
    let (head, body) = response.split_once("\r\n\r\n").ok_or(JoinError::ServerUnreachable)?;
    let status_ok = head.lines().next().is_some_and(|line| line.split(' ').nth(1) == Some("200"));
    if !status_ok {
        return Err(JoinError::ServerUnreachable);
    }
    Ok(body.to_string())
}

/// Value of a key in a section of an ini file, if it is there.
pub fn ini_value<'a>(ini: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut in_section = false;
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) == Some(section);
        } else if in_section {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim());
                }
            }
        }
    }
    None
}

/// The `race.ini` of an online session on the ticket's server. `track` is the track name the server expects
/// (`server_track`). `previous` is the file as it was: the driver's unit of speed and nationality are kept,
/// everything else belongs to the session and is written anew.
pub fn render_race_ini(ticket: &JoinTicket, slot: &Slot, track: &str, previous: &str) -> String {
    let kept = |section: &str, key: &str, default: &str| ini_text(ini_value(previous, section, key).unwrap_or(default));
    let use_mph = if kept("OPTIONS", "USE_MPH", "0") == "1" { "1" } else { "0" };
    let lines = [
        "[BENCHMARK]".to_string(),
        "ACTIVE=0".to_string(),
        String::new(),
        "[CAR_0]".to_string(),
        "SETUP=".to_string(),
        format!("SKIN={}", slot.skin),
        "MODEL=-".to_string(),
        "MODEL_CONFIG=".to_string(),
        "BALLAST=0".to_string(),
        "RESTRICTOR=0".to_string(),
        format!("DRIVER_NAME={}", ini_text(&ticket.driver_name)),
        format!("NATIONALITY={}", kept("CAR_0", "NATIONALITY", "")),
        format!("NATION_CODE={}", kept("CAR_0", "NATION_CODE", "")),
        String::new(),
        "[GHOST_CAR]".to_string(),
        "ENABLED=0".to_string(),
        "FILE=".to_string(),
        "LOAD=0".to_string(),
        "PLAYING=0".to_string(),
        "RECORDING=0".to_string(),
        "SECONDS_ADVANTAGE=0".to_string(),
        String::new(),
        "[HEADER]".to_string(),
        "VERSION=2".to_string(),
        String::new(),
        "[LAP_INVALIDATOR]".to_string(),
        "ALLOWED_TYRES_OUT=-1".to_string(),
        String::new(),
        "[OPTIONS]".to_string(),
        format!("USE_MPH={use_mph}"),
        String::new(),
        "[RACE]".to_string(),
        "AI_LEVEL=100".to_string(),
        "CARS=1".to_string(),
        format!("CONFIG_TRACK={}", ticket.track_layout),
        "DRIFT_MODE=0".to_string(),
        "FIXED_SETUP=0".to_string(),
        "JUMP_START_PENALTY=0".to_string(),
        format!("MODEL={}", ticket.car_model),
        "MODEL_CONFIG=".to_string(),
        "PENALTIES=1".to_string(),
        "RACE_LAPS=0".to_string(),
        format!("SKIN={}", slot.skin),
        format!("TRACK={track}"),
        String::new(),
        "[REMOTE]".to_string(),
        "ACTIVE=1".to_string(),
        format!("GUID={}", ticket.steam_id),
        format!("NAME={}", ini_text(&ticket.driver_name)),
        "PASSWORD=".to_string(),
        format!("REQUESTED_CAR={}", ticket.car_model),
        format!("SERVER_HTTP_PORT={}", ticket.http_port),
        format!("SERVER_IP={}", ticket.host),
        format!("SERVER_NAME={}", ini_text(&ticket.server_name)),
        format!("SERVER_PORT={}", ticket.game_port),
        "TEAM=".to_string(),
        format!("__FEATURES={}", slot.features.join(",")),
        String::new(),
        "[REPLAY]".to_string(),
        "ACTIVE=0".to_string(),
        String::new(),
        "[RESTART]".to_string(),
        "ACTIVE=0".to_string(),
        String::new(),
    ];
    lines.join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn ticket() -> JoinTicket {
        JoinTicket {
            host: "race.digitaldrivers.club".to_string(),
            game_port: 9610,
            http_port: 8110,
            server_name: "Digital Drivers | Friday Cup".to_string(),
            track: "ks_nurburgring".to_string(),
            track_layout: "layout_gp_a".to_string(),
            car_model: "ks_porsche_911_gt3_cup_2017".to_string(),
            driver_name: "Anna Example".to_string(),
            steam_id: "76561198000000001".to_string(),
        }
    }

    // The shape AssettoServer answers `/JSON|<SteamID>` with: every car of the entry list, `IsEntryList`
    // true for the slots open to the driver who asked.
    const ENTRY_LIST: &str = r#"{"Cars":[
        {"Model":"ks_porsche_911_gt3_cup_2017","Skin":"0_cup_a","IsEntryList":false,"DriverName":"Ben","DriverTeam":"","IsConnected":true},
        {"Model":"ks_porsche_911_gt3_cup_2017","Skin":"0_cup_b","IsEntryList":true,"DriverName":null,"DriverTeam":null,"IsConnected":false},
        {"Model":"mercedes_sls","Skin":"Argento_Iridio","IsEntryList":true,"DriverName":null,"DriverTeam":null,"IsConnected":false}],
        "Features":["SPECTATING_AWARE","CLIENT_MESSAGES","WEATHERFX_V1","STEAM_TICKET"]}"#;

    #[test]
    fn accepts_a_ticket_of_the_platform() {
        assert_eq!(ticket().validate(), Ok(()));
        assert_eq!(JoinTicket { track_layout: String::new(), ..ticket() }.validate(), Ok(()));
        assert_eq!(JoinTicket { host: "203.0.113.7".to_string(), ..ticket() }.validate(), Ok(()));
    }

    #[test]
    fn refuses_values_that_could_break_the_ini_file_or_leave_the_content_folder() {
        let cases = [
            (JoinTicket { host: "race.example.org\r\nPASSWORD=x".to_string(), ..ticket() }, "host"),
            (JoinTicket { host: String::new(), ..ticket() }, "host"),
            (JoinTicket { game_port: 0, ..ticket() }, "gamePort"),
            (JoinTicket { http_port: 0, ..ticket() }, "httpPort"),
            (JoinTicket { track: "..".to_string(), ..ticket() }, "track"),
            (JoinTicket { track_layout: "gp\\..\\..".to_string(), ..ticket() }, "trackLayout"),
            (JoinTicket { car_model: "car\n[REMOTE]".to_string(), ..ticket() }, "carModel"),
            (JoinTicket { car_model: String::new(), ..ticket() }, "carModel"),
            (JoinTicket { steam_id: "7656119800000000".to_string(), ..ticket() }, "steamId"),
            (JoinTicket { steam_id: "7656119800000000x".to_string(), ..ticket() }, "steamId"),
            (JoinTicket { driver_name: " \r\n ".to_string(), ..ticket() }, "driverName"),
        ];
        for (ticket, field) in cases {
            assert_eq!(ticket.validate(), Err(JoinError::InvalidTicket(field)));
        }
    }

    #[test]
    fn reads_a_ticket_the_way_the_page_sends_it() {
        let sent = r#"{"host":"127.0.0.1","gamePort":9610,"httpPort":8110,"serverName":"Digital Drivers | Check","track":"ks_nurburgring",
            "trackLayout":"layout_gp_a","carModel":"mercedes_sls","driverName":"Anna","steamId":"76561198000000001"}"#;
        let ticket: JoinTicket = serde_json::from_str(sent).unwrap();
        assert_eq!((ticket.game_port, ticket.car_model.as_str()), (9610, "mercedes_sls"));
        assert!(serde_json::from_str::<JoinTicket>(r#"{"host":"127.0.0.1","gamePort":99999}"#).is_err());
    }

    #[test]
    fn works_out_the_steam_id_of_the_active_account() {
        assert_eq!(steam_id64(4_782_979), Some("76561197965048707".to_string()));
        assert_eq!(steam_id64(0), None);
    }

    #[test]
    fn picks_the_free_slot_with_the_drivers_car() {
        let slot = slot_of(ENTRY_LIST, "ks_porsche_911_gt3_cup_2017").unwrap();
        assert_eq!(slot.skin, "0_cup_b");
        assert_eq!(slot.features, ["SPECTATING_AWARE", "CLIENT_MESSAGES", "WEATHERFX_V1", "STEAM_TICKET"]);
        assert_eq!(slot_of(ENTRY_LIST, "mercedes_sls").unwrap().skin, "Argento_Iridio");
        // Not entered with this car, or the slot is reserved for somebody else.
        assert_eq!(slot_of(ENTRY_LIST, "ks_porsche_911_gt3_r_2016"), Err(JoinError::NoOpenSlot));
        assert_eq!(slot_of(&ENTRY_LIST.replace("\"IsEntryList\":true", "\"IsEntryList\":false"), "mercedes_sls"), Err(JoinError::NoOpenSlot));
        assert_eq!(slot_of("<html>not the race server</html>", "mercedes_sls"), Err(JoinError::ServerUnreachable));
        // A server that answers with something that does not belong into an ini file is not ours.
        assert_eq!(slot_of(&ENTRY_LIST.replace("Argento_Iridio", "x\\r\\n[REMOTE]"), "mercedes_sls"), Err(JoinError::ServerUnreachable));
    }

    #[test]
    fn writes_the_race_ini_of_an_online_session() {
        let slot = slot_of(ENTRY_LIST, "ks_porsche_911_gt3_cup_2017").unwrap();
        let previous = "[OPTIONS]\r\nUSE_MPH=1\r\n\r\n[CAR_0]\r\nNATIONALITY=Germany\r\nNATION_CODE=GER\r\nSKIN=old\r\n\r\n[REPLAY]\r\nACTIVE=1\r\nFILENAME=last.acreplay\r\n\r\n[REMOTE]\r\nPASSWORD=secret-of-another-server\r\n";
        let ini = render_race_ini(&ticket(), &slot, "ks_nurburgring", previous);

        assert_eq!(ini_value(&ini, "REMOTE", "ACTIVE"), Some("1"));
        assert_eq!(ini_value(&ini, "REMOTE", "SERVER_IP"), Some("race.digitaldrivers.club"));
        assert_eq!(ini_value(&ini, "REMOTE", "SERVER_PORT"), Some("9610"));
        assert_eq!(ini_value(&ini, "REMOTE", "SERVER_HTTP_PORT"), Some("8110"));
        assert_eq!(ini_value(&ini, "REMOTE", "GUID"), Some("76561198000000001"));
        assert_eq!(ini_value(&ini, "REMOTE", "NAME"), Some("Anna Example"));
        assert_eq!(ini_value(&ini, "REMOTE", "REQUESTED_CAR"), Some("ks_porsche_911_gt3_cup_2017"));
        assert_eq!(ini_value(&ini, "REMOTE", "__FEATURES"), Some("SPECTATING_AWARE,CLIENT_MESSAGES,WEATHERFX_V1,STEAM_TICKET"));
        assert_eq!(ini_value(&ini, "RACE", "TRACK"), Some("ks_nurburgring"));
        assert_eq!(ini_value(&ini, "RACE", "CONFIG_TRACK"), Some("layout_gp_a"));
        assert_eq!(ini_value(&ini, "RACE", "MODEL"), Some("ks_porsche_911_gt3_cup_2017"));
        assert_eq!(ini_value(&ini, "RACE", "SKIN"), Some("0_cup_b"));
        assert_eq!(ini_value(&ini, "CAR_0", "SKIN"), Some("0_cup_b"));
        // Kept from the file as it was: what belongs to the driver. Gone: what belonged to the last session.
        assert_eq!(ini_value(&ini, "OPTIONS", "USE_MPH"), Some("1"));
        assert_eq!(ini_value(&ini, "CAR_0", "NATION_CODE"), Some("GER"));
        assert_eq!(ini_value(&ini, "REPLAY", "ACTIVE"), Some("0"));
        assert_eq!(ini_value(&ini, "REMOTE", "PASSWORD"), Some(""));
        assert!(!ini.contains("secret-of-another-server") && !ini.contains("last.acreplay"));
        assert!(ini.lines().count() > 50 && ini.contains("\r\n") && !ini.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn takes_the_track_name_the_race_server_gives_including_the_csp_gate() {
        // A server that asks for Custom Shaders Patch 2651 or newer names its track this way, and the game
        // is only let in with exactly that name.
        let gated = r#"{"name":"Digital Drivers | Friday Cup","track":"csp/2651/../ks_nurburgring-layout_gp_a","pass":false}"#;
        assert_eq!(server_track(gated, "ks_nurburgring", "layout_gp_a"), Ok("csp/2651/../ks_nurburgring".to_string()));
        assert_eq!(server_track(r#"{"track":"ks_nurburgring-layout_gp_a"}"#, "ks_nurburgring", "layout_gp_a"), Ok("ks_nurburgring".to_string()));
        assert_eq!(server_track(r#"{"track":"csp/2651/../rt_highway"}"#, "rt_highway", ""), Ok("csp/2651/../rt_highway".to_string()));
        // Another track, or a name that would leave the content folder or add keys: not the ticket's server.
        assert_eq!(server_track(r#"{"track":"ks_monza-gp"}"#, "ks_nurburgring", "layout_gp_a"), Err(JoinError::ServerUnreachable));
        assert_eq!(server_track(r#"{"track":"../../evil/ks_nurburgring-layout_gp_a"}"#, "ks_nurburgring", "layout_gp_a"), Err(JoinError::ServerUnreachable));
        assert_eq!(server_track(r#"{"track":"csp/2651/../x
PASSWORD=y/ks_nurburgring-layout_gp_a"}"#, "ks_nurburgring", "layout_gp_a"), Err(JoinError::ServerUnreachable));
        assert_eq!(server_track("<html></html>", "ks_nurburgring", "layout_gp_a"), Err(JoinError::ServerUnreachable));
    }

    #[test]
    fn writes_the_track_the_way_the_race_server_names_it() {
        let slot = slot_of(ENTRY_LIST, "ks_porsche_911_gt3_cup_2017").unwrap();
        let ini = render_race_ini(&ticket(), &slot, "csp/2651/../ks_nurburgring", "");
        assert_eq!(ini_value(&ini, "RACE", "TRACK"), Some("csp/2651/../ks_nurburgring"));
        assert_eq!(ini_value(&ini, "RACE", "CONFIG_TRACK"), Some("layout_gp_a"));
    }

    #[test]
    fn asks_the_race_server_what_it_runs() {
        let (port, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"track\":\"csp/2651/../ks_nurburgring-layout_gp_a\"}");
        let body = fetch_info("127.0.0.1", port, Duration::from_secs(5)).unwrap();
        assert_eq!(body, "{\"track\":\"csp/2651/../ks_nurburgring-layout_gp_a\"}");
        assert!(server.join().unwrap().starts_with("GET /INFO HTTP/1.0\r\n"));
    }

    #[test]
    fn writes_a_race_ini_on_a_pc_that_has_none_yet() {
        let slot = slot_of(ENTRY_LIST, "mercedes_sls").unwrap();
        let ini = render_race_ini(&JoinTicket { driver_name: "Anna\r\n[REMOTE]\r\nGUID=1".to_string(), ..ticket() }, &slot, "ks_nurburgring", "");
        assert_eq!(ini_value(&ini, "OPTIONS", "USE_MPH"), Some("0"));
        assert_eq!(ini_value(&ini, "REMOTE", "NAME"), Some("Anna[REMOTE]GUID=1"));
        // The name stays one line: what is left of it opens no section and sets no key.
        assert_eq!(ini.lines().filter(|line| line.trim() == "[REMOTE]").count(), 1);
        assert_eq!(ini_value(&ini, "REMOTE", "GUID"), Some("76561198000000001"));
    }

    /// A race server that answers one request and hangs up, the way Kestrel answers HTTP/1.0.
    fn serve_once(response: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let read = stream.read(&mut request).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request[..read]).to_string()
        });
        (port, handle)
    }

    #[test]
    fn asks_the_race_server_for_the_entry_list_as_the_driver() {
        let (port, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"Cars\":[],\"Features\":[]}");
        let body = fetch_entry_list("127.0.0.1", port, "76561198000000001", Duration::from_secs(5)).unwrap();
        assert_eq!(body, "{\"Cars\":[],\"Features\":[]}");
        assert!(server.join().unwrap().starts_with("GET /JSON%7C76561198000000001 HTTP/1.0\r\n"));
    }

    #[test]
    fn reports_a_server_that_is_not_there_or_not_well() {
        let (port, server) = serve_once("HTTP/1.1 500 Internal Server Error\r\n\r\noops");
        assert_eq!(fetch_entry_list("127.0.0.1", port, "76561198000000001", Duration::from_secs(5)), Err(JoinError::ServerUnreachable));
        server.join().unwrap();
        // Nothing listens on the port any more.
        assert_eq!(fetch_entry_list("127.0.0.1", port, "76561198000000001", Duration::from_secs(2)), Err(JoinError::ServerUnreachable));
        assert_eq!(fetch_entry_list("no such host.invalid", 8110, "76561198000000001", Duration::from_secs(2)), Err(JoinError::ServerUnreachable));
    }
}
