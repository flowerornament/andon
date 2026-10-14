//! What each adapter vouches for is what it read (`.design/2026-10-14-coverage.md`):
//! its coverage starts where its read starts, never earlier, and a path it
//! could not list or read is a gap, never a silent skip. Round four's
//! knockouts (`FERRY_GAP`, `JJ_EARLY`, `HAIL_EARLY`, `SESS_EARLY`) each turn one
//! of these red.

use std::path::Path;
use std::process::Command;

use andon::adapters;
use andon::config::Config;
use andon::event::{Event, Kind, Outcome};
use andon::ledger::Ledger;
use andon::time::parse_instant;
use jiff::Timestamp;

fn at(s: &str) -> Timestamp {
    parse_instant(s).unwrap_or_else(|e| panic!("{e}"))
}

fn write(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{e}"));
    }
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
}

fn read(dir: &Path, toml: &str) -> Ledger {
    let config = Config::parse(toml, dir).unwrap_or_else(|e| panic!("{e}"));
    adapters::collect(&config).unwrap_or_else(|e| panic!("{e:#}"))
}

/// Where the ledger's coverage of `kind` starts, from each source.
fn starts(l: &Ledger, kind: &str) -> Vec<Timestamp> {
    l.events()
        .iter()
        .filter(|e| e.kind == Kind::Coverage && e.outcome == Some(Outcome::Pass))
        .filter(|e| e.attr_strs("covers").contains(&kind))
        .map(|e| e.t)
        .collect()
}

fn gaps<'a>(l: &'a Ledger, kind: &str) -> Vec<&'a Event> {
    l.events()
        .iter()
        .filter(|e| e.kind == Kind::Coverage && e.outcome == Some(Outcome::Fail))
        .filter(|e| e.attr_strs("covers").contains(&kind))
        .collect()
}

/// Make `path` unreadable; `false` where permissions do not bind (root).
#[cfg(unix)]
fn lock(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::read_dir(path).is_err() && std::fs::read(path).is_err()
}

#[cfg(unix)]
fn unlock(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
}

const MSG: &str =
    "from: x-1b\nkind: ask\nid: 1013T100000-0001\ntime: 2026-10-13T10:00:00Z\nask: x\n\n";

#[test]
fn hail_covers_from_its_since() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    write(
        &dir.path()
            .join("hail/seats/x-1a/cur/1013T100000-0001.read.md"),
        MSG,
    );
    let l = read(
        dir.path(),
        "[adapters.hail]\nstate = \"hail\"\nsince = \"2026-10-12T00:00:00Z\"\n",
    );
    assert_eq!(starts(&l, "message"), vec![at("2026-10-12T00:00:00Z")]);
    assert_eq!(gaps(&l, "message").len(), 0);
}

#[cfg(unix)]
#[test]
fn hail_names_what_it_could_not_read() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let seats = dir.path().join("hail/seats");
    write(&seats.join("x-1a/cur/1013T100000-0001.read.md"), MSG);
    write(
        &seats.join("x-1b/cur/1013T100000-0002.read.md"),
        "\0\u{1} not a message",
    );
    let cur = seats.join("x-2a/cur");
    write(&cur.join("1013T100000-0003.read.md"), MSG);
    let locked = lock(&cur);
    let l = read(dir.path(), "[adapters.hail]\nstate = \"hail\"\n");
    unlock(&cur);
    let why: Vec<&str> = gaps(&l, "message")
        .iter()
        .filter_map(|g| g.attr_str("reason"))
        .collect();
    assert!(
        why.iter()
            .any(|w| w.contains("1013T100000-0002") && w.contains("not a hail message")),
        "{why:?}"
    );
    if locked {
        assert!(why.iter().any(|w| w.contains("x-2a/cur")), "{why:?}");
    }
    // the garbage file is not counted as a message
    let n = l
        .events()
        .iter()
        .filter(|e| e.kind == Kind::Message)
        .count();
    assert_eq!(n, 1);
}

const TURN: &str = r#"{"type":"assistant","timestamp":"2026-10-13T10:00:00Z","sessionId":"s","cwd":"/x-1a","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#;

