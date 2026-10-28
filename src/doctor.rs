//! `andon doctor`: what each configured source read, in full. The config it
//! loaded; each adapter's result (its events, the span it covers, its gaps
//! and its time, or the whole error); then the hook's wiring as `hook
//! --check` reports it. It fails, naming the fix, when a configured source
//! cannot be read.

use std::fmt::Write as _;
use std::path::Path;

use jiff::Timestamp;

use crate::adapters;
use crate::config::Config;
use crate::event::{ALL_HISTORY, Kind, Outcome, attr};
use crate::render::{self, Display};

/// The report, and whether every configured source read.
#[must_use]
pub fn doctor(
    path: &Path,
    config: &Config,
    cwd: &Path,
    now: Timestamp,
    display: &Display,
) -> (String, bool) {
    let mut out = String::new();
    let _ = writeln!(out, "config  {}", path.display());
    let reads = adapters::read_each(config);
    if reads.is_empty() {
        let _ = writeln!(
            out,
            "\nno adapters configured; add an [adapters.<name>] table (andon init prints a starter config)"
        );
        return (out, false);
    }
    let mut rows = vec![
        ["", "adapter", "events", "available", "time"]
            .map(str::to_owned)
            .to_vec(),
    ];
    let mut notes = Vec::new();
    let mut failed = Vec::new();
    for read in &reads {
        let (row, more) = source_row(read, display);
        if let Err(error) = &read.got {
            failed.push((read.name.clone(), format!("{error:#}")));
        }
        rows.push(row);
        notes.extend(more);
    }
    out.push('\n');
    out.push_str(&render::aligned(&rows, &[false, false, true, false, true]));
    if !notes.is_empty() {
        out.push('\n');
        for note in &notes {
            out.push_str(&render::wrap(note, display.width, 2));
            out.push('\n');
        }
    }
    out.push_str(&columns_off(&reads, display));
    // the hook, as a session starting here would run it
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "cwd": cwd.display().to_string(),
    })
    .to_string();
    let (report, _) = crate::hook::check(
        &crate::hook::Sources {
            config: Some(path),
            ledger: None,
        },
        &payload,
        now,
    );
    let _ = writeln!(out, "\nhook, for a session starting in {}:", cwd.display());
    for line in report.lines() {
        let _ = writeln!(
            out,
            "{}",
            render::wrap(&format!("  {line}"), display.width, 8)
        );
    }
    match failed.first() {
        None => {
            let partial = match notes.len() {
                0 => String::new(),
                n => format!(" ({n} with warnings above)"),
            };
            let _ = writeln!(out, "\nok: every adapter read{partial}");
        }
        Some((name, error)) => {
            let n = failed.len();
            let more = if n > 1 {
                format!(" ({n} adapters failed)")
            } else {
                String::new()
            };
            let name = name.strip_prefix("external ").map_or_else(
                || format!("[adapters.{name}]"),
                |n| format!("external adapter {n}"),
            );
            let _ = writeln!(out, "\nerror: {name} failed{more}: {error}");
        }
    }
    (out, failed.is_empty())
}

/// The board's columns no configured adapter feeds, grouped by the config
/// that would add one, as the board lists them.
fn columns_off(reads: &[adapters::AdapterRead], display: &Display) -> String {
    const COLUMNS: &[(&str, &str)] = &[
        ("goal", "claim"),
        ("budget", "session_active"),
        ("lands", "land"),
        ("landed", "land"),
        ("runs", "run"),
        ("pass", "run"),
        ("unlanded", "run"),
        ("verdict", "verdict"),
        ("comments", "other:comment"),
        ("state", "state:presence"),
    ];
    let events: Vec<_> = reads
        .iter()
        .filter_map(|r| r.got.as_ref().ok())
        .flatten()
        .cloned()
        .collect();
    let ledger = crate::ledger::Ledger::new(events);
    let coverage = crate::fold::coverage::Coverage::new(&ledger);
    let mut off: Vec<(&str, Vec<&str>)> = Vec::new();
    for (column, kind) in COLUMNS {
        if coverage.reads(kind) || coverage.reads(&format!("state:{kind}")) {
            continue;
        }
        let hint = crate::fold::coverage::enable(kind);
        match off.iter_mut().find(|(h, _)| *h == hint) {
            Some((_, cols)) => cols.push(column),
            None => off.push((hint, vec![column])),
        }
    }
    if off.is_empty() {
        return String::new();
    }
    let lead: Vec<String> = off.iter().map(|(_, cols)| cols.join(", ")).collect();
    let pad = lead.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let mut out = String::from("\nboard columns not configured:\n");
    for ((hint, _), cols) in off.iter().zip(&lead) {
        out.push_str(&render::wrap(
            &format!("  {cols:<pad$}  {hint}"),
            display.width,
            pad + 4,
        ));
        out.push('\n');
    }
    out
}

/// One source's row (mark, name, events, span covered, time) and its notes:
/// each gap, or its whole error.
fn source_row(read: &adapters::AdapterRead, display: &Display) -> (Vec<String>, Vec<String>) {
    let time = format!("{:.2}s", read.wall.as_secs_f64());
    let events = match &read.got {
        Ok(events) => events,
        Err(error) => {
            return (
                vec![
                    "error".to_owned(),
                    read.name.clone(),
                    "—".to_owned(),
                    "—".to_owned(),
                    time,
                ],
                vec![format!("{}: {error:#}", read.name)],
            );
        }
    };
    let facts = events.iter().filter(|e| e.kind != Kind::Coverage).count();
    let spans: Vec<_> = events
        .iter()
        .filter(|e| e.kind == Kind::Coverage && e.outcome != Some(Outcome::Fail))
        .collect();
    let gaps: Vec<String> = events
        .iter()
        .filter(|e| e.kind == Kind::Coverage && e.outcome == Some(Outcome::Fail))
        .map(|e| {
            format!(
                "{}: {}",
                read.name,
                e.attr_str(attr::GAP_REASON).unwrap_or("unreadable")
            )
        })
        .collect();
    let from = spans.iter().map(|e| e.t).min();
    let to = spans.iter().filter_map(|e| e.t_end).max();
    let covers = match from {
        None => "no data".to_owned(),
        Some(f) => {
            let f = if f == ALL_HISTORY {
                "all".to_owned()
            } else {
                display.bare_time(f)
            };
            format!(
                "{f} .. {}",
                to.map_or_else(|| "now".to_owned(), |t| display.time(t))
            )
        }
    };
    let mark = if gaps.is_empty() { "ok" } else { "partial" };
    (
        vec![
            mark.to_owned(),
            read.name.clone(),
            facts.to_string(),
            covers,
            time,
        ],
        gaps,
    )
}
