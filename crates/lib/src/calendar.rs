//! The game's calendar: the speed and the table that maps game-clock ticks to
//! dates.
//!
//! Both sit in an engine component of the entity data that nothing names, so
//! they are found by the bytes around them, as the journal is. See
//! `docs/FORMAT.md` ("Calendar").

use memchr::memmem;

use crate::header::civil_from_julian_day;

/// `u64 9`, then `u32 1`: how the component starts.
const PREFIX: [u8; 12] = [9, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];

/// Bytes from the start of [`PREFIX`] to the day table's count.
const TABLE_OFFSET: usize = 61;

/// The day a game-clock value falls in, from the day table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DayStart {
    /// Game-clock value (milliseconds) at which the day started.
    pub tick: u64,
    /// Julian day number (2415021 is 1 January 1900).
    pub day: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calendar {
    /// Length of a calendar day in clock milliseconds: 4000 at speed 1.00x,
    /// 0 when the calendar is stopped.
    pub millis_per_day: u32,
    /// 0 in a save written with the game paused.
    pub play_speed: u32,
    /// The game clock when the save was written.
    pub clock: u64,
    /// One entry per day while the game runs, so the gap between two entries
    /// is that day's length in ticks at the speed then. Not every save has
    /// one entry per day (dates can be set), and the date is the last entry.
    pub days: Vec<DayStart>,
}

impl Calendar {
    /// Finds the calendar after `from`. Returns `None` unless the pattern
    /// matches exactly once and its table fits in the data.
    pub(crate) fn locate(data: &[u8], from: usize) -> Option<Self> {
        let mut found = None;
        for hit in memmem::find_iter(&data[from..], &PREFIX) {
            if let Some(c) = Self::parse(data, from + hit) {
                if found.is_some() {
                    return None;
                }
                found = Some(c);
            }
        }
        found
    }

    fn parse(data: &[u8], at: usize) -> Option<Self> {
        let u32_at = |o: usize| Some(u32::from_le_bytes(data.get(at + o..at + o + 4)?.try_into().unwrap()));
        let u64_at = |o: usize| Some(u64::from_le_bytes(data.get(at + o..at + o + 8)?.try_into().unwrap()));
        // After the prefix: millisPerDay, playSpeed, 8 zero bytes, u32 1,
        // u64 8, u32 1, t1, u8 1, t2.
        if u64_at(20)? != 0 || u32_at(28)? != 1 || u64_at(32)? != 8 || u32_at(40)? != 1 || *data.get(at + 52)? != 1 {
            return None;
        }
        let n = u32_at(TABLE_OFFSET)? as usize;
        let table = data.get(at + TABLE_OFFSET + 4..)?.get(..n.checked_mul(12)?)?;
        let days = table
            .as_chunks::<12>()
            .0
            .iter()
            .map(|e| DayStart {
                tick: u64::from_le_bytes(e[..8].try_into().unwrap()),
                day: u32::from_le_bytes(e[8..].try_into().unwrap()),
            })
            .collect();
        Some(Self { millis_per_day: u32_at(12)?, play_speed: u32_at(16)?, clock: u64_at(44)?, days })
    }

    /// The day a clock value falls in: the last table entry that started at
    /// or before it. `None` for a negative value or an empty table.
    pub fn day_at(&self, tick: i64) -> Option<u32> {
        let tick = u64::try_from(tick).ok()?;
        let after = self.days.partition_point(|d| d.tick <= tick);
        self.days.get(after.checked_sub(1)?).map(|d| d.day)
    }

    /// [`Self::day_at`] as a Gregorian `(year, month, day)`.
    pub fn date_at(&self, tick: i64) -> Option<(i64, u32, u32)> {
        self.day_at(tick).map(civil_from_julian_day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Writer;

    fn component(millis_per_day: u32, days: &[(u64, u32)]) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(&PREFIX);
        w.u32(millis_per_day);
        w.u32(4);
        w.bytes(&[0; 8]);
        w.u32(1);
        w.bytes(&8u64.to_le_bytes());
        w.u32(1);
        w.bytes(&9_000u64.to_le_bytes());
        w.u8(1);
        w.bytes(&8_200u64.to_le_bytes());
        w.u32(days.len() as u32);
        for &(tick, day) in days {
            w.bytes(&tick.to_le_bytes());
            w.u32(day);
        }
        w.buf
    }

    #[test]
    fn reads_the_component_and_the_table() {
        let mut data = vec![7; 33];
        data.extend(component(1000, &[(0, 2415021), (1000, 2415022)]));
        data.extend([5; 10]);
        let c = Calendar::locate(&data, 0).expect("found");
        assert_eq!((c.millis_per_day, c.play_speed, c.clock), (1000, 4, 9_000));
        assert_eq!(c.days, [DayStart { tick: 0, day: 2415021 }, DayStart { tick: 1000, day: 2415022 }]);
    }

    #[test]
    fn day_is_the_last_entry_at_or_before() {
        let c = Calendar::locate(&component(4000, &[(0, 2415021), (4000, 2415022), (12_000, 2415023)]), 0).unwrap();
        assert_eq!(c.day_at(-1), None);
        assert_eq!(c.day_at(0), Some(2415021));
        assert_eq!(c.day_at(3999), Some(2415021));
        assert_eq!(c.day_at(4000), Some(2415022));
        // A stopped or slow stretch makes one long day.
        assert_eq!(c.day_at(11_999), Some(2415022));
        assert_eq!(c.day_at(1_000_000), Some(2415023));
        assert_eq!(c.date_at(4000), Some((1900, 1, 2)));
    }

    #[test]
    fn missing_ambiguous_or_cut_off_is_none() {
        assert!(Calendar::locate(&[0; 100], 0).is_none());
        let one = component(4000, &[(0, 2415021)]);
        let two = [one.clone(), one.clone()].concat();
        assert!(Calendar::locate(&two, 0).is_none());
        assert!(Calendar::locate(&one[..one.len() - 1], 0).is_none());
        // The search starts at `from`.
        assert!(Calendar::locate(&one, 1).is_none());
    }

    #[test]
    fn empty_table_has_no_day() {
        let c = Calendar::locate(&component(4000, &[]), 0).unwrap();
        assert_eq!(c.day_at(0), None);
    }
}