#[test]
fn sessions_cover_from_their_since() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    write(
        &dir.path().join("projects/-x-1a/s.jsonl"),
        &format!("{TURN}\n"),
    );
    let l = read(
        dir.path(),
        &format!(
            "[seats.x-1a]\ncwd = [\"/x-1a\"]\n[adapters.sessions]\nclaude = [\"{}/projects/*\"]\nsince = \"2026-10-12T00:00:00Z\"\n",
            dir.path().display()
        ),
    );
    assert_eq!(
        starts(&l, "session_active"),
        vec![at("2026-10-12T00:00:00Z")]
    );
}

#[cfg(unix)]
#[test]
fn an_unlistable_sessions_directory_is_a_gap() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let sub = dir.path().join("projects/-x-1a/sub");
    write(&sub.join("s.jsonl"), &format!("{TURN}\n"));
    if !lock(&sub) {
        unlock(&sub);
        return;
    }
    let l = read(
        dir.path(),
        &format!(
            "[seats.x-1a]\ncwd = [\"/x-1a\"]\n[adapters.sessions]\nclaude = [\"{}/projects/*\"]\n",
            dir.path().display()
        ),
    );
    unlock(&sub);
    let g = gaps(&l, "session_active");
    assert_eq!(g.len(), 1, "{g:?}");
    assert!(g[0].attr_str("reason").is_some_and(|w| w.contains("sub")));
}

fn receipt(started_ms: i64) -> String {
    format!(
        r#"{{"id":"r{started_ms}","started_ms":{started_ms},"elapsed_ms":1000,"outcome":"pass","recipe":"check"}}"#
    )
}

#[test]
fn ferry_covers_from_its_oldest_receipt() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let receipts = dir.path().join("x-1a/.ferry/receipts");
    // 2026-10-12T00:00:00Z and a day later
    write(&receipts.join("a.json"), &receipt(1_760_227_200_000));
    write(&receipts.join("b.json"), &receipt(1_760_313_600_000));
    let l = read(
        dir.path(),
        &format!(
            "[adapters.ferry]\nreceipts = [\"{}/*/.ferry/receipts\"]\n",
            dir.path().display()
        ),
    );
    let oldest = l
        .events()
        .iter()
        .filter(|e| e.kind == Kind::Run)
        .map(|e| e.t)
        .min()
        .unwrap_or_else(|| panic!("runs"));
    assert_eq!(starts(&l, "run"), vec![oldest]);
}

#[cfg(unix)]
#[test]
fn ferry_gaps_name_each_unreadable_receipt_and_directory() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let a = dir.path().join("x-1a/.ferry/receipts");
    write(&a.join("ok.json"), &receipt(1_760_227_200_000));
    write(&a.join("garbage.json"), "{not json");
    write(&a.join("hello.json"), r#"{"hello":1}"#);
    let b = dir.path().join("x-1b/.ferry/receipts");
    write(&b.join("ok.json"), &receipt(1_760_227_300_000));
    let locked = lock(&b);
    let l = read(
        dir.path(),
        &format!(
            "[adapters.ferry]\nreceipts = [\"{}/*/.ferry/receipts\"]\n",
            dir.path().display()
        ),
    );
    unlock(&b);
    let why: Vec<&str> = gaps(&l, "run")
        .iter()
        .filter_map(|g| g.attr_str("reason"))
        .collect();
    assert!(why.iter().any(|w| w.contains("garbage.json")), "{why:?}");
    assert!(
        why.iter()
            .any(|w| w.contains("hello.json: not a Ferry receipt")),
        "{why:?}"
    );
    if locked {
        assert!(
            why.iter().any(|w| w.contains("x-1b/.ferry/receipts")),
            "{why:?}"
        );
    }
}

fn jj(dir: &Path, args: &[&str]) -> bool {
    Command::new("jj")
        .args(args)
        .current_dir(dir)
        .env("JJ_USER", "x")
        .env("JJ_EMAIL", "x@example.com")
        .env("JJ_CONFIG", "/dev/null")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn jj_covers_from_its_since() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
    if !jj(&repo, &["git", "init"]) || !jj(&repo, &["bookmark", "create", "main", "-r", "@"]) {
        eprintln!("skipped: jj is not available here");
        return;
    }
    let l = read(
        dir.path(),
        "[adapters.jj]\nrepo = \"repo\"\ntrunk = \"main\"\nsince = \"2026-10-12T00:00:00Z\"\n",
    );
    assert_eq!(starts(&l, "land"), vec![at("2026-10-12T00:00:00Z")]);
    assert_eq!(starts(&l, "commit"), vec![at("2026-10-12T00:00:00Z")]);
}

