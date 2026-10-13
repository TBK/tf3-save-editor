//! The savegame header: summary info shown in the load dialog, the preview
//! image and the game configuration (climate, economy, advanced options).
//!
//! Fields are listed in on-disk order. Fields whose meaning
//! is not known are kept under neutral names so they round-trip untouched.

use crate::io::{Reader, Writer};
use crate::lua::{LuaString, Table};
use crate::{Error, MIN_VERSION, Result};

pub const MAGIC: &[u8; 4] = b"tf**";

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// Save format version (604 for the game build this was written against).
    pub version: u32,
    pub unknown_a: u32,
    pub unknown_b: [u32; 2],
    /// The date shown in the game, as a Julian day number (2415021 is
    /// 1 January 1900). See [`Header::calendar_date`].
    pub date: u32,
    /// Player balance as shown in the load dialog. Mirrors the journal balance.
    pub money: i64,
    /// Copy of the company's experience points, which grow with play. Not
    /// the calendar year. It can lag the script state: usually by a few
    /// points, by 242 in one catalog save.
    pub experience: u32,
    pub info_table: Table,
    pub mods: Vec<ModEntry>,
    pub thumbnail: Option<Thumbnail>,
    /// Summary statistics. Index 3 mirrors the balance.
    pub stats_i64: [i64; 6],
    pub stats_i32: [i32; 11],
    pub stats_pairs: Vec<(u32, u32)>,
    pub stats_u32: [u32; 2],
    pub stats2_i64: [i64; 9],
    pub stats2_u32: [u32; 2],
    pub labels: Vec<(LuaString, u32)>,
    pub stats3_u32: u32,
    pub stats3_i64: [i64; 2],
    pub unknown_d: u32,
    pub config: GameConfig,
}

/// Index of the balance inside [`Header::stats_i64`].
pub const STATS_MONEY_INDEX: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub struct ModEntry {
    pub id: LuaString,
    pub source: LuaString,
    pub path: LuaString,
    pub name: LuaString,
    pub extra: LuaString,
    pub flags: u32,
}

/// Raw RGB8 preview, bottom-up rows (the last row is the top of the picture).
///
/// Kept exactly as read: `rgb` is not checked against the size until
/// [`Thumbnail::to_png`].
#[derive(Clone, PartialEq)]
pub struct Thumbnail {
    /// Byte before the size. Meaning unknown: real saves have 0 or 1, both
    /// with a full preview, so it does not say whether an image follows. In
    /// every real save checked it equals [`GameConfig::flag`].
    pub flag: u8,
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl std::fmt::Debug for Thumbnail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Thumbnail({}x{}, {} bytes)", self.width, self.height, self.rgb.len())
    }
}

impl Thumbnail {
    /// Encodes the preview as an upright PNG (useful for UIs and exports).
    pub fn to_png(&self) -> Result<Vec<u8>> {
        if self.rgb.len() != self.width as usize * self.height as usize * 3 {
            return Err(Error::Malformed { offset: 0, what: "thumbnail is not RGB8".into() });
        }
        // PNG rows are top-down. `max(1)` only avoids a panic: a 0-wide image has no bytes.
        let row = (self.width as usize * 3).max(1);
        let upright: Vec<u8> = self.rgb.chunks_exact(row).rev().flatten().copied().collect();
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.width, self.height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| Error::Image(e.to_string()))?;
        w.write_image_data(&upright).map_err(|e| Error::Image(e.to_string()))?;
        w.finish().map_err(|e| Error::Image(e.to_string()))?;
        Ok(out)
    }
}

/// Settings chosen when the game was created.
#[derive(Clone, Debug, PartialEq)]
pub struct GameConfig {
    pub mod_ids: Vec<LuaString>,
    /// e.g. `climate`, `economy`, `nameList` → resource paths.
    pub resources: Vec<(LuaString, LuaString)>,
    /// Per-mod parameter tables. The entry with an empty key holds the base
    /// game settings (`advancedOptions.*`, `townConfig.*`, …).
    pub params: Vec<(LuaString, Table)>,
    pub mission: LuaString,
    pub kind: LuaString,
    pub flag: u8,
    pub value: u32,
    pub id: LuaString,
}

impl GameConfig {
    /// The base game settings table (empty key), if present.
    pub fn settings(&self) -> Option<&Table> {
        self.params.iter().find(|(k, _)| k.0.is_empty()).map(|(_, t)| t)
    }

