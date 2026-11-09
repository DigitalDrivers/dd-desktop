//! Assetto Corsa EVO's replays. The game saves them as `.rpy` files (game state, no video) below
//! `Saved Games\ACE\Replay`: `Temp` holds what it saves by itself when a session is left (the gallery's
//! "AUTO SAVED", rotated by the game), `Saved` what the driver saved ("USER SAVED"). The app lists both from the
//! files' headers, keeps a Temp replay by moving it to `Saved`, and deletes one. Loading one stays with the
//! driver, in the game's gallery: the game has no command for it.
//!
//! The header (research/2026-11-08-replay-video/B-evo-graphics-replay.md section 4, checked on 25 of Joe's files):
//! u16 format | f64 duration s | f64 recorded from s | u32 0 | u32 123 | u32 len + the driver's car id |
//! u32 len + `SeasonDefinition` (protobuf) | u32 123 | u32 len + `SessionDefinition` (protobuf) | ... the data.
//! The season also holds the registration keys of the cars, which carry SteamIDs: they are never read.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::garage::varint;

/// The replays' folder in the game's `Saved Games\ACE`.
pub const REPLAY_DIR: &str = "Replay";
const EXTENSION: &str = ".rpy";
/// The marker between the header's blocks.
const MARKER: u32 = 123;
/// The driver's car id; the game's are below 64 characters.
const MAX_CAR: usize = 256;
/// A header block; Joe's are 2.4 KB (season) and 2.1 KB (session) with a few cars, each further car adds about 60
/// bytes to the season.
const MAX_BLOCK: usize = 1024 * 1024;
/// `SeasonDefinitionType_MultiPlayer`: the season's name is then the server's.
const MULTIPLAYER: u64 = 1;

/// The game's two replay folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Folder {
    /// The driver's saves: the gallery's "USER SAVED".
    Saved,
    /// The game's own saves at the end of a session, which it rotates: the gallery's "AUTO SAVED".
    Temp,
}

impl Folder {
    pub fn dir_name(self) -> &'static str {
        match self {
            Folder::Saved => "Saved",
            Folder::Temp => "Temp",
        }
    }
}

/// What a replay's header says.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// The replay format (16 for every replay of EVO 0.9.1).
    pub format: u16,
    /// Where the recording ends, in seconds since the session started.
    pub duration_s: f64,
    /// Where the recording starts, in seconds since the session started: 0, or where the game's rolling buffer
    /// shifted to in a session longer than the replay length of its settings (`Shifting replay data` in its log).
    pub start_s: f64,
    /// The id of the driver's car (also in replays the game names `..._spectator_...`).
    pub car: String,
    pub track: Option<String>,
    pub layout: Option<String>,
    /// The session's name, "Practice" for example.
    pub session: Option<String>,
    /// The server's name, for an online session.
    pub server: Option<String>,
}

/// Why a header does not read; for tests, the list only says `damaged`.
#[derive(Debug, PartialEq, Eq)]
pub enum Damage {
    /// The file ends inside the header.
    Short,
    /// A value that cannot be: names it.
    Invalid(&'static str),
    Unreadable(io::ErrorKind),
}

fn bytes<const N: usize>(reader: &mut impl Read) -> Result<[u8; N], Damage> {
    let mut out = [0u8; N];
    reader.read_exact(&mut out).map_err(|e| if e.kind() == io::ErrorKind::UnexpectedEof { Damage::Short } else { Damage::Unreadable(e.kind()) })?;
    Ok(out)
}

fn u32_of(reader: &mut impl Read) -> Result<u32, Damage> {
    Ok(u32::from_le_bytes(bytes(reader)?))
}

fn block(reader: &mut impl Read, max: usize, what: &'static str) -> Result<Vec<u8>, Damage> {
    let len = u32_of(reader)? as usize;
    if len > max {
        return Err(Damage::Invalid(what));
    }
    let mut out = Vec::with_capacity(len);
    reader.take(len as u64).read_to_end(&mut out).map_err(|e| Damage::Unreadable(e.kind()))?;
    if out.len() < len {
        return Err(Damage::Short);
    }
    Ok(out)
}

/// One field of a protobuf message: its number and, for what the header needs, its value.
enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

/// The fields of a protobuf message, None when it is none.
fn fields(data: &[u8]) -> Option<Vec<(u64, Value<'_>)>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let key = varint(data, &mut at)?;
        let value = match key & 7 {
            0 => Value::Varint(varint(data, &mut at)?),
            1 | 5 => {
                at = at.checked_add(if key & 7 == 1 { 8 } else { 4 }).filter(|end| *end <= data.len())?;
                Value::Fixed
            }
            2 => {
                let len = usize::try_from(varint(data, &mut at)?).ok()?;
                let end = at.checked_add(len).filter(|end| *end <= data.len())?;
                let body = &data[at..end];
                at = end;
                Value::Bytes(body)
            }
            _ => return None,
        };
        out.push((key >> 3, value));
    }
    Some(out)
}

