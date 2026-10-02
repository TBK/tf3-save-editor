//! Serialised `lua::Value` / `lua::Table`, used by game script states and the
//! game configuration.
//!
//! A value is a `u32` variant index followed by its payload:
//!
//! | index | type   | payload                                   |
//! |-------|--------|-------------------------------------------|
//! | 0     | nil    | –                                         |
//! | 1     | bool   | `u8`                                      |
//! | 2     | number | `f64`                                     |
//! | 3     | string | `u32` length + bytes                      |
//! | 4     | table  | `u8` non-null flag, then a table if set   |
//!
//! A table is a `u32` pair count followed by `key, value` pairs (the engine
//! stores tables in a sorted btree, so order is preserved as read).

use std::fmt;

use crate::io::{Reader, Writer};
use crate::{Error, Result};

/// Recursion guard for hostile or corrupt input.
const MAX_DEPTH: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Number(f64),
    String(LuaString),
    /// `None` is a null table pointer (flag byte 0).
    Table(Option<Table>),
}

/// Lua strings are byte strings; they are almost always UTF-8 in practice.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct LuaString(pub Vec<u8>);

impl LuaString {
    pub fn as_str_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }
}

impl From<&str> for LuaString {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }
}

impl fmt::Debug for LuaString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str_lossy())
    }
}

impl fmt::Display for LuaString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str_lossy())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub entries: Vec<(Value, Value)>,
}

impl Table {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries.iter_mut().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v)
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self> {
        Self::read_depth(r, 0)
    }

    fn read_depth(r: &mut Reader<'_>, depth: usize) -> Result<Self> {
        if depth > MAX_DEPTH {
            return Err(Error::Malformed { offset: r.pos(), what: "lua table nesting too deep".into() });
        }
        // Smallest pair: two nil values (4 + 4 bytes).
        let n = r.count(8)?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let k = Value::read_depth(r, depth)?;
            let v = Value::read_depth(r, depth)?;
            entries.push((k, v));
        }
        Ok(Self { entries })
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.count(self.entries.len());
        for (k, v) in &self.entries {
            k.write(w);
            v.write(w);
        }
    }
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => std::str::from_utf8(&s.0).ok(),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&Table> {
        match self {
            Value::Table(Some(t)) => Some(t),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Table(_) => "table",
        }
    }

    fn read_depth(r: &mut Reader<'_>, depth: usize) -> Result<Self> {
        let offset = r.pos();
        Ok(match r.u32()? {
            0 => Value::Nil,
            1 => Value::Bool(match r.u8()? {
                0 => false,
                1 => true,
                b => return Err(Error::Malformed { offset, what: format!("bool byte {b}") }),
            }),
            2 => Value::Number(r.f64()?),
            3 => Value::String(LuaString(r.string()?)),
            4 => match r.u8()? {
                0 => Value::Table(None),
                1 => Value::Table(Some(Table::read_depth(r, depth + 1)?)),
                b => return Err(Error::Malformed { offset, what: format!("table flag {b}") }),
            },
            tag => return Err(Error::Malformed { offset, what: format!("unknown lua value tag {tag}") }),
        })
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        match self {
            Value::Nil => w.u32(0),
            Value::Bool(b) => {
                w.u32(1);
                w.u8(*b as u8);
            }
            Value::Number(n) => {
                w.u32(2);
                w.f64(*n);
            }
            Value::String(s) => {
                w.u32(3);
                w.string(&s.0);
            }
            Value::Table(None) => {
                w.u32(4);
                w.u8(0);
            }
            Value::Table(Some(t)) => {
                w.u32(4);
                w.u8(1);
                t.write(w);
            }
        }
    }
}

/// One row of a flattened table view.
#[derive(Clone, Debug)]
pub struct Leaf {
    /// Keys from the root table to this entry.
    pub path: Vec<Value>,
    /// Nesting depth (0 for direct children of the root).
    pub depth: usize,
    /// Display form of the last key.
    pub key: String,
    /// The value itself for scalars; a summary placeholder for tables.
    pub value: Value,
}

impl Leaf {
    pub fn is_editable(&self) -> bool {
        matches!(self.value, Value::Bool(_) | Value::Number(_) | Value::String(_))
    }

    /// Dotted path, e.g. `townStates.1.authorityScore`.
    pub fn path_string(&self) -> String {
        path_string(&self.path)
    }
}

pub fn path_string(path: &[Value]) -> String {
    path.iter()
        .map(|k| match k {
            Value::String(s) => s.as_str_lossy().into_owned(),
            other => format_scalar(other),
        })
        .collect::<Vec<_>>()
        .join(".")
}

impl Table {
    /// Depth-first list of every entry; nested tables appear as a row
    /// followed by their children.
    pub fn flatten(&self) -> Vec<Leaf> {
        let mut out = Vec::new();
        self.flatten_into(&mut Vec::new(), &mut out);
        out
    }

