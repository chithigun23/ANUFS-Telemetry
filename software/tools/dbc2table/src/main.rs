//! `dbc2table <input.dbc|input.csv> <output.rs> [--skip-file <file>] [--check]`
//!
//! A `.csv` input is read as a hand-written signal list (see `dbc2table::CSV_HEADER`);
//! anything else is read as a DBC file.
//!
//! With `--check` the output file is not written; the program exits non-zero if
//! the file on disk differs from what would be generated (used to detect a
//! stale checked-in table).

use dbc2table::Options;
use std::process::ExitCode;

const USAGE: &str = "usage: dbc2table <input.dbc|input.csv> <output.rs> [--skip-file <file>] [--check]";

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or(USAGE)?;
    let output = args.next().ok_or(USAGE)?;
    let mut check = false;
    let mut opts = Options::default();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--check" => check = true,
            "--skip-file" => {
                let path = args.next().ok_or(USAGE)?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("reading {path}: {e}"))?;
                opts = Options::from_skip_file(&text);
            }
            _ => return Err(USAGE.to_string()),
        }
    }
    let text = std::fs::read_to_string(&input).map_err(|e| format!("reading {input}: {e}"))?;
    let name = std::path::Path::new(&input).file_name().and_then(|n| n.to_str()).unwrap_or("input.dbc");
    let is_csv = std::path::Path::new(&input)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    let generated = if is_csv {
        dbc2table::generate_from_csv(&text, name).map_err(|e| e.to_string())?
    } else {
        let (_, fixed) = dbc2table::sanitize(&text);
        if fixed > 0 {
            eprintln!(
                "warning: {fixed} signal range(s) without a max value were normalised (does not affect decoding)"
            );
        }
        if !opts.skip.is_empty() {
            eprintln!("note: skipping {} message(s): {}", opts.skip.len(), opts.skip.join(", "));
        }
        dbc2table::generate(&text, name, &opts).map_err(|e| e.to_string())?
    };
    if check {
        let existing = std::fs::read_to_string(&output).map_err(|e| format!("reading {output}: {e}"))?;
        if normalise(&existing) != normalise(&generated) {
            return Err(format!("{output} is out of date; rerun dbc2table"));
        }
        return Ok(());
    }
    std::fs::write(&output, generated).map_err(|e| format!("writing {output}: {e}"))
}

/// Ignore line-ending differences from git on Windows.
fn normalise(s: &str) -> String {
    s.replace("\r\n", "\n")
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
