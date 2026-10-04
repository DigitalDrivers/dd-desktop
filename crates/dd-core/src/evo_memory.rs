//! What Assetto Corsa EVO publishes about the running session in its shared memory (official since 0.6,
//! `Local\acevo_pmf_graphics` and `Local\acevo_pmf_static`, documented by Kunos as `SharedFileOut.h`). The
//! shell maps the pages and hands the bytes here; this reads the few fields the live map needs. Offsets are
//! those of the official header with `#pragma pack(4)`, checked against the page sizes the game itself
//! allocates (4900 and 208 bytes, game 0.9.1).

use serde::Serialize;

pub const GRAPHICS_SIZE: usize = 4900;
pub const STATIC_SIZE: usize = 208;

const STATUS: usize = 4;
const PLAYER_CAR_ID: usize = 24;
const CAR_COORDINATES: usize = 3124;
const CAR_IDS: usize = 3940;
const TRACK: usize = 136;
const TRACK_CONFIGURATION: usize = 169;
const IS_ONLINE: usize = 86;
const SLOTS: usize = 60;
/// `ACEVO_STATUS`: 2 = live driving
const LIVE: i32 = 2;

/// A car the game knows about, by the id the server's log uses for it (`<a>-<b>` in hex).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CarPosition {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// The session as the driver's game sees it, for the live map.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshot {
    pub track: String,
    pub layout: String,
    pub online: bool,
    /// The driver's own car
    pub player: String,
    pub cars: Vec<CarPosition>,
}

fn u64_at(page: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(page[at..at + 8].try_into().unwrap())
}

pub(crate) fn f32_at(page: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(page[at..at + 4].try_into().unwrap())
}

pub(crate) fn text_at(page: &[u8], at: usize, len: usize) -> String {
    let bytes = &page[at..at + len];
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(len);
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// The id the server's log writes for a car: both halves as 16 hex digits, joined by a dash.
pub fn car_id(a: u64, b: u64) -> String {
    format!("{a:016x}-{b:016x}")
}

/// Reads the pages; None unless the game is driving live (menus, replays and pauses show nothing).
pub fn read(graphics: &[u8], statics: &[u8]) -> Option<LiveSnapshot> {
    if graphics.len() < GRAPHICS_SIZE || statics.len() < STATIC_SIZE {
        return None;
    }
    if i32::from_le_bytes(graphics[STATUS..STATUS + 4].try_into().unwrap()) != LIVE {
        return None;
    }
    let cars = (0..SLOTS)
        .filter_map(|slot| {
            let (a, b) = (u64_at(graphics, CAR_IDS + slot * 16), u64_at(graphics, CAR_IDS + slot * 16 + 8));
            if a == 0 && b == 0 {
                return None;
            }
            let at = CAR_COORDINATES + slot * 12;
            let (x, y, z) = (f32_at(graphics, at), f32_at(graphics, at + 4), f32_at(graphics, at + 8));
            (x.is_finite() && y.is_finite() && z.is_finite()).then(|| CarPosition { id: car_id(a, b), x, y, z })
        })
        .collect();
    Some(LiveSnapshot {
        track: text_at(statics, TRACK, 33),
        layout: text_at(statics, TRACK_CONFIGURATION, 33),
        online: statics[IS_ONLINE] != 0,
        player: car_id(u64_at(graphics, PLAYER_CAR_ID), u64_at(graphics, PLAYER_CAR_ID + 8)),
        cars,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages() -> (Vec<u8>, Vec<u8>) {
        let mut graphics = vec![0u8; GRAPHICS_SIZE];
        graphics[STATUS..STATUS + 4].copy_from_slice(&LIVE.to_le_bytes());
        graphics[PLAYER_CAR_ID..PLAYER_CAR_ID + 8].copy_from_slice(&0x4f44a4be5be33c37u64.to_le_bytes());
        graphics[PLAYER_CAR_ID + 8..PLAYER_CAR_ID + 16].copy_from_slice(&0x2b051984457156acu64.to_le_bytes());
        for (slot, (a, b, x)) in [(0x4f44a4be5be33c37u64, 0x2b051984457156acu64, 100.5f32), (0x1u64, 0x2u64, -20.0f32)].into_iter().enumerate() {
            graphics[CAR_IDS + slot * 16..CAR_IDS + slot * 16 + 8].copy_from_slice(&a.to_le_bytes());
            graphics[CAR_IDS + slot * 16 + 8..CAR_IDS + slot * 16 + 16].copy_from_slice(&b.to_le_bytes());
            let at = CAR_COORDINATES + slot * 12;
            graphics[at..at + 4].copy_from_slice(&x.to_le_bytes());
            graphics[at + 4..at + 8].copy_from_slice(&3.0f32.to_le_bytes());
            graphics[at + 8..at + 12].copy_from_slice(&(-x).to_le_bytes());
        }
        let mut statics = vec![0u8; STATIC_SIZE];
        statics[TRACK..TRACK + 11].copy_from_slice(b"Nurburgring");
        statics[TRACK_CONFIGURATION..TRACK_CONFIGURATION + 15].copy_from_slice(b"Touristenfahrt\x00");
        statics[IS_ONLINE] = 1;
        (graphics, statics)
    }

    #[test]
    fn reads_the_cars_of_a_live_session_by_the_server_s_car_ids() {
        let (graphics, statics) = pages();
        let snapshot = read(&graphics, &statics).unwrap();
        assert_eq!(snapshot.player, "4f44a4be5be33c37-2b051984457156ac");
        assert_eq!((snapshot.track.as_str(), snapshot.layout.as_str(), snapshot.online), ("Nurburgring", "Touristenfahrt", true));
        assert_eq!(snapshot.cars, vec![
            CarPosition { id: "4f44a4be5be33c37-2b051984457156ac".into(), x: 100.5, y: 3.0, z: -100.5 },
            CarPosition { id: "0000000000000001-0000000000000002".into(), x: -20.0, y: 3.0, z: 20.0 },
        ]);
    }

    #[test]
    fn shows_nothing_outside_a_live_session_or_on_a_page_too_short() {
        let (mut graphics, statics) = pages();
        assert!(read(&graphics[..100], &statics).is_none());
        graphics[STATUS] = 3;
        assert!(read(&graphics, &statics).is_none());
    }
}
