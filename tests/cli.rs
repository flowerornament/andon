//! The command line's contract: exit codes and one-line stdout.

use std::path::Path;
use std::process::{Command, Output};

fn andon(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_andon"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|error| panic!("andon did not run: {error}"))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn empty_project(config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(dir.path().join("andon.toml"), config).unwrap_or_else(|e| panic!("{e}"));
    dir
}

#[test]
fn version_names_the_binary() {
    let out = andon(&["--version"], Path::new("."));
    assert!(out.status.success());
    assert_eq!(
        stdout(&out).trim(),
        concat!("andon ", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn no_config_exits_2_and_names_the_fix() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let out = andon(&["board"], dir.path());
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stdout(&out),
        "andon: cannot decide: no andon.toml here or above; create one: andon init > andon.toml\n"
    );
}

#[test]
fn init_prints_a_config_that_andon_reads() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let out = andon(&["init"], dir.path());
    assert!(out.status.success());
    std::fs::write(dir.path().join("andon.toml"), &out.stdout).unwrap_or_else(|e| panic!("{e}"));
    // the starter names jj and bd, which an empty directory lacks: andon says
    // it cannot decide rather than drawing an empty board
    let board = andon(&["board"], dir.path());
    assert_eq!(board.status.code(), Some(2));
    assert!(stdout(&board).starts_with("andon: cannot decide: adapter jj"));
}

#[test]
fn a_refused_message_exits_1_with_one_line() {
    let dir = empty_project("[seats.a]\n");
    let out = andon(
        &["check", "message", "ruling", "--text", "do it"],
        dir.path(),
    );
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert_eq!(text.lines().count(), 1);
    assert!(text.starts_with("refused: a ruling carries a decision"));
    let ok = andon(&["check", "message", "fyi", "--text", "status"], dir.path());
    assert_eq!(ok.status.code(), Some(0));
}

#[test]
fn a_goal_check_refuses_missing_sections_and_warns_on_a_vague_done_when() {
    use std::io::Write;
    let dir = empty_project("[seats.a]\n");
    let run = |text: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_andon"))
            .args(["check", "goal", "-"])
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("{e}"));
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(text).unwrap_or_else(|e| panic!("{e}"));
        }
        child.wait_with_output().unwrap_or_else(|e| panic!("{e}"))
    };
    let missing = run(b"## Why\n\nx\n\n## Budget\n\n2 agent-hours\n");
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(
        stdout(&missing),
        "refused: the description: add a \"## Done when\" section\n"
    );
    let vague =
        run(b"## Why\n\nx\n\n## Done when\n\nimprove things\n\n## Budget\n\n2 agent-hours\n");
    assert_eq!(vague.status.code(), Some(0));
    assert_eq!(
        stdout(&vague),
        "warn: the description: the Done when section names no command, number or limit a check could fail on: \"improve things\"\n"
    );
}

#[test]
fn the_hook_is_silent_outside_a_project_and_never_fails() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_andon"))
        .arg("hook")
        .current_dir(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{e}"));
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(b"{\"hook_event_name\":\"SessionStart\"}")
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let out = child.wait_with_output().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "");
}

#[test]
fn the_hook_names_bad_input_in_one_line_and_still_exits_0() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_andon"))
        .arg("hook")
        .current_dir(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{e}"));
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(b"garbage")
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let out = child.wait_with_output().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stdout(&out),
        "andon: hook cannot decide: the hook payload on stdin is not JSON; the harness hook should pipe its JSON payload to `andon hook`\n"
    );
}