/// The last field `number` of a message as bytes (a string or a message).
fn bytes_field<'a>(message: &[(u64, Value<'a>)], number: u64) -> Option<&'a [u8]> {
    message.iter().rev().find_map(|(n, v)| match v {
        Value::Bytes(b) if *n == number => Some(*b),
        _ => None,
    })
}

fn text_field(message: &[(u64, Value<'_>)], number: u64) -> Option<String> {
    bytes_field(message, number).and_then(|b| std::str::from_utf8(b).ok()).map(str::trim).filter(|t| !t.is_empty()).map(str::to_string)
}

fn varint_field(message: &[(u64, Value<'_>)], number: u64) -> Option<u64> {
    message.iter().rev().find_map(|(n, v)| match v {
        Value::Varint(x) if *n == number => Some(*x),
        _ => None,
    })
}

/// Reads a replay's header from the start of the file; reads only the header (about 5 KB), never the data.
pub fn read_header(reader: &mut impl Read) -> Result<Header, Damage> {
    let start: [u8; 0x1A] = bytes(reader)?;
    let format = u16::from_le_bytes([start[0], start[1]]);
    let duration_s = f64::from_le_bytes(start[2..10].try_into().unwrap());
    let start_s = f64::from_le_bytes(start[10..18].try_into().unwrap());
    if u32::from_le_bytes(start[22..26].try_into().unwrap()) != MARKER {
        return Err(Damage::Invalid("marker"));
    }
    if !duration_s.is_finite() || duration_s < 0.0 {
        return Err(Damage::Invalid("duration"));
    }
    if !start_s.is_finite() || start_s < 0.0 || start_s > duration_s {
        return Err(Damage::Invalid("start"));
    }
    let car = block(reader, MAX_CAR, "car")?;
    if car.is_empty() || !car.iter().all(|b| b.is_ascii_graphic()) {
        return Err(Damage::Invalid("car"));
    }
    let season = block(reader, MAX_BLOCK, "season")?;
    let season = fields(&season).ok_or(Damage::Invalid("season"))?;
    if u32_of(reader)? != MARKER {
        return Err(Damage::Invalid("marker"));
    }
    let session = block(reader, MAX_BLOCK, "session")?;
    let session = fields(&session).ok_or(Damage::Invalid("session"))?;
    // SessionDefinition: name 2, scene 21 (track_content_data 1 with its name 1, track_layout_name 11)
    let scene = bytes_field(&session, 21).and_then(fields);
    let track = scene.as_ref().and_then(|scene| bytes_field(scene, 1)).and_then(fields).and_then(|content| text_field(&content, 1));
    let layout = scene.as_ref().and_then(|scene| text_field(scene, 11));
    // SeasonDefinition: name 2, season_type 667
    let server = (varint_field(&season, 667) == Some(MULTIPLAYER)).then(|| text_field(&season, 2)).flatten();
    Ok(Header {
        format,
        duration_s,
        start_s,
        car: String::from_utf8(car).expect("checked to be ASCII"),
        track,
        layout,
        session: text_field(&session, 2),
        server,
    })
}

/// A replay as the page lists it. The header's values are null for a damaged file.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayInfo {
    /// The file's name, which `keep` and `delete` take
    pub name: String,
    pub folder: Folder,
    /// Bytes
    pub size: u64,
    /// When the file was written, seconds since 1970
    pub modified: u64,
    /// The header does not read: the game would not load it either.
    pub damaged: bool,
    pub format: Option<u16>,
    pub duration_s: Option<f64>,
    pub start_s: Option<f64>,
    pub car: Option<String>,
    pub track: Option<String>,
    pub layout: Option<String>,
    pub session: Option<String>,
    pub server: Option<String>,
    /// Recorded in another format than the newest replay: likely by a game version before an update, which the
    /// gallery lists as "INCOMPATIBLE GAME VERSION".
    pub incompatible: bool,
}

/// Both folders, the newest first, with what they take on the disk.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub saved: Vec<ReplayInfo>,
    pub temp: Vec<ReplayInfo>,
    pub saved_bytes: u64,
    pub temp_bytes: u64,
}

/// A replay's file name as the game makes them (`ACEVO_261108-181001_T_dd_bmw_m3_e46_rennsport_Nurburgring.rpy`),
/// and nothing that could leave the folder it is in.
pub fn is_replay_name(value: &str) -> bool {
    value.chars().count() <= 255
        && value.len() > EXTENSION.len()
        && value.ends_with(EXTENSION)
        && value.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '(' | ')' | '-' | '_' | '.'))
        && !value.starts_with([' ', '.'])
}

