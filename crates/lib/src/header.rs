//! The savegame header: summary info shown in the load dialog, the preview
//! image and the game configuration (climate, economy, advanced options).
//!
//! Fields are listed in on-disk order. Fields whose meaning
//! is not known are kept under neutral names so they round-trip untouched.

use crate::io::{Reader, Writer};
use crate::lua::{LuaString, Table};
use crate::{Error, Result};

pub const MAGIC: &[u8; 4] = b"tf**";

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// Save format version (604 for the game build this was written against).
    pub version: u32,
    pub unknown_a: u32,
    pub unknown_b: [u32; 2],
    pub unknown_c: u32,
    /// Player balance as shown in the load dialog. Mirrors the journal balance.
    pub money: i64,
    /// In-game calendar year.
    pub year: u32,
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

/// Raw RGB8 preview, top-down rows.
#[derive(Clone, PartialEq)]
pub struct Thumbnail {
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
    /// Encodes the preview as PNG (useful for UIs and exports).
    pub fn to_png(&self) -> Result<Vec<u8>> {
        if self.rgb.len() != self.width as usize * self.height as usize * 3 {
            return Err(Error::Malformed { offset: 0, what: "thumbnail is not RGB8".into() });
        }
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.width, self.height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| Error::Image(e.to_string()))?;
        w.write_image_data(&self.rgb).map_err(|e| Error::Image(e.to_string()))?;
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

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self> {
        if r.bytes(4)? != MAGIC {
            return Err(Error::NotASave("missing tf** magic".into()));
        }
        let version = r.u32()?;
        let unknown_a = r.u32()?;
        let unknown_b = [r.u32()?, r.u32()?];
        let unknown_c = r.u32()?;
        let money = r.i64()?;
        let year = r.u32()?;
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
        let has_thumb = r.u8()?;
        let width = r.u32()?;
        let height = r.u32()?;
        let rgb = r.string()?;
        let thumbnail = (has_thumb != 0).then_some(Thumbnail { width, height, rgb });
        if has_thumb == 0 && (width != 0 || height != 0) {
            return Err(Error::Malformed { offset: r.pos(), what: "thumbnail size without image".into() });
        }
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
            unknown_c,
            money,
            year,
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
        w.u32(self.unknown_c);
        w.i64(self.money);
        w.u32(self.year);
        self.info_table.write(w);
        w.count(self.mods.len());
        for m in &self.mods {
            for s in [&m.id, &m.source, &m.path, &m.name, &m.extra] {
                w.string(&s.0);
            }
            w.u32(m.flags);
        }
        match &self.thumbnail {
            Some(t) => {
                w.u8(1);
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
