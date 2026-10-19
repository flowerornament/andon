//! The hook's cheap read must give the seat the same row as the full read
//! (`board`). Fixture: a Claude Code transcript in the seat's directory, a
//! Codex rollout that starts in another directory and then works in the
//! seat's, another seat's session, and a claim from an external adapter.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the repositories and files andon reads"
)]

use std::path::Path;

use andon::adapters;
use andon::config::Config;
use andon::fold::board::board;
use andon::time::parse_instant;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap_or(path)).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
}

fn call(t: &str, cwd: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{t}","sessionId":"c1","cwd":"{cwd}","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{}}}}]}}}}"#
    )
}

#[test]
fn the_hooks_row_is_the_boards_row() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path();
    let seat_dir = root.join("code/x-1a");
    let elsewhere = root.join("code/x-dev");
    let other = root.join("code/x-1b");
    std::fs::create_dir_all(seat_dir.join(".jj")).unwrap_or_else(|e| panic!("{e}"));
    let seat = seat_dir.display().to_string();

    // Claude Code: two spans in the seat's directory
    let claude = [
        call("2026-10-13T10:00:00Z", &seat),
        call("2026-10-13T10:05:00Z", &seat),
        call("2026-10-13T11:00:00Z", &seat),
    ]
    .join("\n");
    write(&root.join("claude/projects/-code-x-1a/c1.jsonl"), &claude);
    // another seat's session, which the seat's row must not count
    let theirs = call("2026-10-13T10:30:00Z", &other.display().to_string());
    write(
        &root.join("claude/projects/-code-x-1b/c2.jsonl"),
        &theirs.replace("c1", "c2"),
    );

    // Codex: starts in x-dev, then works in the seat's directory
    let codex = [
        format!(r#"{{"timestamp":"2026-10-13T09:00:00Z","type":"session_meta","payload":{{"id":"r1","cwd":"{}"}}}}"#, elsewhere.display()),
        format!(r#"{{"timestamp":"2026-10-13T12:00:00Z","type":"turn_context","payload":{{"cwd":"{seat}"}}}}"#),
        r#"{"timestamp":"2026-10-13T12:00:01Z","type":"response_item","payload":{"type":"function_call","name":"exec"}}"#.to_owned(),
        format!(r#"{{"timestamp":"2026-10-13T12:05:00Z","type":"turn_context","payload":{{"cwd":"{seat}"}}}}"#),
        r#"{"timestamp":"2026-10-13T12:05:01Z","type":"response_item","payload":{"type":"function_call","name":"exec"}}"#.to_owned(),
    ]
    .join("\n");
    write(&root.join("codex/2026/10/13/rollout-r1.jsonl"), &codex);

    // the claim and its budget, from a project-owned external adapter
    let claims = [
        r#"{"t":"1970-01-01T00:00:00Z","kind":"coverage","outcome":"pass","source":"tracker","attrs":{"covers":["create","claim","close"]}}"#,
        r#"{"t":"2026-10-13T08:00:00Z","kind":"create","subject":"x-goal","ids":{"goal":"x-goal"},"source":"tracker","attrs":{"budget":"4 agent-hours"}}"#,
        r#"{"t":"2026-10-13T08:30:00Z","kind":"claim","actor":"x-1a","subject":"x-goal","ids":{"goal":"x-goal"},"source":"tracker"}"#,
    ]
    .join("\n");
    write(&root.join("tracker.jsonl"), &claims);

    let config = Config::parse(
        &format!(
            r#"
[seats.x-1a]
[seats.x-1b]
[adapters.sessions]
claude = ["{root}/claude/projects/*"]
codex = ["{root}/codex"]
cwd = ["{root}/code/x*"]
[[adapters.external]]
name = "x-tracker"
files = ["{root}/tracker.jsonl"]
"#,
            root = root.display()
        ),
        root,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let now = parse_instant("2026-10-13T13:00:00Z").unwrap_or_else(|e| panic!("{e}"));
    let full = adapters::collect(&config).unwrap_or_else(|e| panic!("{e:#}"));
    let cheap = adapters::collect_for_seat(&config, "x-1a", &seat_dir, now)
        .unwrap_or_else(|e| panic!("{e:#}"));
    let row = |ledger: &andon::ledger::Ledger| {
        let b = board(ledger, &config, now);
        let r = b.seats.into_iter().find(|r| r.seat == "x-1a");
        serde_json::to_string(&r).unwrap_or_default()
    };
    assert_eq!(row(&full), row(&cheap));
    // and the row counts the rollout that began elsewhere: 5 + 0 + 5 minutes
    let b = board(&full, &config, now);
    let used = b.seats[0]
        .goals
        .get()
        .and_then(|g| g.first())
        .and_then(|g| g.used.get().copied());
    assert_eq!(used, Some(0.17), "spans: 10:00-10:05, 11:00, 12:00-12:05");
}