/// `andon ledger` over `config`, run in `tz`: the zone reaches jj and git.
fn ledger_in(tz: &str, dir: &Path) -> Vec<serde_json::Value> {
    let out = Command::new(env!("CARGO_BIN_EXE_andon"))
        .args(["--config", "andon.toml", "ledger"])
        .current_dir(dir)
        .env("TZ", tz)
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn landings(events: &[serde_json::Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["kind"] == "land")
        .filter_map(|e| e["t"].as_str().map(str::to_owned))
        .collect()
}

/// D1 (round five): a date-only `since` is a UTC day, whatever the zone jj
/// runs in; what was read, not only where coverage starts.
#[test]
fn jj_reads_a_date_since_from_utc_midnight_in_any_zone() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
    let at1 = |args: &[&str]| {
        Command::new("jj")
            .args([
                "--config",
                "debug.operation-timestamp=\"2026-10-09T01:00:00Z\"",
            ])
            .args(args)
            .current_dir(&repo)
            .env("JJ_USER", "x")
            .env("JJ_EMAIL", "x@example.com")
            .env("JJ_CONFIG", "/dev/null")
            .env("JJ_TIMESTAMP", "2026-10-09T01:00:00Z")
            .output()
            .is_ok_and(|o| o.status.success())
    };
    if !at1(&["git", "init"]) {
        eprintln!("skipped: jj is not available here");
        return;
    }
    write(&repo.join("a"), "a");
    assert!(at1(&["describe", "-m", "land a"]) && at1(&["new"]));
    assert!(at1(&["bookmark", "create", "main", "-r", "@-"]));
    write(
        &dir.path().join("andon.toml"),
        "[adapters.jj]\nrepo = \"repo\"\ntrunk = \"main\"\nsince = \"2026-10-09\"\n",
    );
    for tz in ["UTC", "America/Los_Angeles", "Pacific/Kiritimati"] {
        assert_eq!(
            landings(&ledger_in(tz, dir.path())),
            vec!["2026-10-09T01:00:00Z"],
            "TZ={tz}"
        );
    }
}

/// D2 (round five): git's approxidate reads a bare date in local time at
/// the current time of day; andon passes it a UTC instant.
#[test]
fn git_reads_a_date_since_from_utc_midnight_in_any_zone() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
    let git = |args: &[&str], at: &str| {
        Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env("GIT_AUTHOR_NAME", "x")
            .env("GIT_AUTHOR_EMAIL", "x@example.com")
            .env("GIT_COMMITTER_NAME", "x")
            .env("GIT_COMMITTER_EMAIL", "x@example.com")
            .env("GIT_AUTHOR_DATE", at)
            .env("GIT_COMMITTER_DATE", at)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .is_ok_and(|o| o.status.success())
    };
    if !git(&["init", "-q", "-b", "main"], "2026-10-09T00:00:00Z") {
        eprintln!("skipped: git is not available here");
        return;
    }
    let times = [
        "2026-10-09T02:00:00Z",
        "2026-10-09T08:00:00Z",
        "2026-10-09T20:00:00Z",
    ];
    for (i, t) in times.iter().enumerate() {
        assert!(git(
            &["commit", "-q", "--allow-empty", "-m", &format!("c{i}")],
            t
        ));
    }
    write(
        &dir.path().join("andon.toml"),
        "[adapters.git]\nrepo = \"repo\"\ntrunk = \"main\"\nsince = \"2026-10-09\"\n",
    );
    for tz in ["UTC", "America/Los_Angeles", "Pacific/Kiritimati"] {
        let mut got = landings(&ledger_in(tz, dir.path()));
        got.sort();
        assert_eq!(got, times, "TZ={tz}");
    }
}