fn unix_secs(time: io::Result<std::time::SystemTime>) -> u64 {
    time.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

fn info(path: &Path, name: String, folder: Folder) -> Option<ReplayInfo> {
    let meta = fs::symlink_metadata(path).ok().filter(fs::Metadata::is_file)?;
    let header = fs::File::open(path).map_err(|e| Damage::Unreadable(e.kind())).and_then(|file| read_header(&mut io::BufReader::with_capacity(8192, file))).ok();
    let h = header.as_ref();
    Some(ReplayInfo {
        name,
        folder,
        size: meta.len(),
        modified: unix_secs(meta.modified()),
        damaged: header.is_none(),
        format: h.map(|h| h.format),
        duration_s: h.map(|h| h.duration_s),
        start_s: h.map(|h| h.start_s),
        car: h.map(|h| h.car.clone()),
        track: h.and_then(|h| h.track.clone()),
        layout: h.and_then(|h| h.layout.clone()),
        session: h.and_then(|h| h.session.clone()),
        server: h.and_then(|h| h.server.clone()),
        incompatible: false,
    })
}

fn list_folder(replay_dir: &Path, folder: Folder) -> Vec<ReplayInfo> {
    let Ok(entries) = fs::read_dir(replay_dir.join(folder.dir_name())) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok().filter(|n| is_replay_name(n))?;
            // a file that went meanwhile is not listed
            info(&entry.path(), name, folder)
        })
        .collect()
}

/// Lists the replays of both folders below `replay_dir` (`Saved Games\ACE\Replay`), each the newest first. A
/// folder the game has not made yet is empty. Reads only each file's header.
///
/// Whether a replay is `incompatible` the game decides by itself, and the header shows only its format: so a
/// replay counts as incompatible when its format differs from the newest replay's, which the installed game
/// wrote unless it was updated since.
pub fn list(replay_dir: &Path) -> Listing {
    let mut saved = list_folder(replay_dir, Folder::Saved);
    let mut temp = list_folder(replay_dir, Folder::Temp);
    let newest_first = |a: &ReplayInfo, b: &ReplayInfo| (b.modified, &b.name).cmp(&(a.modified, &a.name));
    saved.sort_by(newest_first);
    temp.sort_by(newest_first);
    let current = saved.iter().chain(&temp).filter(|r| !r.damaged).max_by_key(|r| r.modified).and_then(|r| r.format);
    for replay in saved.iter_mut().chain(temp.iter_mut()) {
        replay.incompatible = replay.format.is_some() && replay.format != current;
    }
    Listing {
        saved_bytes: saved.iter().map(|r| r.size).sum(),
        temp_bytes: temp.iter().map(|r| r.size).sum(),
        saved,
        temp,
    }
}

/// Why a replay was not kept or deleted. The code goes to the hosted interface, which has the words for it.
#[derive(Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// The name is no replay's file name.
    InvalidName,
    /// There is no such replay (the game may have rotated it meanwhile).
    NotFound,
    /// The file could not be moved or deleted.
    Failed(String),
}

impl ReplayError {
    pub fn code(&self) -> String {
        match self {
            ReplayError::InvalidName => "invalid-replay".to_string(),
            ReplayError::NotFound => "replay-not-found".to_string(),
            ReplayError::Failed(_) => "failed".to_string(),
        }
    }
}

/// The path of an existing replay file (no folder, no link), or why there is none.
fn replay_file(replay_dir: &Path, folder: Folder, name: &str) -> Result<std::path::PathBuf, ReplayError> {
    if !is_replay_name(name) {
        return Err(ReplayError::InvalidName);
    }
    let path = replay_dir.join(folder.dir_name()).join(name);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => Ok(path),
        Ok(_) => Err(ReplayError::InvalidName),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(ReplayError::NotFound),
        Err(e) => Err(ReplayError::Failed(format!("replay: {e}"))),
    }
}

