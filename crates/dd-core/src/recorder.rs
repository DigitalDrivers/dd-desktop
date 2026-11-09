//! Recordings of Assetto Corsa EVO's shared memory, for tests: the shell copies the three pages many times a second
//! and hands the bytes here; this writes them as one stream and reads such a stream back. A recording replays a
//! drive without the game, so the spotter and the engineer's calls are tested against what the game really
//! published. Nothing in it leaves the PC.
//!
//! The stream (the shell gzips it):
//!
//! ```text
//! "DDSMREC1" | u32 length | header as JSON | record*
//! record := tag u8 | t_us u64 | body          (little endian; t_us = microseconds since the recording began)
//!   1 frame:   flags u8 | physics (800 bytes) | graphics (4900 bytes)
//!   2 static:  the static page (208 bytes), written when it changed
//!   3 missing: mask u8 of the pages the game does not publish, written when that changed
//! ```

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use crate::evo_memory::{f32_at, GRAPHICS_SIZE, STATIC_SIZE};
use crate::laps::PHYSICS_SIZE;

pub const MAGIC: &[u8; 8] = b"DDSMREC1";
/// What a recording's file name ends in.
pub const EXTENSION: &str = "ddsm.gz";

const FRAME: u8 = 1;
const STATIC: u8 = 2;
const MISSING: u8 = 3;

/// Flag of a frame: the physics page's `packetId` moved on while the page was copied, so the copy may be torn.
pub const PHYSICS_MOVED: u8 = 1;
/// Flag of a frame: the same for the graphics page.
pub const GRAPHICS_MOVED: u8 = 2;

/// Bits of a missing record's mask.
pub const NO_PHYSICS: u8 = 1;
pub const NO_GRAPHICS: u8 = 2;
pub const NO_STATIC: u8 = 4;

/// The fastest a recording samples the pages.
pub const MAX_HZ: u32 = 240;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// When the recording began, milliseconds since 1970: lines it up with a video of the same drive
    pub started_unix_ms: u64,
    /// How many times a second the pages were copied
    pub hz: u32,
    /// What was tested, as the driver named it
    pub label: String,
    /// The app's version
    pub app: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    Frame { t_us: u64, flags: u8, physics: Vec<u8>, graphics: Vec<u8> },
    Static { t_us: u64, page: Vec<u8> },
    Missing { t_us: u64, mask: u8 },
}

/// A page the shell copied, and whether its `packetId` moved on during the copy.
pub struct Copied<'a> {
    pub page: &'a [u8],
    pub moved: bool,
}

/// Writes a recording.
pub struct Writer<W: Write> {
    out: W,
    statics: Vec<u8>,
    missing: u8,
    frames: u64,
}

impl<W: Write> Writer<W> {
    pub fn new(mut out: W, header: &Header) -> io::Result<Self> {
        let json = serde_json::to_vec(header)?;
        out.write_all(MAGIC)?;
        out.write_all(&(json.len() as u32).to_le_bytes())?;
        out.write_all(&json)?;
        Ok(Self { out, statics: Vec::new(), missing: 0, frames: 0 })
    }

    /// One sample: the pages as the shell found them at `t_us`, None for a page the game does not publish (or
    /// one of another size). A frame needs the physics and the graphics page.
    pub fn sample(&mut self, t_us: u64, physics: Option<Copied>, graphics: Option<Copied>, statics: Option<&[u8]>) -> io::Result<()> {
        let physics = physics.filter(|p| p.page.len() == PHYSICS_SIZE);
        let graphics = graphics.filter(|g| g.page.len() == GRAPHICS_SIZE);
        let statics = statics.filter(|s| s.len() == STATIC_SIZE);
        let missing = if physics.is_none() { NO_PHYSICS } else { 0 } | if graphics.is_none() { NO_GRAPHICS } else { 0 } | if statics.is_none() { NO_STATIC } else { 0 };
        if missing != self.missing {
            self.missing = missing;
            self.start(MISSING, t_us)?;
            self.out.write_all(&[missing])?;
        }
        if let Some(page) = statics.filter(|page| *page != self.statics.as_slice()) {
            self.statics = page.to_vec();
            self.start(STATIC, t_us)?;
            self.out.write_all(page)?;
        }
        if let (Some(physics), Some(graphics)) = (physics, graphics) {
            let flags = if physics.moved { PHYSICS_MOVED } else { 0 } | if graphics.moved { GRAPHICS_MOVED } else { 0 };
            self.start(FRAME, t_us)?;
            self.out.write_all(&[flags])?;
            self.out.write_all(physics.page)?;
            self.out.write_all(graphics.page)?;
            self.frames += 1;
        }
        Ok(())
    }

