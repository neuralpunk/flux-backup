//! Small helpers for human and JSON output.

use std::io::{self, IsTerminal, Write};

use serde::Serialize;

pub fn print_json<T: Serialize + ?Sized>(v: &T) -> anyhow::Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, v)?;
    writeln!(out)?;
    Ok(())
}

/// One compact JSON value per line (for streams such as `flux watch --json`).
pub fn print_json_line<T: Serialize + ?Sized>(v: &T) -> anyhow::Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, v)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}

/// Aligned `key  value` lines.
pub fn kv(rows: &[(&str, String)]) {
    let w = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (k, v) in rows {
        println!("{k:<w$}  {v}");
    }
}

/// A plain left-aligned table with a header row.
pub fn table(header: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let mut s = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i + 1 == cells.len() {
                s.push_str(c);
            } else {
                s.push_str(&format!("{c:<w$}  ", w = widths[i]));
            }
        }
        println!("{}", s.trim_end());
    };
    let bold = io::stdout().is_terminal();
    if bold {
        print!("\x1b[1m");
    }
    line(header.to_vec());
    if bold {
        print!("\x1b[0m");
    }
    for r in rows {
        line(r.iter().map(String::as_str).collect());
    }
}

/// Ask a yes/no question on the terminal. Returns false when stdin is not a terminal.
pub fn confirm(question: &str) -> io::Result<bool> {
    if !io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("{question} [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}
