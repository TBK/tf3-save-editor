//! Reading and modifying Transport Fever 3 save games (`*.sav`).
//!
//! A save is a zstd stream holding one sequential binary blob: the
//! [`header`](header::Header) (summary, preview image, game configuration)
//! followed by the world. The world has no offset tables or section sizes, so
//! sections that this crate understands can be re-serialised and spliced back
//! between the untouched bytes around them.
//!
//! Understood sections:
//! * the header, parsed in full;
//! * the player's [`journal`](journal::Journal), which defines the money;
//! * the [game script states](scripts::ScriptStates).

pub mod header;
pub mod io;
pub mod journal;
pub mod lua;
pub mod scripts;

use std::path::Path;

use header::{Header, STATS_MONEY_INDEX};
use io::{Reader, Writer};
use journal::Journal;
use scripts::ScriptStates;

/// Save format version this crate was written against.
pub const KNOWN_VERSION: u32 = 604;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a Transport Fever 3 save: {0}")]
    NotASave(String),
    #[error("unexpected end of data at offset {offset:#x} (wanted {wanted} bytes)")]
    UnexpectedEof { offset: usize, wanted: usize },
    #[error("malformed data at offset {offset:#x}: {what}")]
    Malformed { offset: usize, what: String },
    #[error("{0}")]
    Invalid(String),
    #[error("{0} not found in this save")]
    Missing(&'static str),
    #[error("image encoding failed: {0}")]
    Image(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A loaded save. Edit the public fields, then call [`SaveFile::to_bytes`].
pub struct SaveFile {
    pub header: Header,
    pub journal: Option<Journal>,
    pub scripts: Option<ScriptStates>,
    raw: Vec<u8>,
    header_len: usize,
    journal_span: Option<(usize, usize)>,
    scripts_span: Option<(usize, usize)>,
    /// Number of journals that matched the balance; more than one means the
    /// money section is ambiguous and is left read-only.
    pub journal_matches: usize,
}

impl SaveFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_compressed(&std::fs::read(path)?)
    }

    pub fn from_compressed(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 4 || bytes[..4] != [0x28, 0xb5, 0x2f, 0xfd] {
            return Err(Error::NotASave("not zstd-compressed".into()));
        }
        let raw = zstd::stream::decode_all(bytes)?;
        Self::from_raw(raw)
    }

    /// Parses an already decompressed save.
    pub fn from_raw(raw: Vec<u8>) -> Result<Self> {
        let mut r = Reader::new(&raw);
        let header = Header::read(&mut r)?;
        let header_len = r.pos();

        let mut journals = Journal::find_with_balance(&raw, header_len, header.money);
        let journal_matches = journals.len();
        let (journal, journal_span) = if journal_matches == 1 {
            let (at, j) = journals.pop().expect("one match");
            let len = 4 + j.entries.len() * journal::ENTRY_SIZE + 8;
            (Some(j), Some((at, len)))
        } else {
            (None, None)
        };

        let (scripts, scripts_span) = match ScriptStates::locate(&raw, header_len) {
            Some((at, s, len)) => (Some(s), Some((at, len))),
            None => (None, None),
        };

        if let (Some((ja, jl)), Some((sa, sl))) = (journal_span, scripts_span)
            && ja < sa + sl
            && sa < ja + jl
        {
            return Err(Error::Malformed { offset: ja, what: "journal overlaps script states".into() });
        }

        Ok(Self { header, journal, scripts, raw, header_len, journal_span, scripts_span, journal_matches })
    }

    /// Current money (the journal balance, or the header copy if the journal
    /// was not found).
    pub fn money(&self) -> i64 {
        self.journal.as_ref().map_or(self.header.money, |j| j.balance)
    }

    /// Sets the player's money by booking an "Other" journal entry and
    /// updating the cached copies in the header.
    pub fn set_money(&mut self, value: i64) -> Result<()> {
        let journal = self.journal.as_mut().ok_or(Error::Missing("player journal"))?;
        let old = journal.balance;
        journal.set_balance(value)?;
        if self.header.money == old {
            self.header.money = value;
        }
        if self.header.stats_i64[STATS_MONEY_INDEX] == old {
            self.header.stats_i64[STATS_MONEY_INDEX] = value;
        }
        Ok(())
    }

    /// Decompressed save with all edits applied.
    pub fn to_raw(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.header.write(&mut w);

        // Splice edited sections back in offset order.
        let mut sections: Vec<(usize, usize, Writer)> = Vec::new();
        if let (Some(j), Some((at, len))) = (&self.journal, self.journal_span) {
            let mut sw = Writer::new();
            j.write(&mut sw);
            sections.push((at, len, sw));
        }
        if let (Some(s), Some((at, len))) = (&self.scripts, self.scripts_span) {
            let mut sw = Writer::new();
            s.write(&mut sw);
            sections.push((at, len, sw));
        }
        sections.sort_by_key(|s| s.0);

        let mut pos = self.header_len;
        for (at, len, sw) in sections {
            w.bytes(&self.raw[pos..at]);
            w.bytes(&sw.buf);
            pos = at + len;
        }
        w.bytes(&self.raw[pos..]);
        w.buf
    }

    /// Compressed save, ready to be written to disk.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        compress(&self.to_raw())
    }

    /// Writes the save atomically: to a temporary file next to `path`, then
    /// renamed over it.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let bytes = self.to_bytes()?;
        let tmp = path.with_extension("sav.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn raw_len(&self) -> usize {
        self.raw.len()
    }
}

/// Compresses like the game does: one data frame followed by an empty frame.
pub fn compress(raw: &[u8]) -> Result<Vec<u8>> {
    let mut out = zstd::stream::encode_all(raw, 3)?;
    out.extend(zstd::stream::encode_all(&[][..], 3)?);
    Ok(out)
}

/// Writes a backup copy (`<name>.sav.bak`, or `.bak2`, … if taken) and
/// returns its path.
pub fn backup(path: &Path) -> Result<std::path::PathBuf> {
    for i in 1.. {
        let ext = if i == 1 { "sav.bak".to_string() } else { format!("sav.bak{i}") };
        let candidate = path.with_extension(ext);
        if !candidate.exists() {
            std::fs::copy(path, &candidate)?;
            return Ok(candidate);
        }
    }
    unreachable!()
}