    fn start(&mut self, tag: u8, t_us: u64) -> io::Result<()> {
        self.out.write_all(&[tag])?;
        self.out.write_all(&t_us.to_le_bytes())
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Hands what was written so far to the file, so a recording cut short keeps all but its last moments.
    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

/// Reads a recording record by record. A recording cut short ends at its last whole record.
pub struct Reader<R: Read> {
    input: R,
    pub header: Header,
}

impl<R: Read> Reader<R> {
    pub fn new(mut input: R) -> io::Result<Self> {
        let mut magic = [0u8; 8];
        input.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a shared memory recording"));
        }
        let mut len = [0u8; 4];
        input.read_exact(&mut len)?;
        let mut json = vec![0u8; u32::from_le_bytes(len) as usize];
        input.read_exact(&mut json)?;
        Ok(Self { input, header: serde_json::from_slice(&json)? })
    }

    fn bytes(&mut self, len: usize) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0u8; len];
        self.input.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn record(&mut self) -> io::Result<Record> {
        let head = self.bytes(9)?;
        let t_us = u64::from_le_bytes(head[1..9].try_into().unwrap());
        match head[0] {
            FRAME => Ok(Record::Frame { t_us, flags: self.bytes(1)?[0], physics: self.bytes(PHYSICS_SIZE)?, graphics: self.bytes(GRAPHICS_SIZE)? }),
            STATIC => Ok(Record::Static { t_us, page: self.bytes(STATIC_SIZE)? }),
            MISSING => Ok(Record::Missing { t_us, mask: self.bytes(1)?[0] }),
            tag => Err(io::Error::new(io::ErrorKind::InvalidData, format!("unknown record {tag}"))),
        }
    }
}

impl<R: Read> Iterator for Reader<R> {
    type Item = io::Result<Record>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.record() {
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => None,
            record => Some(record),
        }
    }
}

// What the recorder's status line shows; offsets as in `evo_memory` and `laps`.
const STATUS: usize = 4;
const PLAYER_CAR_ID: usize = 24;
const SPEED_KMH: usize = 28;
const TOTAL_DRIVERS: usize = 2392;
const ACTIVE_CARS: usize = 3852;
const CAR_IDS: usize = 3940;
const SLOTS: usize = 60;

/// A frame at a glance, for the recorder's status line: enough to see during a test whether the game publishes.
#[derive(Debug, Clone, PartialEq)]
pub struct Glance {
    /// `ACEVO_STATUS`: 0 off, 1 replay, 2 live, 3 pause
    pub status: i32,
    pub physics_packet: i32,
    pub graphics_packet: i32,
    /// Slots that carry a car id
    pub cars: usize,
    /// The slot whose car id is the driver's
    pub own_slot: Option<usize>,
    pub speed_kmh: f32,
    pub total_drivers: u32,
    pub active_cars: u8,
}

fn i32_at(page: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(page[at..at + 4].try_into().unwrap())
}