    pub fn settings_mut(&mut self) -> Option<&mut Table> {
        self.params.iter_mut().find(|(k, _)| k.0.is_empty()).map(|(_, t)| t)
    }
}

impl Header {
    pub fn title(&self) -> String {
        self.mods.first().map(|m| m.name.to_string()).unwrap_or_default()
    }

    /// [`Self::date`] as a Gregorian `(year, month, day)`.
    pub fn calendar_date(&self) -> (i64, u32, u32) {
        civil_from_julian_day(self.date)
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self> {
        if r.bytes(4)? != MAGIC {
            return Err(Error::NotASave("missing tf** magic".into()));
        }
        let version = r.u32()?;
        if version < MIN_VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        let unknown_a = r.u32()?;
        let unknown_b = [r.u32()?, r.u32()?];
        let date = r.u32()?;
        let money = r.i64()?;
        let experience = r.u32()?;
        let info_table = Table::read(r)?;
        let n = r.count(24)?;
        let mut mods = Vec::with_capacity(n);
        for _ in 0..n {
            mods.push(ModEntry {
                id: LuaString(r.string()?),
                source: LuaString(r.string()?),
                path: LuaString(r.string()?),
                name: LuaString(r.string()?),
                extra: LuaString(r.string()?),
                flags: r.u32()?,
            });
        }
        let thumbnail = read_thumbnail(r)?;
        let mut stats_i64 = [0; 6];
        for v in &mut stats_i64 {
            *v = r.i64()?;
        }
        let mut stats_i32 = [0; 11];
        for v in &mut stats_i32 {
            *v = r.i32()?;
        }
        let n = r.count(8)?;
        let mut stats_pairs = Vec::with_capacity(n);
        for _ in 0..n {
            stats_pairs.push((r.u32()?, r.u32()?));
        }
        let stats_u32 = [r.u32()?, r.u32()?];
        let mut stats2_i64 = [0; 9];
        for v in &mut stats2_i64 {
            *v = r.i64()?;
        }
        let stats2_u32 = [r.u32()?, r.u32()?];
        let n = r.count(8)?;
        let mut labels = Vec::with_capacity(n);
        for _ in 0..n {
            labels.push((LuaString(r.string()?), r.u32()?));
        }
        let stats3_u32 = r.u32()?;
        let stats3_i64 = [r.i64()?, r.i64()?];
        let unknown_d = r.u32()?;
        let config = GameConfig::read(r)?;
        Ok(Self {
            version,
            unknown_a,
            unknown_b,
            date,
            money,
            experience,
            info_table,
            mods,
            thumbnail,
            stats_i64,
            stats_i32,
            stats_pairs,
            stats_u32,
            stats2_i64,
            stats2_u32,
            labels,
            stats3_u32,
            stats3_i64,
            unknown_d,
            config,
        })
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.bytes(MAGIC);
        w.u32(self.version);
        w.u32(self.unknown_a);
        w.u32(self.unknown_b[0]);
        w.u32(self.unknown_b[1]);
        w.u32(self.date);
        w.i64(self.money);
        w.u32(self.experience);
        self.info_table.write(w);
        w.count(self.mods.len());
        for m in &self.mods {
            for s in [&m.id, &m.source, &m.path, &m.name, &m.extra] {
                w.string(&s.0);
            }
            w.u32(m.flags);
        }
        write_thumbnail(self.thumbnail.as_ref(), w);
        self.stats_i64.iter().for_each(|v| w.i64(*v));
        self.stats_i32.iter().for_each(|v| w.i32(*v));
        w.count(self.stats_pairs.len());
        for (a, b) in &self.stats_pairs {
            w.u32(*a);
            w.u32(*b);
        }
        self.stats_u32.iter().for_each(|v| w.u32(*v));
        self.stats2_i64.iter().for_each(|v| w.i64(*v));
        self.stats2_u32.iter().for_each(|v| w.u32(*v));
        w.count(self.labels.len());
        for (s, v) in &self.labels {
            w.string(&s.0);
            w.u32(*v);
        }
        w.u32(self.stats3_u32);
        self.stats3_i64.iter().for_each(|v| w.i64(*v));
        w.u32(self.unknown_d);
        self.config.write(w);
    }
}

/// Reads the preview section. Only the bytes [`write_thumbnail`] emits for
/// `None` read back as `None`; anything else is kept as read, so every save
/// round-trips.
fn read_thumbnail(r: &mut Reader<'_>) -> Result<Option<Thumbnail>> {
    let flag = r.u8()?;
    let width = r.u32()?;
    let height = r.u32()?;
    let rgb = r.string()?;
    let empty = flag == 0 && width == 0 && height == 0 && rgb.is_empty();
    Ok((!empty).then_some(Thumbnail { flag, width, height, rgb }))
}

fn write_thumbnail(t: Option<&Thumbnail>, w: &mut Writer) {
    match t {
        Some(t) => {
            w.u8(t.flag);
            w.u32(t.width);
            w.u32(t.height);
            w.string(&t.rgb);
        }
        None => {
            w.u8(0);
            w.u32(0);
            w.u32(0);
            w.u32(0);
        }
    }
}

impl GameConfig {
    fn read(r: &mut Reader<'_>) -> Result<Self> {
        let n = r.count(4)?;
        let mod_ids = (0..n).map(|_| r.string().map(LuaString)).collect::<Result<_>>()?;
        let n = r.count(8)?;
        let mut resources = Vec::with_capacity(n);
        for _ in 0..n {
            resources.push((LuaString(r.string()?), LuaString(r.string()?)));
        }
        let n = r.count(8)?;
        let mut params = Vec::with_capacity(n);
        for _ in 0..n {
            params.push((LuaString(r.string()?), Table::read(r)?));
        }
        Ok(Self {
            mod_ids,
            resources,
            params,
            mission: LuaString(r.string()?),
            kind: LuaString(r.string()?),
            flag: r.u8()?,
            value: r.u32()?,
            id: LuaString(r.string()?),
        })
    }

