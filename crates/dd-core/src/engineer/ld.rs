//! MoTeC i2 `.ld` files as Assetto Corsa EVO writes them (ported from `~/acevo-setups/tools/ld.py`). Bytes 8..16
//! hold the pointers to the channel list and the data; the date (`dd/mm/yyyy`) is at byte 94 and the time
//! (`hh:mm:ss`) at byte 126, both the PC's local time when the game wrote the file. The channels are a linked list
//! of headers (`<IIIIHHHHhhhh>`: previous, next, data pointer, sample count, an id, data type class, data type size,
//! rate in Hz, shift, mul, scale, decimals), the name at +32 and the unit at +72. A value is
//! `raw × 10^-decimals / scale × mul + shift × mul`, raw as i16/i32, or as f16/f32 when the class is 7.
//! A Nordschleife file is 15–70 MB, so only the channels asked for are read, by seeking.

use std::io::{self, Read, Seek, SeekFrom};

/// More channels than any game writes: a list that does not end is a broken file.
const MAX_CHANNELS: usize = 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub name: String,
    pub unit: String,
    pub rate: u16,
    pub count: u32,
    data: u32,
    class: u16,
    size: u16,
    shift: i16,
    mul: i16,
    scale: i16,
    decimals: i16,
}

/// The samples of a channel at its rate; sample `k` is at `k / rate` seconds of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub rate: f64,
    pub values: Vec<f32>,
}

impl Series {
    /// The value at `t` seconds, linearly between samples, held at the ends.
    pub fn at(&self, t: f64) -> f64 {
        let x = t * self.rate;
        let last = self.values.len().saturating_sub(1);
        if self.values.is_empty() {
            return 0.0;
        }
        if x <= 0.0 {
            return self.values[0] as f64;
        }
        let i = x.floor() as usize;
        if i >= last {
            return self.values[last] as f64;
        }
        let f = x - i as f64;
        self.values[i] as f64 * (1.0 - f) + self.values[i + 1] as f64 * f
    }

    /// The indices of the samples within [a, b] seconds.
    pub fn range(&self, a: f64, b: f64) -> std::ops::Range<usize> {
        let lo = ((a * self.rate).ceil().max(0.0) as usize).min(self.values.len());
        let hi = (((b * self.rate).floor() + 1.0).max(0.0) as usize).min(self.values.len());
        lo..hi.max(lo)
    }
}

/// The header of a file: when it was written and its channels.
#[derive(Debug, Clone, PartialEq)]
pub struct Ld {
    pub date: String,
    pub time: String,
    pub channels: Vec<Channel>,
}

fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// An IEEE half float.
fn f16(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((bits >> 10) & 0x1f) as i32;
    let frac = (bits & 0x3ff) as f32;
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 if frac == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

impl Ld {
    pub fn read<R: Read + Seek>(r: &mut R) -> io::Result<Ld> {
        let broken = |what: &str| io::Error::new(io::ErrorKind::InvalidData, format!("not a MoTeC file: {what}"));
        let mut head = [0u8; 142];
        r.seek(SeekFrom::Start(0))?;
        r.read_exact(&mut head)?;
        let len = r.seek(SeekFrom::End(0))?;
        let mut channels = Vec::new();
        let mut at = u32_at(&head, 8);
        while at != 0 {
            if channels.len() >= MAX_CHANNELS || at as u64 + 124 > len {
                return Err(broken("channel list"));
            }
            let mut h = [0u8; 124];
            r.seek(SeekFrom::Start(at as u64))?;
            r.read_exact(&mut h)?;
            let channel = Channel {
                name: text(&h[32..64]),
                unit: text(&h[72..84]),
                rate: u16_at(&h, 22),
                count: u32_at(&h, 12),
                data: u32_at(&h, 8),
                class: u16_at(&h, 18),
                size: u16_at(&h, 20),
                shift: i16_at(&h, 24),
                mul: i16_at(&h, 26),
                scale: i16_at(&h, 28),
                decimals: i16_at(&h, 30),
            };
            if channel.data as u64 + channel.count as u64 * channel.size as u64 > len {
                return Err(broken(&format!("channel {} runs past the end", channel.name)));
            }
            channels.push(channel);
            at = u32_at(&h, 4);
        }
        Ok(Ld { date: text(&head[94..110]), time: text(&head[126..142]), channels })
    }

    pub fn channel(&self, name: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.name == name)
    }

    /// Seconds of data: the sample count of SPEED by its rate.
    pub fn duration(&self) -> Option<f64> {
        self.channel("SPEED").filter(|c| c.rate > 0).map(|c| c.count as f64 / c.rate as f64)
    }

    /// When the game wrote the file, as seconds since 1970 of the local wall clock read as if it were UTC.
    pub fn written_local(&self) -> Option<i64> {
        let d: Vec<i64> = self.date.split('/').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        let t: Vec<i64> = self.time.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        match (d.as_slice(), t.as_slice()) {
            ([day, month, year], [h, m, s]) if (1..=12).contains(month) && (1..=31).contains(day) => {
                Some(super::clock::days_from_civil(*year, *month, *day) * 86_400 + h * 3600 + m * 60 + s)
            }
            _ => None,
        }
    }

    /// The values of channel `name`, None when the file has no such channel or a type this does not read.
    pub fn values<R: Read + Seek>(&self, r: &mut R, name: &str) -> io::Result<Option<Series>> {
        let Some(c) = self.channel(name).filter(|c| c.rate > 0) else { return Ok(None) };
        let mut raw = vec![0u8; c.count as usize * c.size as usize];
        r.seek(SeekFrom::Start(c.data as u64))?;
        r.read_exact(&mut raw)?;
        let k = 10f64.powi(-(c.decimals as i32)) / (if c.scale == 0 { 1.0 } else { c.scale as f64 }) * (if c.mul == 0 { 1.0 } else { c.mul as f64 });
        let shift = c.shift as f64 * if c.mul == 0 { 1.0 } else { c.mul as f64 };
        let decode: fn(&[u8]) -> f64 = match (c.class == 7, c.size) {
            (true, 4) => |b| f32::from_le_bytes(b.try_into().unwrap()) as f64,
            (true, 2) => |b| f16(u16::from_le_bytes(b.try_into().unwrap())) as f64,
            (false, 4) => |b| i32::from_le_bytes(b.try_into().unwrap()) as f64,
            (false, 2) => |b| i16::from_le_bytes(b.try_into().unwrap()) as f64,
            _ => return Ok(None),
        };
        let values = raw.chunks_exact(c.size as usize).map(|b| (decode(b) * k + shift) as f32).collect();
        Ok(Some(Series { rate: c.rate as f64, values }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{chan, write_ld, Chan};
    use super::*;
    use std::io::Cursor;

    fn file() -> Vec<u8> {
        write_ld("04/10/2026", "23:40:23", &[
            chan("SPEED", "m/s", 60, (0..600).map(|i| i as f32 * 0.1).collect()),
            chan("G_LAT", "m/s2", 20, vec![0.5, -1.25, 2.0]),
            Chan { name: "TYRE_PRESS_LF", unit: "psi", rate: 20, values: vec![23.7, 27.33], i16_dec: Some(2) },
        ])
    }

    #[test]
    fn reads_the_header_and_the_channel_list() {
        let ld = Ld::read(&mut Cursor::new(file())).unwrap();
        assert_eq!((ld.date.as_str(), ld.time.as_str()), ("04/10/2026", "23:40:23"));
        let names: Vec<&str> = ld.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["SPEED", "G_LAT", "TYRE_PRESS_LF"]);
        assert_eq!((ld.channels[0].unit.as_str(), ld.channels[0].rate, ld.channels[0].count), ("m/s", 60, 600));
        assert_eq!(ld.duration(), Some(10.0));
        // 2026-10-04 23:40:23 read as UTC
        assert_eq!(ld.written_local(), Some(1_791_157_223));
    }

    #[test]
    fn reads_f32_and_scaled_i16_channels() {
        let bytes = file();
        let mut r = Cursor::new(bytes);
        let ld = Ld::read(&mut r).unwrap();
        assert_eq!(ld.values(&mut r, "G_LAT").unwrap().unwrap(), Series { rate: 20.0, values: vec![0.5, -1.25, 2.0] });
        let psi = ld.values(&mut r, "TYRE_PRESS_LF").unwrap().unwrap();
        assert!((psi.values[0] - 23.7).abs() < 1e-5 && (psi.values[1] - 27.33).abs() < 1e-5, "{psi:?}");
        assert_eq!(ld.values(&mut r, "FUEL").unwrap(), None);
        let speed = ld.values(&mut r, "SPEED").unwrap().unwrap();
        assert!((speed.at(1.0) - 6.0).abs() < 1e-5, "sample 60");
        assert!((speed.at(1.0 + 1.0 / 120.0) - 6.05).abs() < 1e-5, "between samples");
        assert_eq!((speed.at(-1.0), speed.at(100.0)), (0.0, speed.values[599] as f64));
        assert_eq!(speed.range(1.0, 2.0), 60..121);
    }

    #[test]
    fn decodes_half_floats_and_scale_mul_shift() {
        assert_eq!((f16(0x3c00), f16(0xc000), f16(0x3555), f16(0)), (1.0, -2.0, 0.333_251_95, 0.0));
        let mut bytes = file();
        // G_LAT's header: shift 2, mul 3, scale 2 → raw × 1 / 2 × 3 + 2 × 3
        let at = 256 + 124;
        bytes[at + 24..at + 26].copy_from_slice(&2i16.to_le_bytes());
        bytes[at + 26..at + 28].copy_from_slice(&3i16.to_le_bytes());
        bytes[at + 28..at + 30].copy_from_slice(&2i16.to_le_bytes());
        let mut r = Cursor::new(bytes);
        let ld = Ld::read(&mut r).unwrap();
        assert_eq!(ld.values(&mut r, "G_LAT").unwrap().unwrap().values, vec![6.75, 4.125, 9.0]);
    }

    #[test]
    fn refuses_a_broken_file() {
        let mut bytes = file();
        bytes.truncate(2000);
        assert!(Ld::read(&mut Cursor::new(bytes)).is_err(), "data past the end");
        let mut looped = file();
        let at = 256 + 2 * 124;
        looped[at + 4..at + 8].copy_from_slice(&256u32.to_le_bytes());
        assert!(Ld::read(&mut Cursor::new(looped)).is_err(), "a list that does not end");
        assert!(Ld::read(&mut Cursor::new(vec![0u8; 50])).is_err(), "shorter than a header");
    }
}