/// None for pages of another size.
pub fn glance(physics: &[u8], graphics: &[u8]) -> Option<Glance> {
    if physics.len() != PHYSICS_SIZE || graphics.len() != GRAPHICS_SIZE {
        return None;
    }
    let id = |at: usize| &graphics[at..at + 16];
    let taken: Vec<usize> = (0..SLOTS).filter(|slot| id(CAR_IDS + slot * 16).iter().any(|b| *b != 0)).collect();
    Some(Glance {
        status: i32_at(graphics, STATUS),
        physics_packet: i32_at(physics, 0),
        graphics_packet: i32_at(graphics, 0),
        cars: taken.len(),
        own_slot: taken.iter().copied().find(|slot| id(CAR_IDS + slot * 16) == id(PLAYER_CAR_ID)),
        speed_kmh: f32_at(physics, SPEED_KMH),
        total_drivers: i32_at(graphics, TOTAL_DRIVERS) as u32,
        active_cars: graphics[ACTIVE_CARS],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header { started_unix_ms: 1_794_250_000_000, hz: 20, label: "offline-ai".into(), app: "0.27.0".into() }
    }

    fn pages(packet: i32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut physics = vec![0u8; PHYSICS_SIZE];
        physics[..4].copy_from_slice(&packet.to_le_bytes());
        physics[SPEED_KMH..SPEED_KMH + 4].copy_from_slice(&123.5f32.to_le_bytes());
        let mut graphics = vec![0u8; GRAPHICS_SIZE];
        graphics[..4].copy_from_slice(&(packet * 2).to_le_bytes());
        graphics[STATUS] = 2;
        (physics, graphics, vec![7u8; STATIC_SIZE])
    }

    fn whole(page: &[u8]) -> Option<Copied<'_>> {
        Some(Copied { page, moved: false })
    }

    #[test]
    fn a_recording_reads_back_what_was_sampled() {
        let mut writer = Writer::new(Vec::new(), &header()).unwrap();
        let (physics, graphics, statics) = pages(1);
        // the game is not running yet
        writer.sample(0, None, None, None).unwrap();
        writer.sample(50_000, None, None, None).unwrap();
        // it publishes; the graphics page moved on during the second copy
        writer.sample(100_000, whole(&physics), whole(&graphics), Some(&statics)).unwrap();
        let (physics2, graphics2, _) = pages(2);
        writer.sample(150_000, whole(&physics2), Some(Copied { page: &graphics2, moved: true }), Some(&statics)).unwrap();
        // a new session: the static page changed
        let statics2 = vec![8u8; STATIC_SIZE];
        writer.sample(200_000, whole(&physics2), whole(&graphics2), Some(&statics2)).unwrap();
        assert_eq!(writer.frames(), 3);

        let bytes = writer.into_inner();
        let reader = Reader::new(bytes.as_slice()).unwrap();
        assert_eq!(reader.header, header());
        let records: Vec<Record> = reader.map(Result::unwrap).collect();
        assert_eq!(records, vec![
            Record::Missing { t_us: 0, mask: NO_PHYSICS | NO_GRAPHICS | NO_STATIC },
            Record::Missing { t_us: 100_000, mask: 0 },
            Record::Static { t_us: 100_000, page: statics },
            Record::Frame { t_us: 100_000, flags: 0, physics, graphics },
            Record::Frame { t_us: 150_000, flags: GRAPHICS_MOVED, physics: physics2.clone(), graphics: graphics2.clone() },
            Record::Static { t_us: 200_000, page: statics2 },
            Record::Frame { t_us: 200_000, flags: 0, physics: physics2, graphics: graphics2 },
        ]);
    }

    #[test]
    fn a_frame_needs_the_physics_and_the_graphics_page_in_their_sizes() {
        let mut writer = Writer::new(Vec::new(), &header()).unwrap();
        let (physics, graphics, statics) = pages(1);
        writer.sample(0, whole(&physics), None, Some(&statics)).unwrap();
        writer.sample(1, whole(&physics[..100]), whole(&graphics), Some(&statics)).unwrap();
        assert_eq!(writer.frames(), 0);
        let bytes = writer.into_inner();
        let records: Vec<Record> = Reader::new(bytes.as_slice()).unwrap().map(Result::unwrap).collect();
        assert_eq!(records, vec![Record::Missing { t_us: 0, mask: NO_GRAPHICS }, Record::Static { t_us: 0, page: statics }, Record::Missing { t_us: 1, mask: NO_PHYSICS }]);
    }

    #[test]
    fn a_recording_cut_short_ends_at_its_last_whole_record() {
        let mut writer = Writer::new(Vec::new(), &header()).unwrap();
        let (physics, graphics, statics) = pages(1);
        for t in 0..3 {
            writer.sample(t, whole(&physics), whole(&graphics), Some(&statics)).unwrap();
        }
        let bytes = writer.into_inner();
        let cut = &bytes[..bytes.len() - 1000];
        let records: Vec<Record> = Reader::new(cut).unwrap().map(Result::unwrap).collect();
        assert_eq!(records.iter().filter(|r| matches!(r, Record::Frame { .. })).count(), 2);
    }

    #[test]
    fn refuses_what_is_not_a_recording() {
        assert!(Reader::new(&b"DDSMREC2\0\0\0\0"[..]).is_err());
        let mut bytes = Writer::new(Vec::new(), &header()).unwrap().into_inner();
        bytes.extend_from_slice(&[9, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(Reader::new(bytes.as_slice()).unwrap().next().unwrap().is_err(), "an unknown record");
    }

    #[test]
    fn a_glance_counts_the_cars_and_finds_the_driver_s_slot() {
        let (physics, mut graphics, _) = pages(5);
        assert_eq!(glance(&physics, &graphics).unwrap().own_slot, None, "no car at all");
        for (slot, id) in [(0usize, [1u8; 16]), (1, [2u8; 16]), (7, [3u8; 16])] {
            graphics[CAR_IDS + slot * 16..CAR_IDS + slot * 16 + 16].copy_from_slice(&id);
        }
        graphics[PLAYER_CAR_ID..PLAYER_CAR_ID + 16].copy_from_slice(&[3u8; 16]);
        graphics[TOTAL_DRIVERS..TOTAL_DRIVERS + 4].copy_from_slice(&11u32.to_le_bytes());
        graphics[ACTIVE_CARS] = 3;
        assert_eq!(glance(&physics, &graphics), Some(Glance { status: 2, physics_packet: 5, graphics_packet: 10, cars: 3, own_slot: Some(7), speed_kmh: 123.5, total_drivers: 11, active_cars: 3 }));
        assert_eq!(glance(&physics[..10], &graphics), None);
    }
}