#[test]
fn a_kept_ledger_replaces_the_sources() {
    let dir = empty_project("[seats.andon-1b]\n");
    let ledger = dir.path().join("ledger.jsonl");
    std::fs::write(
        &ledger,
        concat!(
            r#"{"t":"2026-10-13T10:00:00Z","actor":"andon-1b","seat":"andon-1b","kind":"land","ids":{"commit":"abc"},"outcome":"pass","source":"test"}"#,
            "\n",
            r#"{"t":"2026-10-01T00:00:00Z","kind":"coverage","outcome":"pass","source":"test","attrs":{"covers":["land"]}}"#,
            "\n"
        ),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let out = andon(
        &[
            "--ledger",
            "ledger.jsonl",
            "asof",
            "2026-10-13T12:00:00Z",
            "--json",
        ],
        dir.path(),
    );
    assert!(out.status.success(), "{}", stdout(&out));
    let board: serde_json::Value =
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(board["seats"][0]["seat"], "andon-1b");
    assert_eq!(board["seats"][0]["landings"], 1);
    // runs were never covered: unknown, with the reason, never 0
    assert_eq!(board["seats"][0]["runs"]["unknown"], "no source reads runs");
}

fn hook_with(args: &[&str], dir: &Path, payload: &str) -> Output {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_andon"))
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{e}"));
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(payload.as_bytes())
            .unwrap_or_else(|e| panic!("{e}"));
    }
    child.wait_with_output().unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn hook_check_reports_the_wiring_and_exits_1_naming_the_fix() {
    let dir = empty_project("[seats.x-1a]\n");
    std::fs::create_dir_all(dir.path().join("x-1a")).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(dir.path().join("ledger.jsonl"), "").unwrap_or_else(|e| panic!("{e}"));
    let cwd = dir.path().join("x-1a");
    let payload = |event: &str, transcript: &str| {
        format!(
            r#"{{"hook_event_name":"{event}","cwd":"{}"{transcript}}}"#,
            cwd.display()
        )
    };
    // wired right for a session start
    let ok = hook_with(
        &["--ledger", "ledger.jsonl", "hook", "--check"],
        dir.path(),
        &payload("SessionStart", ""),
    );
    assert_eq!(ok.status.code(), Some(0), "{}", stdout(&ok));
    assert!(
        stdout(&ok).contains("ok      seat: x-1a"),
        "{}",
        stdout(&ok)
    );
    // a prompt whose transcript cannot be read: refused, naming why
    let bad = payload("UserPromptSubmit", r#","transcript_path":"/nope""#);
    let refused = hook_with(
        &["--ledger", "ledger.jsonl", "hook", "--check"],
        dir.path(),
        &bad,
    );
    assert_eq!(refused.status.code(), Some(1));
    assert!(stdout(&refused).contains("refused transcript: cannot read a last turn from /nope"));
    // and the hook itself says so on the prompt, still exiting 0
    let said = hook_with(&["--ledger", "ledger.jsonl", "hook"], dir.path(), &bad);
    assert_eq!(said.status.code(), Some(0));
    assert!(stdout(&said).contains("hook cannot tell what changed since the last turn"));
    // no andon.toml anywhere above the desk: refused, naming the fix
    let bare = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let none = hook_with(
        &["hook", "--check"],
        bare.path(),
        &format!(
            r#"{{"hook_event_name":"SessionStart","cwd":"{}"}}"#,
            bare.path().display()
        ),
    );
    assert_eq!(none.status.code(), Some(1));
    assert!(stdout(&none).contains("refused config: no andon.toml"));
}

/// andon-e10: every column the board and scorecard print is defined, one
/// line each, in its command's `--help`.
#[test]
fn every_column_is_defined_in_help() {
    let help = |cmd: &str| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_andon"))
            .args([cmd, "--help"])
            .output()
            .unwrap_or_else(|e| panic!("{e}"));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let defined = |text: &str, name: &str| {
        text.lines()
            .any(|l| l.trim_start().starts_with(&format!("{name}: ")))
    };
    let board = help("board");
    for name in andon::render::board_header().iter().skip(1) {
        assert!(
            defined(&board, name),
            "board --help does not define {name}:\n{board}"
        );
    }
    for flag in [
        "over budget",
        "goals in progress",
        "no landing in",
        "last land refused",
        "runs since last landing",
    ] {
        assert!(
            board.contains(flag),
            "board --help does not explain the flag {flag:?}"
        );
    }
    let card = help("scorecard");
    for name in andon::render::SCORECARD_HEADER.iter().skip(1) {
        assert!(
            defined(&card, name),
            "scorecard --help does not define {name}:\n{card}"
        );
    }
}
