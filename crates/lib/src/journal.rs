//! The player's financial journal.
//!
//! The engine keeps one journal per entity: a time-ordered list of booked
//! amounts, followed by the cached balance (the sum of all amounts). The
//! player's money is that balance, so changing money means booking an entry.
//!
//! Layout: `u32 count`, then `count` × (`i64 time`, `i64 amount`,
//! 5 × `u8` category), then `i64 balance`.

use crate::io::{Reader, Writer};
use crate::{Error, Result};

pub const ENTRY_SIZE: usize = 21;

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
    pub balance: i64,
}

impl Journal {
    pub fn sum(&self) -> i128 {
        self.entries.iter().map(|e| e.amount as i128).sum()
    }

    /// Books an `Other` entry so the balance becomes `target`. The entry is
    /// stamped with the latest time already in the journal, which keeps the
    /// list ordered as the engine requires.
    pub fn set_balance(&mut self, target: i64) -> Result<()> {
        let delta = (target as i128) - (self.balance as i128);
        if delta == 0 {
            return Ok(());
        }
        let amount = i64::try_from(delta).map_err(|_| Error::Invalid("money change too large".into()))?;
        let time = self.entries.last().map_or(0, |e| e.time);
        self.entries.push(Entry { time, amount, category: Category::OTHER });
        self.balance = target;
        Ok(())
    }

    /// Validating parse. Used both for loading and for locating journals.
    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self> {
        let start = r.pos();
        let n = r.count(ENTRY_SIZE)?;
        let mut entries = Vec::with_capacity(n);
        let mut last_time = i64::MIN;
        let mut sum: i128 = 0;
        for _ in 0..n {
            let time = r.i64()?;
            let amount = r.i64()?;
            let category = Category::from_bytes(r.bytes(5)?);
            if !category.is_valid() || time < last_time || time < 0 {
                return Err(Error::Malformed { offset: start, what: "not a journal".into() });
            }
            last_time = time;
            sum += amount as i128;
            entries.push(Entry { time, amount, category });
        }
        let balance = r.i64()?;
        if balance as i128 != sum {
            return Err(Error::Malformed { offset: start, what: "journal balance mismatch".into() });
        }
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
    /// equals `balance`. Returns `(offset, journal)` pairs.
    pub(crate) fn find_with_balance(data: &[u8], from: usize, balance: i64) -> Vec<(usize, Journal)> {
        let found = Self::find_by_balance_bytes(data, from, balance);
        if found.is_empty() { Self::find_by_linear_scan(data, from, balance) } else { found }
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
}
