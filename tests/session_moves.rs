//! A session that moves between workspaces is each seat's for its own part
//! (andon-90p): before, the whole session went to the directory it used
//! most, so the seat it moved to was credited nothing.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures write the transcripts andon reads"
)]

mod common;

use andon::adapters;
use andon::fold::scorecard::scorecard;
use andon::time::{Window, parse_instant};

#[test]
fn a_session_moving_between_workspaces_credits_each_seat_its_part() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path();
    let call = |t: &str, ws: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"{t}","sessionId":"s1","cwd":"{}/code/{ws}","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{}}}}]}}}}"#,
            root.display()
        )
    };
    // 10:00-10:30 in x-1a, then 10:30-11:00 in x-1b
    let lines = [
        call("2026-10-13T10:00:00Z", "x-1a"),
        call("2026-10-13T10:10:00Z", "x-1a"),
        call("2026-10-13T10:20:00Z", "x-1a"),
        call("2026-10-13T10:30:00Z", "x-1b"),
        call("2026-10-13T11:00:00Z", "x-1b"),
    ];
    let file = root.join("claude/projects/-code-x/s1.jsonl");
    std::fs::create_dir_all(file.parent().unwrap_or(root)).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&file, lines.join("\n") + "\n").unwrap_or_else(|e| panic!("{e}"));
    let config = common::config(
        &format!(
            "[seats.x-1a]\n[seats.x-1b]\n[adapters.sessions]\nclaude = [\"{root}/claude/projects/*\"]\ncwd = [\"{root}/code/x*\"]\ngap_minutes = 45\n",
            root = root.display()
        ),
        root,
    );
    let ledger = adapters::collect(&config).unwrap_or_else(|e| panic!("{e:#}"));
    let at = |s: &str| parse_instant(s).unwrap_or_else(|e| panic!("{e}"));
    let window =
        Window::parse("2026-10-13", at("2026-10-14T00:00:00Z")).unwrap_or_else(|e| panic!("{e}"));
    let card = scorecard(&ledger, &config, window);
    let hours = |seat: &str| card.seats[seat].active_hours.get().copied();
    assert_eq!((hours("x-1a"), hours("x-1b")), (Some(0.5), Some(0.5)));
}
