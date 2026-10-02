//! Tests against a real save, which you provide: set `TF3_TEST_SAVE` to the
//! path of a `.sav` file. Skipped when it is unset. Saves are never checked
//! in, as they may contain material that cannot be redistributed.

use std::path::PathBuf;

use tf3save::SaveFile;
use tf3save::lua::Value;

fn test_save() -> Option<PathBuf> {
    std::env::var_os("TF3_TEST_SAVE").map(PathBuf::from)
}

macro_rules! require_save {
    () => {
        match test_save() {
            Some(p) => p,
            None => {
                eprintln!("TF3_TEST_SAVE not set; skipping");
                return;
            }
        }
    };
}

#[test]
fn unmodified_roundtrip_is_byte_identical() {
    let path = require_save!();
    let compressed = std::fs::read(&path).unwrap();
    let raw = zstd::stream::decode_all(&compressed[..]).unwrap();
    let save = SaveFile::from_raw(raw.clone()).unwrap();
    assert!(save.journal.is_some(), "journal not found ({} matches)", save.journal_matches);
    assert!(save.scripts.is_some(), "script states not found");
    assert!(save.to_raw() == raw, "re-serialised save differs");
}

#[test]
fn money_edit_survives_reload() {
    let path = require_save!();
    let mut save = SaveFile::open(&path).unwrap();
    let before = save.money();
    let entries = save.journal.as_ref().unwrap().entries.len();
    save.set_money(123_456_789).unwrap();
    let bytes = save.to_bytes().unwrap();

    let again = SaveFile::from_compressed(&bytes).unwrap();
    assert_ne!(before, 123_456_789);
    assert_eq!(again.money(), 123_456_789);
    assert_eq!(again.header.money, 123_456_789);
    assert_eq!(again.journal.as_ref().unwrap().entries.len(), entries + 1);
    // Sections after the journal must still parse.
    assert_eq!(again.scripts, save.scripts);
}

#[test]
fn script_edit_survives_reload() {
    let path = require_save!();
    let mut save = SaveFile::open(&path).unwrap();
    let scripts = save.scripts.as_mut().unwrap();
    let first = &mut scripts.scripts[0];
    let name = first.name.clone();
    first.state.entries.push((Value::String("tf3se_marker".into()), Value::String("a longer string value".into())));
    let bytes = save.to_bytes().unwrap();

    let again = SaveFile::from_compressed(&bytes).unwrap();
    let state = &again.scripts.as_ref().unwrap().get(&name).unwrap().state;
    assert_eq!(state.get("tf3se_marker").and_then(Value::as_str), Some("a longer string value"));
    assert_eq!(again.money(), save.money());
}
