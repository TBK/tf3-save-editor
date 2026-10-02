//! Prints how long each stage of loading a save takes:
//! `cargo run --release --example timing -p tf3save -- game.sav`

use std::time::Instant;
fn main() {
    let path = std::env::args().nth(1).expect("path");
    let t = Instant::now();
    let bytes = std::fs::read(&path).unwrap();
    println!("read       {:?}", t.elapsed());
    let t = Instant::now();
    let raw = tf3save::decompress(&bytes).unwrap();
    println!("decompress {:?}", t.elapsed());
    let t = Instant::now();
    let s = tf3save::SaveFile::from_raw(raw).unwrap();
    println!("parse      {:?}", t.elapsed());
    let t = Instant::now();
    let _ = s.header.thumbnail.as_ref().unwrap().to_png().unwrap();
    println!("png        {:?}", t.elapsed());
}