    fn flatten_into(&self, prefix: &mut Vec<Value>, out: &mut Vec<Leaf>) {
        for (k, v) in &self.entries {
            prefix.push(k.clone());
            let value = match v {
                Value::Table(Some(t)) => Value::Table(Some(Table { entries: Vec::with_capacity(t.entries.len()) })),
                other => other.clone(),
            };
            out.push(Leaf { path: prefix.clone(), depth: prefix.len() - 1, key: format_key(k), value });
            if let Value::Table(Some(t)) = v {
                t.flatten_into(prefix, out);
            }
            prefix.pop();
        }
    }

    /// Follows `path` (exact keys) to a value.
    pub fn lookup_mut(&mut self, path: &[Value]) -> Option<&mut Value> {
        let (first, rest) = path.split_first()?;
        let v = self.entries.iter_mut().find(|(k, _)| k == first).map(|(_, v)| v)?;
        if rest.is_empty() {
            return Some(v);
        }
        match v {
            Value::Table(Some(t)) => t.lookup_mut(rest),
            _ => None,
        }
    }

    pub fn lookup(&self, path: &[Value]) -> Option<&Value> {
        let (first, rest) = path.split_first()?;
        let v = self.entries.iter().find(|(k, _)| k == first).map(|(_, v)| v)?;
        if rest.is_empty() {
            return Some(v);
        }
        v.as_table()?.lookup(rest)
    }

    /// Resolves a dotted path (`townStates.1.authorityScore`) to exact keys.
    /// Each segment matches a string key, or a number key with the same
    /// printed form.
    pub fn resolve_path(&self, dotted: &str) -> Option<Vec<Value>> {
        let mut table = self;
        let mut path = Vec::new();
        let segments: Vec<&str> = dotted.split('.').collect();
        for (i, seg) in segments.iter().enumerate() {
            let (k, v) = table.entries.iter().find(|(k, _)| match k {
                Value::String(s) => s.0 == seg.as_bytes(),
                Value::Number(n) => format_number(*n) == *seg,
                Value::Bool(b) => b.to_string() == *seg,
                _ => false,
            })?;
            path.push(k.clone());
            if i + 1 < segments.len() {
                table = v.as_table()?;
            }
        }
        Some(path)
    }
}

impl Value {
    /// Parses user input as a new value of the same type as `self`.
    pub fn parse_like(&self, input: &str) -> Result<Value> {
        let input = input.trim();
        match self {
            Value::Number(_) => input
                .replace(['_', ','], "")
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .map(Value::Number)
                .ok_or_else(|| Error::Invalid(format!("{input:?} is not a number"))),
            Value::Bool(_) => match input.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Ok(Value::Bool(true)),
                "false" | "0" | "no" | "off" => Ok(Value::Bool(false)),
                _ => Err(Error::Invalid(format!("{input:?} is not true/false"))),
            },
            Value::String(_) => Ok(Value::String(LuaString(input.as_bytes().to_vec()))),
            other => Err(Error::Invalid(format!("{} values cannot be edited", other.type_name()))),
        }
    }
}

/// Formats a key the way Lua source would (`name`, `[1]`, `["a b"]`).
pub fn format_key(k: &Value) -> String {
    match k {
        Value::String(s) => {
            let s = s.as_str_lossy();
            let ident = s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if ident { s.into_owned() } else { format!("[{s:?}]") }
        }
        other => format!("[{}]", format_scalar(other)),
    }
}

/// Formats a non-table value for display.
pub fn format_scalar(v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => format_number(*n),
        Value::String(s) => format!("{:?}", s.as_str_lossy()),
        Value::Table(None) => "nil table".into(),
        Value::Table(Some(t)) => format!("{{…{} entries}}", t.entries.len()),
    }
}

pub fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_nested() {
        let mut inner = Table::default();
        inner.entries.push((Value::Number(1.0), Value::String("x".into())));
        let t = Table {
            entries: vec![
                (Value::String("a".into()), Value::Bool(true)),
                (Value::String("b".into()), Value::Table(Some(inner))),
                (Value::String("c".into()), Value::Table(None)),
                (Value::String("d".into()), Value::Nil),
            ],
        };
        let mut w = Writer::new();
        t.write(&mut w);
        let back = Table::read(&mut Reader::new(&w.buf)).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn flatten_and_resolve() {
        let mut inner = Table::default();
        inner.entries.push((Value::Number(1.0), Value::Number(5.0)));
        let mut t = Table::default();
        t.entries.push((Value::String("towns".into()), Value::Table(Some(inner))));
        let rows = t.flatten();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].path_string(), "towns.1");
        let path = t.resolve_path("towns.1").unwrap();
        *t.lookup_mut(&path).unwrap() = Value::Number(7.0);
        assert_eq!(t.lookup(&path), Some(&Value::Number(7.0)));
        assert!(t.resolve_path("towns.2").is_none());
        assert_eq!(Value::Number(0.0).parse_like("1_000").unwrap(), Value::Number(1000.0));
        assert!(Value::Number(0.0).parse_like("abc").is_err());
    }

    #[test]
    fn rejects_bad_tag() {
        let mut w = Writer::new();
        w.u32(1);
        w.u32(9);
        assert!(Table::read(&mut Reader::new(&w.buf)).is_err());
    }
}
