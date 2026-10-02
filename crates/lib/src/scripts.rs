//! Persistent state of the game's Lua "game scripts" (`*.gs`): company,
//! loans, towns, mission progress, achievements, …
//!
//! Layout: `u32 unknown`, `u32 count`, then per script:
//! `string mod`, `string name`, `u8 flag`, table `state`,
//! `u8 has_subscriptions`, (`u32 n`, `n` × `string event`).

use crate::io::{Reader, Writer};
use crate::lua::{LuaString, Table};
use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptState {
    /// Owning mod; empty for the base game.
    pub module: LuaString,
    /// Script resource name, e.g. `game_mechanics/finance/loan.gs`.
    pub name: String,
    pub flag: u8,
    pub state: Table,
    /// Events the script subscribed to.
    pub subscriptions: Option<Vec<LuaString>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptStates {
    pub unknown: u32,
    pub scripts: Vec<ScriptState>,
}

impl ScriptStates {
    pub fn get(&self, name: &str) -> Option<&ScriptState> {
        self.scripts.iter().find(|s| s.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut ScriptState> {
        self.scripts.iter_mut().find(|s| s.name == name)
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self> {
        let unknown = r.u32()?;
        let n = r.count(14)?;
        let mut scripts = Vec::with_capacity(n);
        for _ in 0..n {
            let module = LuaString(r.string()?);
            let offset = r.pos();
            let name = String::from_utf8(r.string()?)
                .ok()
                .filter(|n| n.ends_with(".gs"))
                .ok_or_else(|| Error::Malformed { offset, what: "script name is not a .gs resource".into() })?;
            let flag = r.u8()?;
            let state = Table::read(r)?;
            let subscriptions = match r.u8()? {
                0 => None,
                1 => {
                    let k = r.count(4)?;
                    Some((0..k).map(|_| r.string().map(LuaString)).collect::<Result<_>>()?)
                }
                b => return Err(Error::Malformed { offset: r.pos() - 1, what: format!("subscription flag {b}") }),
            };
            scripts.push(ScriptState { module, name, flag, state, subscriptions });
        }
        Ok(Self { unknown, scripts })
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.u32(self.unknown);
        w.count(self.scripts.len());
        for s in &self.scripts {
            w.string(&s.module.0);
            w.string(s.name.as_bytes());
            w.u8(s.flag);
            s.state.write(w);
            match &s.subscriptions {
                None => w.u8(0),
                Some(subs) => {
                    w.u8(1);
                    w.count(subs.len());
                    subs.iter().for_each(|e| w.string(&e.0));
                }
            }
        }
    }

    /// Locates the block by its first `*.gs` name and validates it with a
    /// full parse. Returns the block offset, the parsed block and its length.
    pub(crate) fn locate(data: &[u8], from: usize) -> Option<(usize, Self, usize)> {
        for hit in memchr::memmem::find_iter(&data[from..], b".gs") {
            let end = from + hit + 3;
            for name_len in 4..=512.min(end.saturating_sub(from + 4)) {
                let name_at = end - name_len;
                if read_u32(data, name_at - 4) != Some(name_len as u32) {
                    continue;
                }
                // The owning-mod string sits right before the name.
                for mod_len in 0..=256usize {
                    let Some(mod_at) = (name_at - 4).checked_sub(mod_len + 4) else { break };
                    if read_u32(data, mod_at) != Some(mod_len as u32) || mod_at < from + 8 {
                        continue;
                    }
                    let block_at = mod_at - 8;
                    let mut r = Reader::at(data, block_at);
                    if let Ok(block) = Self::read(&mut r) {
                        return Some((block_at, block, r.pos() - block_at));
                    }
                }
            }
        }
        None
    }
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::Value;

    #[test]
    fn roundtrip_and_locate() {
        let mut state = Table::default();
        state.entries.push((Value::String("version".into()), Value::Number(2.0)));
        let block = ScriptStates {
            unknown: 0,
            scripts: vec![
                ScriptState {
                    module: LuaString::default(),
                    name: "game_mechanics/a.gs".into(),
                    flag: 1,
                    state: state.clone(),
                    subscriptions: Some(vec!["Ev".into()]),
                },
                ScriptState { module: "some_mod".into(), name: "b.gs".into(), flag: 0, state, subscriptions: None },
            ],
        };
        let mut w = Writer::new();
        w.bytes(b"junk.gs junk");
        let at = w.buf.len();
        block.write(&mut w);
        let len = w.buf.len() - at;
        w.bytes(&[1, 2, 3]);
        let (found_at, found, found_len) = ScriptStates::locate(&w.buf, 0).unwrap();
        assert_eq!((found_at, found_len), (at, len));
        assert_eq!(found, block);
    }
}