/// Keeps a Temp replay, which the game would rotate out: moves it to `Saved` under its name, as the gallery's
/// "move" does, or with `_2`, `_3`, ... before `.rpy` when `Saved` has one of that name. Answers the name in
/// `Saved`. Never replaces a file.
pub fn keep(replay_dir: &Path, name: &str) -> Result<String, ReplayError> {
    let from = replay_file(replay_dir, Folder::Temp, name)?;
    let saved = replay_dir.join(Folder::Saved.dir_name());
    fs::create_dir_all(&saved).map_err(|e| ReplayError::Failed(format!("saved folder: {e}")))?;
    let stem = &name[..name.len() - EXTENSION.len()];
    let target = std::iter::once(name.to_string())
        .chain((2..100).map(|n| format!("{stem}_{n}{EXTENSION}")))
        .find(|candidate| fs::symlink_metadata(saved.join(candidate)).is_err_and(|e| e.kind() == io::ErrorKind::NotFound))
        .ok_or_else(|| ReplayError::Failed("saved folder: every name taken".to_string()))?;
    match fs::rename(&from, saved.join(&target)) {
        Ok(()) => Ok(target),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(ReplayError::NotFound),
        Err(e) => Err(ReplayError::Failed(format!("move: {e}"))),
    }
}

/// Deletes one replay file. One that is already gone is no mistake.
pub fn delete(replay_dir: &Path, folder: Folder, name: &str) -> Result<(), ReplayError> {
    let path = match replay_file(replay_dir, folder, name) {
        Err(ReplayError::NotFound) => return Ok(()),
        other => other?,
    };
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ReplayError::Failed(format!("delete: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Joe's Temp replay of 2026-11-08 15:11:52 up to the end of the session block, with the registration keys
    /// (SteamIDs) and the season's id overwritten by zeros.
    const JOE: &[u8] = include_bytes!("../tests/fixtures/replay-header-261108-151152.rpy");

    /// A header as the game writes it, with the given blocks.
    fn header(format: u16, duration: f64, start: f64, car: &[u8], season: &[u8], session: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(format.to_le_bytes());
        out.extend(duration.to_le_bytes());
        out.extend(start.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend(MARKER.to_le_bytes());
        for (i, block) in [car, season, session].into_iter().enumerate() {
            if i == 2 {
                out.extend(MARKER.to_le_bytes());
            }
            out.extend((block.len() as u32).to_le_bytes());
            out.extend(block);
        }
        // what follows the header: the next marker and the data
        out.extend(MARKER.to_le_bytes());
        out.extend([0xAB; 64]);
        out
    }

    fn pb_bytes(field: u64, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        pb_varint(&mut out, field << 3 | 2);
        pb_varint(&mut out, body.len() as u64);
        out.extend(body);
        out
    }

    fn pb_number(field: u64, value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        pb_varint(&mut out, field << 3);
        pb_varint(&mut out, value);
        out
    }

    fn pb_varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push((value as u8) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    /// A season of an online session on a server named `server`, with a car's registration key (a SteamID).
    fn season(server: &str, season_type: u64) -> Vec<u8> {
        let car = pb_bytes(1, &pb_bytes(1, b"76561190000000000-1"));
        [pb_bytes(2, server.as_bytes()), pb_bytes(101, &[pb_number(1, 0), pb_bytes(2, &car)].concat()), pb_number(667, season_type), pb_number(671, 10)].concat()
    }

    fn session(name: &str, track: &str, layout: &str) -> Vec<u8> {
        let content = [pb_bytes(1, track.as_bytes()), pb_bytes(5, b"DEU")].concat();
        let scene = [pb_bytes(1, &content), pb_bytes(4, b"content\\tracks\\x.scene"), pb_bytes(11, layout.as_bytes())].concat();
        [pb_bytes(2, name.as_bytes()), pb_bytes(21, &scene)].concat()
    }

    fn synthetic() -> Vec<u8> {
        header(16, 557.406005859375, 0.0, b"dd_bmw_m3_e46_rennsport", &season("DD | Spa | Club Cars", 1), &session("Race", "Spa", "GP"))
    }

    #[test]
    fn reads_joe_s_header() {
        let h = read_header(&mut &JOE[..]).unwrap();
        assert_eq!(h.format, 16);
        assert!((h.duration_s - 120.78).abs() < 0.01, "{}", h.duration_s);
        assert_eq!(h.start_s, 0.0);
        assert_eq!(h.car, "dd_bmw_m3_e46_rennsport");
        assert_eq!(h.track.as_deref(), Some("Nurburgring"));
        assert_eq!(h.layout.as_deref(), Some("Touristenfahrten"));
        assert_eq!(h.session.as_deref(), Some("Practice"));
        assert_eq!(h.server.as_deref(), Some("AC EVO Digital Drivers | Nurburgring | Club Cars"));
    }

    #[test]
    fn the_fixture_holds_no_steam_id() {
        // every SteamID64 of a person starts with 7656119; the two registration keys are zeros now
        assert!(!JOE.windows(7).any(|w| w == b"7656119"));
        assert_eq!(JOE.windows(50).filter(|w| w.iter().all(|b| *b == b'0')).count(), 2);
    }

    #[test]
    fn reads_every_field_of_a_header() {
        let h = read_header(&mut &synthetic()[..]).unwrap();
        assert_eq!(
            h,
            Header {
                format: 16,
                duration_s: 557.406005859375,
                start_s: 0.0,
                car: "dd_bmw_m3_e46_rennsport".into(),
                track: Some("Spa".into()),
                layout: Some("GP".into()),
                session: Some("Race".into()),
                server: Some("DD | Spa | Club Cars".into()),
            }
        );
        // the rolling buffer's start
        let late = header(16, 8525.85, 7428.64, b"dd_audi_rs6_gt3", &season("x", 1), &session("Practice", "Nurburgring", "Touristenfahrten"));
        let h = read_header(&mut &late[..]).unwrap();
        assert_eq!((h.duration_s, h.start_s), (8525.85, 7428.64));
    }

    #[test]
    fn names_a_server_only_for_an_online_session() {
        let offline = header(16, 60.0, 0.0, b"ks_car", &season("Practice", 0), &session("Practice", "Imola", ""));
        let h = read_header(&mut &offline[..]).unwrap();
        assert_eq!((h.server, h.layout), (None, None));
        // no season type at all: not online either
        let bare = header(16, 60.0, 0.0, b"ks_car", &pb_bytes(2, b"Name"), &[]);
        let h = read_header(&mut &bare[..]).unwrap();
        assert_eq!((h.server, h.track, h.session), (None, None, None));
    }

    #[test]
    fn refuses_damaged_short_and_foreign_files() {
        let good = synthetic();
        // cut anywhere inside the header
        let header_len = good.len() - 4 - 64;
        for cut in [0, 1, 10, 0x19, 0x1D, 0x30, header_len - 1] {
            assert_eq!(read_header(&mut &good[..cut]), Err(Damage::Short), "cut at {cut}");
        }
        // the header alone is enough
        assert!(read_header(&mut &good[..header_len]).is_ok());

        let mut wrong_marker = good.clone();
        wrong_marker[0x16] = 124;
        assert_eq!(read_header(&mut &wrong_marker[..]), Err(Damage::Invalid("marker")));
        let nan = header(16, f64::NAN, 0.0, b"car", &[], &[]);
        assert_eq!(read_header(&mut &nan[..]), Err(Damage::Invalid("duration")));
        let backwards = header(16, 10.0, 20.0, b"car", &[], &[]);
        assert_eq!(read_header(&mut &backwards[..]), Err(Damage::Invalid("start")));
        let control = header(16, 10.0, 0.0, b"car\n", &[], &[]);
        assert_eq!(read_header(&mut &control[..]), Err(Damage::Invalid("car")));
        let no_car = header(16, 10.0, 0.0, b"", &[], &[]);
        assert_eq!(read_header(&mut &no_car[..]), Err(Damage::Invalid("car")));
        // a car length of 4 GB: refused before anything is read
        let mut huge = good.clone();
        huge[0x1A..0x1E].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read_header(&mut &huge[..]), Err(Damage::Invalid("car")));
        let broken_season = header(16, 10.0, 0.0, b"car", &[0x12, 0x40, b'x'], &[]);
        assert_eq!(read_header(&mut &broken_season[..]), Err(Damage::Invalid("season")));
        let broken_session = header(16, 10.0, 0.0, b"car", &[], &[0x0F]);
        assert_eq!(read_header(&mut &broken_session[..]), Err(Damage::Invalid("session")));
        // another file under the name: a PNG, a text
        assert!(read_header(&mut &b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\x07\x80\0\0\x048\x08\x06\0\0\0\0\0\0\0\0\0\0"[..]).is_err());
        assert!(read_header(&mut &[b'a'; 4096][..]).is_err());
    }

    /// A Replay folder made new for every test.
    fn replay_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dd-core-replays-{name}-{}", std::process::id())).join(REPLAY_DIR);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Saved")).unwrap();
        fs::create_dir_all(dir.join("Temp")).unwrap();
        dir
    }

    fn put(dir: &Path, folder: &str, name: &str, data: &[u8], modified: u64) {
        let path = dir.join(folder).join(name);
        fs::write(&path, data).unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(modified)).unwrap();
    }

    const T1: &str = "ACEVO_261108-151152_T_dd_bmw_m3_e46_rennsport_Nurburgring.rpy";
    const T2: &str = "ACEVO_261108-181001_T_dd_bmw_m3_e46_rennsport_Nurburgring.rpy";
    const S1: &str = "ACEVO_261108-152559_T_spectator_Nurburgring.rpy";

    #[test]
    fn lists_both_folders_newest_first_with_their_sizes() {
        let dir = replay_dir("list");
        put(&dir, "Temp", T1, JOE, 1_000);
        put(&dir, "Temp", T2, &synthetic(), 3_000);
        put(&dir, "Saved", S1, JOE, 2_000);
        // not replays: another file, a folder, a broken name
        put(&dir, "Saved", "notes.txt", b"x", 5_000);
        fs::create_dir_all(dir.join("Saved").join("old.rpy")).unwrap();
        put(&dir, "Temp", "broken.rpy", b"\x10\0nothing", 4_000);

        let listing = list(&dir);
        let names = |r: &[ReplayInfo]| r.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&listing.temp), ["broken.rpy", T2, T1]);
        assert_eq!(names(&listing.saved), [S1]);
        assert_eq!(listing.saved_bytes, JOE.len() as u64);
        assert_eq!(listing.temp_bytes, (JOE.len() + synthetic().len() + 9) as u64);

        let broken = &listing.temp[0];
        assert!(broken.damaged && !broken.incompatible);
        assert_eq!((broken.duration_s, broken.car.as_deref(), broken.size, broken.modified), (None, None, 9, 4_000));
        let saved = &listing.saved[0];
        assert_eq!((saved.folder, saved.damaged, saved.format, saved.car.as_deref()), (Folder::Saved, false, Some(16), Some("dd_bmw_m3_e46_rennsport")));
        assert_eq!(saved.track.as_deref(), Some("Nurburgring"));
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_missing_folder_is_an_empty_one() {
        let dir = replay_dir("missing");
        fs::remove_dir_all(dir.join("Temp")).unwrap();
        put(&dir, "Saved", S1, JOE, 2_000);
        let listing = list(&dir);
        assert_eq!((listing.saved.len(), listing.temp.len(), listing.temp_bytes), (1, 0, 0));
        let nothing = list(&dir.join("nowhere"));
        assert!(nothing.saved.is_empty() && nothing.temp.is_empty());
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn calls_replays_of_another_format_than_the_newest_incompatible() {
        let dir = replay_dir("incompatible");
        let old = header(15, 60.0, 0.0, b"ks_car", &[], &[]);
        put(&dir, "Saved", "ACEVO_250101-120000_T_ks_car_Imola.rpy", &old, 1_000);
        put(&dir, "Saved", S1, JOE, 2_000);
        // a damaged newer file does not decide
        put(&dir, "Temp", T2, b"junk", 3_000);
        let listing = list(&dir);
        assert_eq!(listing.saved.iter().map(|r| r.incompatible).collect::<Vec<_>>(), [false, true]);
        assert!(!listing.temp[0].incompatible);
        // after a game update the newest is the new format, and the 16s are the old ones
        let new = header(17, 60.0, 0.0, b"ks_car", &[], &[]);
        put(&dir, "Temp", T1, &new, 4_000);
        let listing = list(&dir);
        assert_eq!(listing.saved.iter().map(|r| r.incompatible).collect::<Vec<_>>(), [true, true]);
        assert_eq!(listing.temp.iter().map(|r| (r.name.as_str(), r.incompatible)).collect::<Vec<_>>(), [(T1, false), (T2, false)]);
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn keeps_a_temp_replay_by_moving_it_to_saved_without_replacing_one() {
        let dir = replay_dir("keep");
        put(&dir, "Temp", T1, JOE, 1_000);
        assert_eq!(keep(&dir, T1), Ok(T1.to_string()));
        assert!(!dir.join("Temp").join(T1).exists());
        assert_eq!(fs::read(dir.join("Saved").join(T1)).unwrap(), JOE);
        // the same name again: the one in Saved stays, the new one gets a number
        put(&dir, "Temp", T1, b"second", 1_000);
        put(&dir, "Saved", "ACEVO_261108-151152_T_dd_bmw_m3_e46_rennsport_Nurburgring_2.rpy", b"taken", 1_000);
        assert_eq!(keep(&dir, T1), Ok("ACEVO_261108-151152_T_dd_bmw_m3_e46_rennsport_Nurburgring_3.rpy".to_string()));
        assert_eq!(fs::read(dir.join("Saved").join(T1)).unwrap(), JOE);
        assert_eq!(fs::read(dir.join("Saved").join("ACEVO_261108-151152_T_dd_bmw_m3_e46_rennsport_Nurburgring_3.rpy")).unwrap(), b"second");
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn keeps_into_a_saved_folder_the_game_has_not_made_yet() {
        let dir = replay_dir("keep-new");
        fs::remove_dir_all(dir.join("Saved")).unwrap();
        put(&dir, "Temp", T1, JOE, 1_000);
        assert_eq!(keep(&dir, T1), Ok(T1.to_string()));
        assert!(dir.join("Saved").join(T1).is_file());
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_replay_that_went_meanwhile_is_not_found_for_keep_and_gone_for_delete() {
        let dir = replay_dir("vanished");
        put(&dir, "Temp", T1, JOE, 1_000);
        let listed = list(&dir);
        assert_eq!(listed.temp.len(), 1);
        // the game rotated it out after the page listed it
        fs::remove_file(dir.join("Temp").join(T1)).unwrap();
        assert_eq!(keep(&dir, T1), Err(ReplayError::NotFound));
        assert_eq!(delete(&dir, Folder::Temp, T1), Ok(()));
        assert!(fs::read_dir(dir.join("Saved")).unwrap().next().is_none());
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn deletes_one_replay_in_the_folder_asked() {
        let dir = replay_dir("delete");
        put(&dir, "Temp", T1, JOE, 1_000);
        put(&dir, "Saved", T1, JOE, 1_000);
        assert_eq!(delete(&dir, Folder::Saved, T1), Ok(()));
        assert!(!dir.join("Saved").join(T1).exists());
        assert!(dir.join("Temp").join(T1).is_file());
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn refuses_names_that_could_leave_the_folder_or_are_no_replay() {
        let dir = replay_dir("refuses");
        put(&dir, "Temp", T1, JOE, 1_000);
        // a file next to the Replay folder, which a path trick would reach
        fs::write(dir.join("victim.rpy"), b"x").unwrap();
        fs::write(dir.parent().unwrap().join("victim.rpy"), b"x").unwrap();
        let bad = [
            "", ".rpy", "..\\victim.rpy", "../victim.rpy", "..\\..\\victim.rpy", "C:\\victim.rpy", "C:victim.rpy",
            "\\\\server\\share\\victim.rpy", "Temp/../victim.rpy", "victim.rpy:stream", "victim.rpy\0", "victim.txt", "victim",
            " victim.rpy", ".victim.rpy", "victim.RPY",
        ];
        for name in bad {
            assert!(!is_replay_name(name), "{name:?}");
            assert_eq!(keep(&dir, name), Err(ReplayError::InvalidName), "{name:?}");
            assert_eq!(delete(&dir, Folder::Saved, name), Err(ReplayError::InvalidName), "{name:?}");
        }
        assert!(!is_replay_name(&format!("{}.rpy", "x".repeat(252))));
        assert!(is_replay_name(&format!("{}.rpy", "x".repeat(251))));
        // a folder under a replay's name is no replay
        fs::create_dir_all(dir.join("Saved").join("folder.rpy")).unwrap();
        assert_eq!(delete(&dir, Folder::Saved, "folder.rpy"), Err(ReplayError::InvalidName));
        assert!(dir.join("Saved").join("folder.rpy").is_dir());
        assert!(dir.join("victim.rpy").is_file() && dir.parent().unwrap().join("victim.rpy").is_file());
        assert!(dir.join("Temp").join(T1).is_file());
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn names_its_errors_by_code() {
        assert_eq!(ReplayError::InvalidName.code(), "invalid-replay");
        assert_eq!(ReplayError::NotFound.code(), "replay-not-found");
        assert_eq!(ReplayError::Failed("delete: access denied".into()).code(), "failed");
        assert_eq!(serde_json::to_string(&[Folder::Saved, Folder::Temp]).unwrap(), r#"["saved","temp"]"#);
        assert_eq!(serde_json::from_str::<Folder>(r#""temp""#).unwrap(), Folder::Temp);
        assert!(serde_json::from_str::<Folder>(r#""../Temp""#).is_err());
    }

    #[test]
    fn hands_the_page_camel_case_and_no_steam_id() {
        let dir = replay_dir("json");
        put(&dir, "Temp", T1, JOE, 1_000);
        put(&dir, "Saved", S1, &synthetic(), 2_000);
        let json = serde_json::to_string(&list(&dir)).unwrap();
        for key in ["\"savedBytes\"", "\"tempBytes\"", "\"durationS\"", "\"startS\"", "\"incompatible\"", "\"folder\":\"temp\""] {
            assert!(json.contains(key), "{key} in {json}");
        }
        assert!(!json.contains("7656119"), "{json}");
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    /// Runs with `DD_REPLAY_FIXTURE=<Saved Games\ACE\Replay> cargo test -p dd-core replays -- --ignored --nocapture`,
    /// and with `DD_REPLAY_KIT=<a 07-replays-*.json of research/2026-11-09-testabend>` it compares every replay the
    /// test kit listed. Reads only the headers.
    #[test]
    #[ignore]
    fn reads_the_real_replay_folder_as_the_test_kit_did() {
        let dir = PathBuf::from(std::env::var("DD_REPLAY_FIXTURE").expect("DD_REPLAY_FIXTURE: a Replay folder"));
        let listing = list(&dir);
        let all: Vec<&ReplayInfo> = listing.saved.iter().chain(&listing.temp).collect();
        for r in &all {
            println!(
                "{:5} {:64} {:>8.1} MB {:>8.2} s from {:>7.2} s {:?} {:?}/{:?} {:?} {:?} format {:?}{}{}",
                r.folder.dir_name(), r.name, r.size as f64 / 1e6, r.duration_s.unwrap_or(-1.0), r.start_s.unwrap_or(-1.0), r.car, r.track, r.layout,
                r.session, r.server, r.format, if r.damaged { " DAMAGED" } else { "" }, if r.incompatible { " INCOMPATIBLE" } else { "" }
            );
        }
        println!("{} saved ({} bytes), {} temp ({} bytes)", listing.saved.len(), listing.saved_bytes, listing.temp.len(), listing.temp_bytes);
        assert!(all.iter().all(|r| !r.damaged && !r.incompatible));
        let Ok(kit) = std::env::var("DD_REPLAY_KIT") else { return };
        let text = fs::read_to_string(kit).unwrap();
        let kit: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
        let replays = kit["replays"].as_array().unwrap();
        assert_eq!(replays.len() as u64, kit["count"].as_u64().unwrap());
        for expected in replays {
            let name = expected["name"].as_str().unwrap();
            let found = all.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("{name} not listed"));
            let folder = expected["folder"].as_str().unwrap();
            assert_eq!(found.folder.dir_name(), folder, "{name}");
            let header = &expected["header"];
            assert!((found.duration_s.unwrap() - header["durationS"].as_f64().unwrap()).abs() < 0.006, "{name}");
            assert_eq!(found.car.as_deref(), header["carId"].as_str(), "{name}");
            assert_eq!(Some(found.format.unwrap() as u64), header["version"].as_u64(), "{name}");
            let strings: Vec<&str> = header["strings"].as_array().unwrap().iter().filter_map(|s| s.as_str()).collect();
            for value in [&found.track, &found.layout, &found.session, &found.server].into_iter().flatten() {
                assert!(strings.contains(&value.as_str()), "{name}: {value}");
            }
        }
        let longest = all.iter().map(|r| r.duration_s.unwrap()).fold(0.0, f64::max);
        println!("the kit listed {}, the longest {longest:.2} s", replays.len());
    }
}
