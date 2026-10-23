//! `tel-decode` command line.
//!
//! ```text
//! tel-decode info    <image-or-device> [--start-lba N] [--blocks N]
//! tel-decode samples <image-or-device> [--session S] [-o file.csv] [region options]
//! tel-decode events  ...      tel-decode can-raw ...      tel-decode signals ...
//! tel-decode prepare <image-or-device> --blocks N [--start-lba N] --yes-erase
//! ```
//!
//! `--blocks` defaults to everything from `--start-lba` to the end of the file.
//! `prepare` overwrites only the first three blocks of the region.

use std::io::{BufWriter, Write};
use std::process::ExitCode;
use tel_decode::export::{export, iso8601, session_info, What};
use tel_decode::scan::{scan, DEFAULT_STOP_AFTER_INVALID};
use tel_decode::source::{BlockSource, FileSource};
use tel_logfmt::recovery::Geometry;

struct Opts {
    path: String,
    start_lba: u32,
    blocks: Option<u32>,
    session: Option<u32>,
    out: Option<String>,
    yes_erase: bool,
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut it = args.iter();
    let path = it.next().ok_or("missing <image-or-device>")?.clone();
    let mut o = Opts { path, start_lba: 0, blocks: None, session: None, out: None, yes_erase: false };
    while let Some(a) = it.next() {
        let mut num = |name: &str| -> Result<u32, String> {
            it.next()
                .ok_or(format!("{name} needs a value"))?
                .parse::<u32>()
                .map_err(|_| format!("{name} needs a number"))
        };
        match a.as_str() {
            "--start-lba" => o.start_lba = num("--start-lba")?,
            "--blocks" => o.blocks = Some(num("--blocks")?),
            "--session" => o.session = Some(num("--session")?),
            "-o" | "--out" => o.out = Some(it.next().ok_or("-o needs a file name")?.clone()),
            "--yes-erase" => o.yes_erase = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(o)
}

fn geometry(o: &Opts, src: &mut FileSource) -> Result<Geometry, String> {
    let total = src.len_blocks().map_err(|e| e.to_string())?;
    let blocks = match o.blocks {
        Some(b) => b,
        None => u32::try_from(total.saturating_sub(u64::from(o.start_lba)))
            .map_err(|_| "device too large for --blocks default; pass --blocks".to_string())?,
    };
    Ok(Geometry { start_lba: o.start_lba, blocks })
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = args.split_first().ok_or(USAGE)?;
    let o = parse_opts(rest)?;
    match cmd.as_str() {
        "info" => {
            let mut src = FileSource::open(&o.path).map_err(|e| format!("{}: {e}", o.path))?;
            let geo = geometry(&o, &mut src)?;
            let report = scan(&mut src, geo, DEFAULT_STOP_AFTER_INVALID).map_err(|e| e.to_string())?;
            println!("card id        {:#010x}", report.card_id);
            println!("region         {} blocks from block {}", report.region_blocks, geo.start_lba);
            println!("valid blocks   {}", report.valid_blocks);
            println!(
                "damaged blocks {}   foreign {}   read errors {}",
                report.invalid_blocks, report.foreign_blocks, report.read_errors
            );
            println!("scan ended     {:?} at region block {}", report.ended, report.scanned_to_rel);
            println!("runs:");
            for r in &report.runs {
                let info =
                    session_info(&mut src, geo, report.card_id, r.session, DEFAULT_STOP_AFTER_INVALID).ok().flatten();
                let when = info
                    .filter(|i| i.utc_unix_ms != 0)
                    .map(|i| iso8601(i.utc_unix_ms))
                    .unwrap_or_else(|| "time unknown".into());
                let fw = info
                    .map(|i| String::from_utf8_lossy(&i.firmware).trim_end_matches('\0').to_string())
                    .unwrap_or_default();
                println!(
                    "  session {:>3}  blocks {:>8} (seq {}..{})  {}  {}{}{}",
                    r.session,
                    r.blocks,
                    r.first_seq,
                    r.last_seq,
                    if r.clean_close { "closed cleanly" } else { "NOT closed (power loss?)" },
                    when,
                    if r.gap_before > 0 {
                        format!("  [{} unusable blocks before]", r.gap_before)
                    } else {
                        String::new()
                    },
                    if fw.is_empty() { String::new() } else { format!("  fw {fw}") },
                );
            }
            Ok(())
        }
        "samples" | "events" | "can-raw" | "signals" => {
            let what = match cmd.as_str() {
                "samples" => What::Samples,
                "events" => What::Events,
                "can-raw" => What::CanRaw,
                _ => What::Signals,
            };
            let mut src = FileSource::open(&o.path).map_err(|e| format!("{}: {e}", o.path))?;
            let geo = geometry(&o, &mut src)?;
            let header = tel_decode::scan::read_header(&mut src, geo).map_err(|e| e.to_string())?;
            let mut out: Box<dyn Write> = match &o.out {
                Some(p) => Box::new(BufWriter::new(std::fs::File::create(p).map_err(|e| format!("{p}: {e}"))?)),
                None => Box::new(BufWriter::new(std::io::stdout().lock())),
            };
            let (stats, _) =
                export(&mut src, geo, header.card_id, o.session, what, DEFAULT_STOP_AFTER_INVALID, &mut out)
                    .map_err(|e| e.to_string())?;
            out.flush().map_err(|e| e.to_string())?;
            eprintln!("{} rows written, {} records could not be decoded", stats.rows, stats.undecodable);
            Ok(())
        }
        "prepare" => {
            if !o.yes_erase {
                return Err("prepare overwrites the first 3 blocks of the region; pass --yes-erase to confirm".into());
            }
            let blocks = o.blocks.ok_or("prepare needs --blocks N")?;
            let mut dev = FileSource::open_rw(&o.path).map_err(|e| format!("{}: {e}", o.path))?;
            let card_id = fresh_card_id();
            tel_decode::prepare(&mut dev, Geometry { start_lba: o.start_lba, blocks }, card_id)
                .map_err(|e| e.to_string())?;
            println!("prepared region of {blocks} blocks at block {} with card id {card_id:#010x}", o.start_lba);
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

const USAGE: &str = "usage: tel-decode <info|samples|events|can-raw|signals|prepare> <image-or-device> [options]\n\
options: --start-lba N  --blocks N  --session S  -o FILE  --yes-erase";

/// A card id that differs from run to run, from the clock and the process id.
fn fresh_card_id() -> u32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mixed = nanos ^ (u128::from(std::process::id()) << 64) ^ (nanos >> 32);
    let part = |shift: u32| u32::try_from((mixed >> shift) & 0xFFFF_FFFF).unwrap_or(0);
    let id = part(0) ^ part(32) ^ part(64);
    if id == 0 {
        0x5EED_5EED
    } else {
        id
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
