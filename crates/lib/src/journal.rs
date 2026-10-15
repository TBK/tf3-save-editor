//! The player's financial journal.
//!
//! The engine keeps one journal per entity: a time-ordered list of booked
//! amounts, followed by a cached balance. The player's money is the sum of
//! the amounts; the cached balance (which the header copies) differs from it
//! in some saves. Changing money means booking an entry.
//!
//! Layout: `u32 count`, then `count` × (`i64 time`, `i64 amount`,
//! 5 × `u8` category), then `i64 balance`.
//!
//! The player's journal sits at a fixed place in the entity data: after the
//! edge-addon name table (see `docs/FORMAT.md`, "Where the journal is").

use crate::io::{Reader, Writer};
use crate::{Error, Result};

pub const ENTRY_SIZE: usize = 21;

/// Resource path of the base game's last edge-addon entry in the name table.
const EDGE_TABLE_ANCHOR: &[u8] = b"edge_addons/barrier_track_b.edge";

/// Longest `vec<u32>` accepted between the name table and the journal.
const MAX_IDS: u32 = 1 << 20;

/// How far past the usual place a journal start is looked for, in bytes.
const LATE_START_WINDOW: usize = 64;

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// End of the length-prefixed printable ASCII string at `at`.
fn skip_text(data: &[u8], at: usize) -> Option<usize> {
    let len = read_u32(data, at)? as usize;
    let end = at.checked_add(4 + len)?;
    (len <= 512 && data.get(at + 4..end)?.iter().all(|b| (0x20..0x7f).contains(b))).then_some(end)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Category {
    pub kind: u8,
    pub construction: u8,
    pub maintenance: u8,
    pub other: u8,
    pub carrier: u8,
}

impl Category {
    /// `Type.OTHER` with every sub-category set to OTHER, the category used
    /// for the starting capital.
    pub const OTHER: Category = Category { kind: 6, construction: 6, maintenance: 2, other: 0, carrier: 3 };

    fn from_bytes(b: &[u8]) -> Self {
        Self { kind: b[0], construction: b[1], maintenance: b[2], other: b[3], carrier: b[4] }
    }

    fn to_bytes(self) -> [u8; 5] {
        [self.kind, self.construction, self.maintenance, self.other, self.carrier]
    }

    /// Whether each byte is inside its enum's range in the engine.
    pub fn is_valid(self) -> bool {
        self.kind <= 7 && self.construction <= 7 && self.maintenance <= 3 && self.other == 0 && self.carrier <= 5
    }

    pub fn type_name(self) -> &'static str {
        match self.kind {
            0 => "Loan",
            1 => "Interest",
            2 => "Construction",
            3 => "Vehicle purchase",
            4 => "Maintenance",
            5 => "Income",
            6 => "Other",
            7 => "Subsidy",
            _ => "?",
        }
    }

    pub fn carrier_name(self) -> &'static str {
        match self.carrier {
            0 => "Road",
            1 => "Rail",
            2 => "Tram",
            3 => "General",
            4 => "Air",
            5 => "Water",
            _ => "?",
        }
    }

    pub fn label(self) -> String {
        let detail = match self.kind {
            2 => match self.construction {
                0 => Some("streets"),
                1 => Some("tracks"),
                2 => Some("signals"),
                3 => Some("stations"),
                4 => Some("depots"),
                5 => Some("bulldozer"),
                7 => Some("warehouses"),
                _ => None,
            },
            4 => match self.maintenance {
                0 => Some("vehicle running costs"),
                1 => Some("infrastructure"),
                3 => Some("vehicle maintenance"),
                _ => None,
            },
            _ => None,
        };
        match (detail, self.carrier) {
            (Some(d), 3) => format!("{} – {d}", self.type_name()),
            (Some(d), _) => format!("{} – {d} ({})", self.type_name(), self.carrier_name()),
            (None, 3) => self.type_name().to_string(),
            (None, _) => format!("{} ({})", self.type_name(), self.carrier_name()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Game time in milliseconds.
    pub time: i64,
    pub amount: i64,
    pub category: Category,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Journal {
    pub entries: Vec<Entry>,
    /// The stored balance. Equal to [`Journal::sum`] in most saves.
    pub balance: i64,
}

impl Journal {
    /// Sum of all booked amounts.
    pub fn sum(&self) -> i128 {
        self.entries.iter().map(|e| e.amount as i128).sum()
    }

    /// The player's money: [`Journal::sum`], limited to the `i64` range.
    pub fn money(&self) -> i64 {
        self.sum().clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// Books an `Other` entry so the money becomes `target`, and stores
    /// `target` as the balance. The entry is stamped with the latest time
    /// already in the journal, which keeps the list ordered as the engine
    /// requires.
    pub fn set_balance(&mut self, target: i64) -> Result<()> {
        let delta = (target as i128) - self.sum();
        if delta != 0 {
            let amount = i64::try_from(delta).map_err(|_| Error::Invalid("money change too large".into()))?;
            let time = self.entries.last().map_or(0, |e| e.time);
            self.entries.push(Entry { time, amount, category: Category::OTHER });
        }
        self.balance = target;
        Ok(())
    }

    /// Validating parse. Used both for loading and for locating journals. The
    /// stored balance is read as it is and not compared with the sum.
    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self> {
        let start = r.pos();
        let n = r.count(ENTRY_SIZE)?;
        let mut entries = Vec::with_capacity(n);
        let mut last_time = i64::MIN;
        for _ in 0..n {
            let time = r.i64()?;
            let amount = r.i64()?;
            let category = Category::from_bytes(r.bytes(5)?);
            if !category.is_valid() || time < last_time || time < 0 {
                return Err(Error::Malformed { offset: start, what: "not a journal".into() });
            }
            last_time = time;
            entries.push(Entry { time, amount, category });
        }
        let balance = r.i64()?;
        Ok(Self { entries, balance })
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.count(self.entries.len());
        for e in &self.entries {
            w.i64(e.time);
            w.i64(e.amount);
            w.bytes(&e.category.to_bytes());
        }
        w.i64(self.balance);
    }

    /// Finds every non-empty journal in `data[from..]` whose cached balance
    /// equals `balance` and equals the sum of its amounts. Returns
    /// `(offset, journal)` pairs.
    pub(crate) fn find_with_balance(data: &[u8], from: usize, balance: i64) -> Vec<(usize, Journal)> {
        let found = Self::find_by_balance_bytes(data, from, balance);
        if found.is_empty() { Self::find_by_linear_scan(data, from, balance) } else { found }
    }

    /// Finds the player's journal by its place in the entity data. Returns
    /// the offset and the journal.
    ///
    /// The journal follows the edge-addon name table: entries of
    /// (`str` owning mod, `str` resource path, `u32` index), the base game's
    /// last one being "barrier_track_b.edge" with index 5. After the table
    /// come a `u32`, a `vec<u32>`, a `u32` and the bytes `01 00 00 00 00`, and
    /// then the journal. Its stored balance equals the header money or the
    /// sum of its amounts. Up to 64 bytes further on, a non-empty journal is
    /// accepted if its stored balance equals `header_money`.
    pub(crate) fn find_by_position(data: &[u8], from: usize, header_money: i64) -> Option<(usize, Journal)> {
        for hit in memchr::memmem::find_iter(&data[from..], EDGE_TABLE_ANCHOR) {
            let mut at = from + hit + EDGE_TABLE_ANCHOR.len();
            if read_u32(data, at) != Some(5) {
                continue;
            }
            at += 4;
            // Entries added by mods follow the base game's.
            while let Some(path_at) = skip_text(data, at)
                && let Some(end) = skip_text(data, path_at).filter(|&e| e > path_at + 4)
                && read_u32(data, end).is_some()
            {
                at = end + 4;
            }
            let Some(ids) = read_u32(data, at + 4).filter(|&n| n <= MAX_IDS) else { continue };
            let usual = at + 8 + ids as usize * 4 + 4 + 5;
            if read_u32(data, usual - 5) == Some(1)
                && data.get(usual - 1) == Some(&0)
                && let Ok(j) = Journal::read(&mut Reader::at(data, usual))
                && (j.balance == header_money || j.balance as i128 == j.sum())
            {
                return Some((usual, j));
            }
            for start in usual + 1..usual + LATE_START_WINDOW {
                if let Ok(j) = Journal::read(&mut Reader::at(data, start))
                    && !j.entries.is_empty()
                    && j.balance == header_money
                {
                    return Some((start, j));
                }
            }
        }
        None
    }

    /// Fast path: SIMD-search the balance bytes, then walk back over
    /// 21-byte entries until the stored count matches the number walked.
    /// Requires the newest entry to have a non-zero amount, which keeps
    /// runs of zero bytes from turning into long walks.
    fn find_by_balance_bytes(data: &[u8], from: usize, balance: i64) -> Vec<(usize, Journal)> {
        let target = balance.to_le_bytes();
        let mut found = Vec::new();
        let mut search_from = from;
        for hit in memchr::memmem::find_iter(&data[from..], &target) {
            let bal_at = from + hit;
            if bal_at < search_from {
                continue;
            }
            let mut later_time = i64::MAX;
            for k in 1.. {
                let Some(entry_at) = bal_at.checked_sub(k * ENTRY_SIZE).filter(|&e| e >= from + 4) else { break };
                let time = i64::from_le_bytes(data[entry_at..entry_at + 8].try_into().unwrap());
                let amount = i64::from_le_bytes(data[entry_at + 8..entry_at + 16].try_into().unwrap());
                let valid = Category::from_bytes(&data[entry_at + 16..entry_at + 21]).is_valid()
                    && (0..=later_time).contains(&time)
                    && (k > 1 || amount != 0);
                if !valid {
                    break;
                }
                later_time = time;
                let start = entry_at - 4;
                if u32::from_le_bytes(data[start..entry_at].try_into().unwrap()) as usize == k
                    && let Ok(j) = Journal::read(&mut Reader::at(data, start))
                    && j.sum() == j.balance as i128
                {
                    found.push((start, j));
                    search_from = bal_at + 8;
                    break;
                }
            }
        }
        found
    }

    /// Slow fallback: try every offset as a journal start. For each offset
    /// the stored count tells where the balance must be, so most offsets are
    /// rejected with two loads.
    fn find_by_linear_scan(data: &[u8], from: usize, balance: i64) -> Vec<(usize, Journal)> {
        let target = balance.to_le_bytes();
        let mut found = Vec::new();
        let mut p = from;
        while p + 4 + ENTRY_SIZE + 8 <= data.len() {
            let n = u32::from_le_bytes(data[p..p + 4].try_into().unwrap()) as usize;
            let bal_at = p + 4 + n.saturating_mul(ENTRY_SIZE);
            if n > 0
                && bal_at + 8 <= data.len()
                && data[bal_at..bal_at + 8] == target
                && Category::from_bytes(&data[p + 20..p + 25]).is_valid()
                && let Ok(j) = Journal::read(&mut Reader::at(data, p))
                && j.sum() == j.balance as i128
            {
                found.push((p, j));
                p = bal_at + 8;
                continue;
            }
            p += 1;
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Journal {
        Journal {
            entries: vec![
                Entry { time: 10, amount: 1000, category: Category::OTHER },
                Entry {
                    time: 20,
                    amount: -300,
                    category: Category { kind: 4, construction: 6, maintenance: 0, other: 0, carrier: 0 },
                },
            ],
            balance: 700,
        }
    }

    #[test]
    fn roundtrip_and_locate() {
        let j = sample();
        let mut w = Writer::new();
        w.bytes(&[0xAA; 13]);
        j.write(&mut w);
        w.bytes(&[0; 7]);
        let found = Journal::find_with_balance(&w.buf, 0, 700);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 13);
        assert_eq!(found[0].1, j);
    }

    #[test]
    fn locate_falls_back_when_newest_amount_is_zero() {
        let mut j = sample();
        j.entries.push(Entry { time: 30, amount: 0, category: Category::OTHER });
        let mut w = Writer::new();
        w.bytes(&[0; 40]);
        j.write(&mut w);
        let found = Journal::find_with_balance(&w.buf, 0, 700);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 40);
    }

    #[test]
    fn set_balance_books_other_entry() {
        let mut j = sample();
        j.set_balance(5_000_000).unwrap();
        assert_eq!(j.balance, 5_000_000);
        assert_eq!(j.sum(), 5_000_000);
        let last = j.entries.last().unwrap();
        assert_eq!(last.time, 20);
        assert_eq!(last.category, Category::OTHER);
    }

    /// Name table with a mod's entry after the base game's last one, then
    /// the bytes that precede the journal.
    fn entity_data(journal: &Journal, extra_gap: usize) -> (Vec<u8>, usize) {
        let mut w = Writer::new();
        w.bytes(&[0xAA; 9]);
        w.string(b"");
        w.string(b"infrastructure/edge_addons/barrier_track_b.edge");
        w.u32(5);
        w.string(b"some_mod");
        w.string(b"edge_addons/some_mod_post.edge");
        w.u32(6);
        w.u32(24);
        w.u32(3);
        (10..13).for_each(|id| w.u32(id));
        w.u32(13);
        if extra_gap == 0 {
            w.u32(1);
            w.u8(0);
        } else {
            w.bytes(&[1, 1]);
            w.bytes(&vec![0; extra_gap + 3]);
        }
        let at = w.buf.len();
        journal.write(&mut w);
        w.bytes(&[0; 7]);
        (w.buf, at)
    }

    #[test]
    fn locate_by_position_ignores_the_balance() {
        let mut j = sample();
        j.balance = 0;
        let (data, at) = entity_data(&j, 0);
        let (found_at, found) = Journal::find_by_position(&data, 0, 0).unwrap();
        assert_eq!(found_at, at);
        assert_eq!(found, j);
        assert_eq!(found.money(), 700);
    }

    #[test]
    fn locate_by_position_finds_an_empty_journal() {
        let j = Journal { entries: vec![], balance: 0 };
        let (data, at) = entity_data(&j, 0);
        assert_eq!(Journal::find_by_position(&data, 0, 0).unwrap().0, at);
    }

    #[test]
    fn locate_by_position_accepts_a_later_start_only_with_the_header_money() {
        let mut j = sample();
        j.balance = -5;
        let (data, at) = entity_data(&j, 12);
        assert_eq!(Journal::find_by_position(&data, 0, -5).unwrap().0, at);
        assert!(Journal::find_by_position(&data, 0, 700).is_none());
    }

    #[test]
    fn locate_by_position_rejects_an_empty_journal_with_a_foreign_balance() {
        let j = Journal { entries: vec![], balance: 99 };
        let (data, _) = entity_data(&j, 0);
        assert!(Journal::find_by_position(&data, 0, 0).is_none());
        assert!(Journal::find_by_position(&data, 0, 99).is_some());
    }

    #[test]
    fn locate_by_position_needs_the_name_table() {
        let mut w = Writer::new();
        sample().write(&mut w);
        assert!(Journal::find_by_position(&w.buf, 0, 700).is_none());
    }

    #[test]
    fn money_is_the_sum_when_the_stored_balance_differs() {
        let mut j = sample();
        j.balance = 0;
        assert_eq!(j.money(), 700);
        j.set_balance(5_000_000).unwrap();
        assert_eq!((j.money(), j.balance), (5_000_000, 5_000_000));
        assert_eq!(j.entries.last().unwrap().amount, 4_999_300);
    }

    #[test]
    fn set_balance_to_the_sum_only_stores_it() {
        let mut j = sample();
        j.balance = 0;
        j.set_balance(700).unwrap();
        assert_eq!((j.entries.len(), j.balance), (2, 700));
    }
}