    fn write(&self, w: &mut Writer) {
        w.count(self.mod_ids.len());
        self.mod_ids.iter().for_each(|s| w.string(&s.0));
        w.count(self.resources.len());
        for (k, v) in &self.resources {
            w.string(&k.0);
            w.string(&v.0);
        }
        w.count(self.params.len());
        for (k, t) in &self.params {
            w.string(&k.0);
            t.write(w);
        }
        w.string(&self.mission.0);
        w.string(&self.kind.0);
        w.u8(self.flag);
        w.u32(self.value);
        w.string(&self.id.0);
    }
}

/// Julian day number to Gregorian `(year, month, day)` (Fliegel and
/// Van Flandern).
fn civil_from_julian_day(jd: u32) -> (i64, u32, u32) {
    let a = jd as i64 + 32044;
    let b = (4 * a + 3) / 146097;
    let c = a - 146097 * b / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - 1461 * d / 4;
    let m = (5 * e + 2) / 153;
    let day = e - (153 * m + 2) / 5 + 1;
    let month = m + 3 - 12 * (m / 10);
    let year = 100 * b + d - 4800 + m / 10;
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A preview section built byte by byte, independent of `write_thumbnail`.
    fn section(flag: u8, width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(flag);
        w.u32(width);
        w.u32(height);
        w.string(rgb);
        w.buf
    }

    /// Reads a preview section, checks it writes back unchanged and returns it.
    fn roundtrip(bytes: &[u8]) -> Option<Thumbnail> {
        let mut r = Reader::new(bytes);
        let t = read_thumbnail(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        let mut w = Writer::new();
        write_thumbnail(t.as_ref(), &mut w);
        assert_eq!(w.buf, bytes, "preview section not written back as read");
        t
    }

    fn sample() -> Header {
        Header {
            version: 604,
            unknown_a: 0,
            unknown_b: [0; 2],
            date: 0,
            money: 0,
            experience: 0,
            info_table: Table::default(),
            mods: Vec::new(),
            thumbnail: None,
            stats_i64: [0; 6],
            stats_i32: [0; 11],
            stats_pairs: Vec::new(),
            stats_u32: [0; 2],
            stats2_i64: [0; 9],
            stats2_u32: [0; 2],
            labels: Vec::new(),
            stats3_u32: 0,
            stats3_i64: [0; 2],
            unknown_d: 0,
            config: GameConfig {
                mod_ids: Vec::new(),
                resources: Vec::new(),
                params: Vec::new(),
                mission: LuaString::default(),
                kind: LuaString::default(),
                flag: 0,
                value: 0,
                id: LuaString::default(),
            },
        }
    }

    /// Just the magic and a version, so only the version check can answer.
    fn magic_and_version(version: u32) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(MAGIC);
        w.u32(version);
        w.buf
    }

    #[test]
    fn versions_before_599_are_rejected_up_front() {
        for v in [0, 543, 568, 585, MIN_VERSION - 1] {
            let bytes = magic_and_version(v);
            match Header::read(&mut Reader::new(&bytes)) {
                Err(Error::UnsupportedVersion(got)) => assert_eq!(got, v),
                other => panic!("version {v}: expected UnsupportedVersion, got {other:?}"),
            }
        }
        let msg = Error::UnsupportedVersion(568).to_string();
        assert_eq!(msg, "unsupported format version 568 (this editor reads version 599 and newer)");
    }

    #[test]
    fn versions_from_599_pass_the_check() {
        // Cut off after the version: anything but UnsupportedVersion will do.
        for v in [MIN_VERSION, 601, 604, 700] {
            let bytes = magic_and_version(v);
            let err = Header::read(&mut Reader::new(&bytes)).unwrap_err();
            assert!(!matches!(err, Error::UnsupportedVersion(_)), "version {v}: {err}");
        }
    }

    #[test]
    fn preview_with_flag_0_is_kept() {
        let rgb: Vec<u8> = (0..12).collect();
        let t = roundtrip(&section(0, 2, 2, &rgb)).expect("preview read");
        assert_eq!((t.flag, t.width, t.height), (0, 2, 2));
        assert_eq!(t.rgb, rgb);
        assert!(t.to_png().is_ok());
    }

    #[test]
    fn preview_with_flag_1_is_kept() {
        let t = roundtrip(&section(1, 2, 2, &[7; 12])).expect("preview read");
        assert_eq!(t.flag, 1);
    }

    #[test]
    fn empty_preview_reads_as_none() {
        assert!(roundtrip(&section(0, 0, 0, &[])).is_none());
    }

    #[test]
    fn preview_of_wrong_length_is_kept_but_not_encoded() {
        let t = roundtrip(&section(0, 2, 2, &[7; 5])).expect("preview read");
        assert_eq!(t.rgb.len(), 5);
        assert!(t.to_png().is_err());
    }

    #[test]
    fn png_is_flipped_upright() {
        // Stored bottom-up: a blue bottom row, then a red top row.
        let t = Thumbnail { flag: 0, width: 1, height: 2, rgb: vec![0, 0, 255, 255, 0, 0] };
        let png = t.to_png().unwrap();
        let mut reader = png::Decoder::new(&png[..]).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height, info.color_type), (1, 2, png::ColorType::Rgb));
        assert_eq!(&buf[..info.buffer_size()], [255, 0, 0, 0, 0, 255]);
    }

    #[test]
    fn header_roundtrip_keeps_preview() {
        for flag in [0, 1] {
            let mut h = sample();
            h.thumbnail = Some(Thumbnail { flag, width: 2, height: 1, rgb: vec![1, 2, 3, 4, 5, 6] });
            let mut w = Writer::new();
            h.write(&mut w);
            let mut r = Reader::new(&w.buf);
            assert_eq!(Header::read(&mut r).unwrap(), h);
            assert_eq!(r.remaining(), 0);
        }
    }

    #[test]
    fn julian_day_to_date() {
        assert_eq!(civil_from_julian_day(2415021), (1900, 1, 1));
        // Dates read off the game's screen for three saves.
        assert_eq!(civil_from_julian_day(2451652), (2000, 4, 17));
        assert_eq!(civil_from_julian_day(2428583), (1937, 2, 18));
        assert_eq!(civil_from_julian_day(2521351), (2191, 2, 14));
    }

    #[test]
    fn julian_day_month_and_leap_year_edges() {
        // 1900 is not a leap year, 2000 is.
        assert_eq!(civil_from_julian_day(2415079), (1900, 2, 28));
        assert_eq!(civil_from_julian_day(2415080), (1900, 3, 1));
        assert_eq!(civil_from_julian_day(2451604), (2000, 2, 29));
        assert_eq!(civil_from_julian_day(2451544), (1999, 12, 31));
        assert_eq!(civil_from_julian_day(2451545), (2000, 1, 1));
        // Latest date seen in a catalog save.
        assert_eq!(civil_from_julian_day(2835101), (3050, 2, 21));
    }
}