/// C7 (round four), held by a test (round five: `BD_STATE_HIST`): an export
/// is current state, so bd covers claims and closes only as `state:*` at the
/// capture instant, never as history; creations and comments are history.
#[cfg(unix)]
#[test]
fn bd_covers_claims_and_closes_only_as_state_at_capture() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let bin = dir.path().join("bin");
    let issue = r#"{"id":"x-1","title":"t","status":"in_progress","created_at":"2026-10-01T00:00:00Z","started_at":"2026-10-02T00:00:00Z","assignee":"x-1a"}"#;
    write(
        &bin.join("bd"),
        &format!("#!/bin/sh\n[ \"$1\" = export ] && printf '%s\\n' '{issue}'\n"),
    );
    std::fs::set_permissions(bin.join("bd"), std::fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("{e}"));
    write(
        &dir.path().join("andon.toml"),
        "[adapters.bd]\ndir = \".\"\n",
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_andon"))
        .args(["--config", "andon.toml", "ledger"])
        .current_dir(dir.path())
        .env("PATH", path)
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let events: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let covers = |e: &serde_json::Value, k: &str| {
        e["attrs"]["covers"]
            .as_array()
            .is_some_and(|c| c.iter().any(|x| x == k))
    };
    let coverage: Vec<&serde_json::Value> =
        events.iter().filter(|e| e["kind"] == "coverage").collect();
    for kind in ["claim", "close"] {
        assert!(
            !coverage.iter().any(|e| covers(e, kind)),
            "bd claims {kind} history: {coverage:?}"
        );
        let state: Vec<_> = coverage
            .iter()
            .filter(|e| covers(e, &format!("state:{kind}")))
            .collect();
        assert_eq!(state.len(), 1, "{coverage:?}");
        assert_eq!(state[0]["t"], state[0]["t_end"], "a state is one instant");
    }
    assert!(
        coverage
            .iter()
            .any(|e| covers(e, "create") && e["t"] == "1970-01-01T00:00:00Z")
    );
}

/// D8 (round five): the sender's outbox copy names no sender; its seat
/// directory does. It is a message, not a gap.
#[test]
fn a_pending_copy_is_its_seats_message() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    write(
        &dir.path().join("hail/seats/x-1b/pending/1013T100000-0009"),
        "kind: ask\nid: 1013T100000-0009\ntime: 2026-10-13T10:00:00Z\nask: x\n\n",
    );
    let l = read(dir.path(), "[adapters.hail]\nstate = \"hail\"\n");
    assert_eq!(gaps(&l, "message").len(), 0);
    let m: Vec<&Event> = l
        .events()
        .iter()
        .filter(|e| e.kind == Kind::Message)
        .collect();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].actor.as_deref(), Some("x-1b"));
}

/// E2 (round six): the read margin. A change committed before `since` and
/// put on the trunk after it is a landing after `since`; a read that starts
/// at `since` by committer date misses it.
#[test]
fn jj_reads_a_change_committed_before_since_and_landed_after() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
    let jj_at = |op: &str, commit: &str, args: &[&str]| {
        Command::new("jj")
            .args(["--config", &format!("debug.operation-timestamp=\"{op}\"")])
            .args(args)
            .current_dir(&repo)
            .env("JJ_USER", "x")
            .env("JJ_EMAIL", "x@example.com")
            .env("JJ_CONFIG", "/dev/null")
            .env("JJ_TIMESTAMP", commit)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    let before = "2026-10-08T23:55:00Z";
    if !jj_at(before, before, &["git", "init"]) {
        eprintln!("skipped: jj is not available here");
        return;
    }
    let early = "2026-10-01T00:00:00Z";
    write(&repo.join("base"), "base");
    assert!(jj_at(early, early, &["describe", "-m", "base"]));
    assert!(jj_at(
        early,
        early,
        &["bookmark", "create", "main", "-r", "@"]
    ));
    assert!(jj_at(early, early, &["new"]));
    write(&repo.join("a"), "a");
    assert!(jj_at(before, before, &["describe", "-m", "land a"]));
    assert!(jj_at(before, before, &["new"]));
    // the trunk moves onto it seven minutes after `since`
    assert!(jj_at(
        "2026-10-09T00:02:00Z",
        before,
        &["bookmark", "set", "main", "-r", "@-"]
    ));
    write(
        &dir.path().join("andon.toml"),
        "[adapters.jj]\nrepo = \"repo\"\ntrunk = \"main\"\nsince = \"2026-10-09\"\n",
    );
    assert_eq!(
        landings(&ledger_in("UTC", dir.path())),
        vec!["2026-10-09T00:02:00Z"]
    );
}
