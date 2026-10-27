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
            "\nno sources are configured; add an [adapters] table (andon init prints one)"
        );
        return (out, false);
    }
    let mut rows = vec![
        ["", "source", "events", "covers", "time"]
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
        let _ = writeln!(out, "  {line}");
    }
    match failed.first() {
        None => {
            let _ = writeln!(out, "\ndoctor: every configured source reads");
        }
        Some((name, error)) => {
            let n = failed.len();
            let more = if n > 1 {
                format!(" ({n} sources cannot)")
            } else {
                String::new()
            };
            let _ = writeln!(out, "\ndoctor: {name} cannot be read{more}; fix: {error}");
        }
    }
    (out, failed.is_empty())
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
                    "cannot".to_owned(),
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
                "{}: gap: {}",
                read.name,
                e.attr_str(attr::GAP_REASON).unwrap_or("unreadable")
            )
        })
        .collect();
    let from = spans.iter().map(|e| e.t).min();
    let to = spans.iter().filter_map(|e| e.t_end).max();
    let covers = match from {
        None => "nothing".to_owned(),
        Some(f) => {
            let f = if f == ALL_HISTORY {
                "all history".to_owned()
            } else {
                display.bare_time(f)
            };
            format!(
                "{f} .. {}",
                to.map_or_else(|| "now".to_owned(), |t| display.time(t))
            )
        }
    };
    let mark = if gaps.is_empty() { "ok" } else { "gaps" };
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
