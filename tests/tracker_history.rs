//! Tracker history from bd's events table (andon-nqi): claims, closes,
//! reopens and assignments, each row's stored time resolved to an instant.
//! bd once stored local time labelled UTC and later true UTC; the rows
//! whose issue timestamps pin them say which, and a change bd's events
//! never recorded is put where the export's present state says it was.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the stand-in bd andon reads"
)]
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

/// A stand-in `bd`: `export` prints the issues, `sql` the event rows.
fn bd(dir: &Path, issues: &[&str], rows: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(dir.join("issues.jsonl"), issues.join("\n") + "\n")
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(dir.join("rows.json"), rows).unwrap_or_else(|e| panic!("{e}"));
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\n  export) cat '{d}/issues.jsonl' ;;\n  sql) cat '{d}/rows.json' ;;\nesac\n",
        d = dir.display()
    );
    std::fs::write(bin.join("bd"), script).unwrap_or_else(|e| panic!("{e}"));
    std::fs::set_permissions(bin.join("bd"), std::fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("{e}"));
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn row(issue: &str, kind: &str, at: &str, new: &str) -> String {
    serde_json::json!({
        "issue_id": issue, "event_type": kind, "actor": "x-1a",
        "new_value": new, "created_at": at, "old_assignee": null, "old_known": 0,
    })
    .to_string()
}

fn tracker(dir: &Path, path: &str, window: &str) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_andon"))
        .args(["scorecard", window, "--json"])
        .current_dir(dir)
        .env("PATH", path)
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    v["tracker"].clone()
}

#[test]
fn claims_closes_and_reopens_are_history_with_each_rows_time_resolved() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    // x-a: created 10-01 10:00, claimed, closed and reopened that day, all
    // stored as local time (UTC-7) labelled UTC. x-b: created 10-10,
    // closed, reopened and closed again, stored as true UTC. x-c: claimed
    // 10-10 09:10, its assignee cleared by 09:30 with no row recorded
    let issues = [
        r#"{"id":"x-a","title":"a","status":"open","assignee":"x-1a","created_at":"2026-10-01T10:00:00Z","updated_at":"2026-10-01T14:00:00Z"}"#,
        r#"{"id":"x-b","title":"b","status":"closed","created_at":"2026-10-10T10:00:00Z","closed_at":"2026-10-10T12:00:00Z","updated_at":"2026-10-10T12:00:00Z"}"#,
        r#"{"id":"x-c","title":"c","status":"open","created_at":"2026-10-10T09:00:00Z","updated_at":"2026-10-10T09:30:00Z"}"#,
    ];
    let claim = r#"{"assignee":"x-1a","status":"in_progress"}"#;
    let rows = [
        row("x-a", "created", "2026-10-01T03:00:00Z", ""),
        row("x-a", "claimed", "2026-10-01T05:00:00Z", claim),
        row("x-a", "closed", "2026-10-01T06:00:00Z", "done"),
        row("x-a", "reopened", "2026-10-01T07:00:00Z", ""),
        row("x-b", "created", "2026-10-10T10:00:00Z", ""),
        row("x-b", "closed", "2026-10-10T11:00:00Z", "done"),
        row("x-b", "reopened", "2026-10-10T11:30:00Z", ""),
        row("x-b", "closed", "2026-10-10T12:00:00Z", "done again"),
        row("x-c", "created", "2026-10-10T09:00:00Z", ""),
        row("x-c", "claimed", "2026-10-10T09:10:00Z", claim),
    ];
    let path = bd(dir.path(), &issues, &format!("[{}]", rows.join(",")));
    std::fs::write(
        dir.path().join("andon.toml"),
        "[seats.x-1a]\n[adapters.bd]\ndir = \".\"\nevents_zone = \"XST7\"\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let at = |to: &str| tracker(dir.path(), &path, &format!("2026-10-01T00:00:00Z..{to}"));

    // x-a's rows fall between a local pin (its creation) and a UTC one
    // (x-b's): read as local time, they are 10-01 12:00 to 14:00
    let before = at("2026-10-01T13:30:00Z");
    assert_eq!(before["closed"], 1, "closed at 13:00: {before}");
    assert_eq!(before["open_at_end"], 0, "not yet reopened: {before}");
    let after = at("2026-10-01T14:30:00Z");
    assert_eq!(after["open_at_end"], 1, "x-a reopened at 14:00: {after}");
    assert_eq!(after["unowned_at_end"], 0, "x-a is x-1a's: {after}");
    // x-b closed twice is one item closed
    let tenth = tracker(dir.path(), &path, "2026-10-10");
    assert_eq!(tenth["closed"], 1, "{tenth}");
    // x-c's cleared assignee: unknown while it could have happened, then
    // x-c is nobody's
    let during = at("2026-10-10T09:20:00Z");
    assert!(during["unowned_at_end"]["unknown"].is_string(), "{during}");
    let later = at("2026-10-10T09:40:00Z");
    assert_eq!(later["unowned_at_end"], 1, "{later}");
}

#[test]
fn without_bds_events_claims_and_closes_stay_current_state() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let issues = [
        r#"{"id":"x-a","title":"a","status":"closed","created_at":"2026-10-01T10:00:00Z","closed_at":"2026-10-01T12:00:00Z"}"#,
    ];
    // an embedded bd prints no rows: the export is all there is
    let path = bd(dir.path(), &issues, "");
    std::fs::write(
        dir.path().join("andon.toml"),
        "[seats.x-1a]\n[adapters.bd]\ndir = \".\"\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let day = tracker(dir.path(), &path, "2026-10-01");
    assert!(day["open_at_end"]["unknown"].is_string(), "{day}");
}
