//! `tf3se-cli`: inspect and edit Transport Fever 3 saves from the command line.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use tf3save::SaveFile;
use tf3save::lua::{Value, format_number, format_scalar};

#[derive(Parser)]
#[command(
    name = "tf3se-cli",
    version,
    about = "Transport Fever 3 save editor",
    after_help = "The save file can be given positionally or with --save, before or after the command:\n  \
                  tf3se-cli money game.sav\n  tf3se-cli --save game.sav money\n  tf3se-cli money --save game.sav"
)]
struct Cli {
    /// Save file to work on, instead of the SAVE argument.
    // Never set after parsing: `hoist_save_option` moves it into the SAVE
    // position first. Declared so it shows up in --help.
    #[arg(short, long, global = true, value_name = "FILE")]
    save: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct Output {
    /// Write the result here instead of overwriting the input.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// When overwriting the input, skip the `.sav.bak` backup.
    #[arg(long)]
    no_backup: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Summary: title, year, money, mods, sections found.
    Info { save: PathBuf },
    /// Show or set the player's money.
    Money {
        save: PathBuf,
        /// New balance.
        #[arg(long, allow_hyphen_values = true)]
        set: Option<i64>,
        #[command(flatten)]
        out: Output,
    },
    /// List the journal (money bookings), newest last.
    Journal {
        save: PathBuf,
        /// Only show the last N entries.
        #[arg(long, default_value_t = 30)]
        last: usize,
    },
    /// Show or change game settings (advanced options, town sensitivity…).
    Settings {
        save: PathBuf,
        /// `key=value` pairs to change.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        #[command(flatten)]
        out: Output,
    },
    /// List game scripts, or dump one script's state.
    Scripts {
        save: PathBuf,
        /// Script name (or a unique substring such as `loan`).
        script: Option<String>,
    },
    /// Change a value inside a script state.
    Set {
        save: PathBuf,
        /// Script name (or unique substring).
        script: String,
        /// Dotted path, e.g. `availableLoans.1.amount`.
        path: String,
        /// New value (same type as the current one).
        #[arg(allow_hyphen_values = true)]
        value: String,
        #[command(flatten)]
        out: Output,
    },
    /// Export the preview image as PNG.
    Thumbnail { save: PathBuf, png: PathBuf },
}

/// Moves `--save FILE` / `-s FILE` / `--save=FILE` from anywhere on the
/// command line to the SAVE position right after the subcommand, so each
/// subcommand keeps one positional layout.
fn hoist_save_option(args: Vec<OsString>, subcommands: &[String]) -> Result<Vec<OsString>> {
    let mut rest = Vec::with_capacity(args.len());
    let mut save = None;
    let mut iter = args.into_iter();
    rest.extend(iter.next()); // program name
    while let Some(arg) = iter.next() {
        let text = arg.to_string_lossy();
        if text == "--" {
            rest.push(arg);
            rest.extend(iter.by_ref());
            break;
        }
        let value = if text == "--save" || text == "-s" {
            Some(iter.next().with_context(|| format!("{text} needs a file name"))?)
        } else {
            text.strip_prefix("--save=").map(OsString::from)
        };
        match value {
            Some(v) if save.is_some() => bail!("--save given more than once ({})", v.to_string_lossy()),
            Some(v) => save = Some(v),
            None => rest.push(arg),
        }
    }
    let Some(save) = save else { return Ok(rest) };
    // Top-level flags take no values, so the first non-flag is the subcommand.
    match rest.iter().skip(1).position(|a| !a.to_string_lossy().starts_with('-')) {
        Some(i) if subcommands.iter().any(|c| *c == rest[i + 1].to_string_lossy()) => {
            rest.insert(i + 2, save);
            Ok(rest)
        }
        _ => bail!("--save needs a command, e.g. `tf3se-cli --save game.sav info`"),
    }
}

fn main() -> Result<()> {
    let subcommands: Vec<String> = Cli::command().get_subcommands().map(|c| c.get_name().to_string()).collect();
    let args = hoist_save_option(std::env::args_os().collect(), &subcommands)?;
    match Cli::parse_from(args).command {
        Command::Info { save } => info(&open(&save)?),
        Command::Money { save, set, out } => {
            let mut s = open(&save)?;
            match set {
                None => println!("{}", s.money()),
                Some(v) => {
                    let old = s.money();
                    s.set_money(v)?;
                    write(&s, &save, &out)?;
                    println!("money: {old} -> {v}");
                }
            }
            Ok(())
        }
        Command::Journal { save, last } => {
            let s = open(&save)?;
            let j = s.journal.as_ref().context("player journal not found")?;
            let skip = j.entries.len().saturating_sub(last);
            for e in &j.entries[skip..] {
                println!("{:>12}  {:>14}  {}", e.time, e.amount, e.category.label());
            }
            println!("{} entries, balance {}", j.entries.len(), j.balance);
            Ok(())
        }
        Command::Settings { save, set, out } => {
            let mut s = open(&save)?;
            if set.is_empty() {
                let t = s.header.config.settings().context("settings table not found")?;
                for (k, v) in &t.entries {
                    println!("{} = {}", k.as_str().unwrap_or("?"), format_scalar(v));
                }
                return Ok(());
            }
            let t = s.header.config.settings_mut().context("settings table not found")?;
            for kv in &set {
                let (k, v) = kv.split_once('=').context("expected KEY=VALUE")?;
                let slot = t.get_mut(k.trim()).with_context(|| format!("unknown setting {k:?}"))?;
                *slot = slot.parse_like(v)?;
            }
            write(&s, &save, &out)
        }
        Command::Scripts { save, script } => {
            let s = open(&save)?;
            let states = s.scripts.as_ref().context("script states not found")?;
            match script {
                None => {
                    for sc in &states.scripts {
                        let module = if sc.module.0.is_empty() { String::new() } else { format!("  [{}]", sc.module) };
                        println!("{}{module}", sc.name);
                    }
                }
                Some(name) => {
                    let sc = &states.scripts[find_script(&s, &name)?];
                    for leaf in sc.state.flatten() {
                        let indent = "  ".repeat(leaf.depth);
                        let v = match &leaf.value {
                            Value::Table(Some(_)) => String::from("{"),
                            other => format_scalar(other),
                        };
                        println!("{indent}{} = {v}", leaf.key);
                    }
                }
            }
            Ok(())
        }
        Command::Set { save, script, path, value, out } => {
            let mut s = open(&save)?;
            let idx = find_script(&s, &script)?;
            let sc = &mut s.scripts.as_mut().expect("checked").scripts[idx];
            let keys = sc.state.resolve_path(&path).with_context(|| format!("no value at {path:?} in {}", sc.name))?;
            let slot = sc.state.lookup_mut(&keys).expect("resolved");
            let old = format_scalar(slot);
            *slot = slot.parse_like(&value)?;
            println!("{}: {path}: {old} -> {}", sc.name, format_scalar(slot));
            write(&s, &save, &out)
        }
        Command::Thumbnail { save, png } => {
            let s = open(&save)?;
            let t = s.header.thumbnail.as_ref().context("save has no preview image")?;
            std::fs::write(&png, t.to_png()?)?;
            Ok(())
        }
    }
}

fn open(path: &Path) -> Result<SaveFile> {
    let s = SaveFile::open(path).with_context(|| format!("reading {}", path.display()))?;
    if s.header.version != tf3save::KNOWN_VERSION {
        eprintln!(
            "warning: save format version {} (this tool was built for {}); verify results in game",
            s.header.version,
            tf3save::KNOWN_VERSION
        );
    }
    Ok(s)
}

fn write(s: &SaveFile, input: &Path, out: &Output) -> Result<()> {
    let target = out.output.as_deref().unwrap_or(input);
    if target == input && !out.no_backup {
        let b = tf3save::backup(input)?;
        eprintln!("backup: {}", b.display());
    }
    s.save(target).with_context(|| format!("writing {}", target.display()))?;
    eprintln!("saved: {}", target.display());
    Ok(())
}

fn find_script(s: &SaveFile, name: &str) -> Result<usize> {
    let states = s.scripts.as_ref().context("script states not found")?;
    if let Some(i) = states.scripts.iter().position(|sc| sc.name == name) {
        return Ok(i);
    }
    let hits: Vec<usize> = (0..states.scripts.len()).filter(|&i| states.scripts[i].name.contains(name)).collect();
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => bail!("no script matches {name:?}"),
        _ => bail!(
            "{name:?} is ambiguous: {}",
            hits.iter().map(|&i| states.scripts[i].name.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn info(s: &SaveFile) -> Result<()> {
    let h = &s.header;
    println!("title:     {}", h.title());
    println!("version:   {}", h.version);
    println!("year:      {}", h.year);
    println!("money:     {}", s.money());
    for m in &h.mods {
        println!("mod:       {} ({})", m.name, m.id);
    }
    for (k, v) in &h.config.resources {
        println!("{:<10} {}", format!("{k}:"), v);
    }
    if let Some(t) = &h.thumbnail {
        println!("preview:   {}x{}", t.width, t.height);
    }
    println!("size:      {} bytes uncompressed", s.raw_len());
    match &s.journal {
        Some(j) => println!("journal:   {} entries, balance {}", j.entries.len(), format_number(j.balance as f64)),
        None => println!("journal:   not found ({} candidates) — money is read-only", s.journal_matches),
    }
    match &s.scripts {
        Some(sc) => println!("scripts:   {} game script states", sc.scripts.len()),
        None => println!("scripts:   not found"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Vec<String>> {
        let subcommands: Vec<String> = Cli::command().get_subcommands().map(|c| c.get_name().to_string()).collect();
        let out = hoist_save_option(args.iter().map(OsString::from).collect(), &subcommands)?;
        Ok(out.into_iter().map(|a| a.to_string_lossy().into_owned()).collect())
    }

    #[test]
    fn save_option_moves_after_subcommand() {
        assert_eq!(run(&["tf3se-cli", "--save", "a.sav", "money"]).unwrap(), ["tf3se-cli", "money", "a.sav"]);
        assert_eq!(
            run(&["tf3se-cli", "-s", "a.sav", "money", "--set", "5"]).unwrap(),
            ["tf3se-cli", "money", "a.sav", "--set", "5"]
        );
        assert_eq!(
            run(&["tf3se-cli", "set", "loan", "x.1", "--save=a.sav", "-3"]).unwrap(),
            ["tf3se-cli", "set", "a.sav", "loan", "x.1", "-3"]
        );
        assert_eq!(run(&["tf3se-cli", "info", "a.sav"]).unwrap(), ["tf3se-cli", "info", "a.sav"]);
    }

    #[test]
    fn save_option_errors() {
        assert!(run(&["tf3se-cli", "--save", "a.sav"]).is_err());
        assert!(run(&["tf3se-cli", "--save"]).is_err());
        assert!(run(&["tf3se-cli", "-s", "a", "--save", "b", "info"]).is_err());
    }

    #[test]
    fn parses_after_hoisting() {
        let args = run(&["tf3se-cli", "--save", "a.sav", "set", "loan", "availableLoans.1.amount", "-5"]).unwrap();
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(
            matches!(cli.command, Command::Set { ref save, ref value, .. } if save == Path::new("a.sav") && value == "-5")
        );
    }

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
