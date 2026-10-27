//! `andon goals`, `seats`, `doctor` and `init` (andon-2fh): each is plain on
//! a pipe, doctor fails naming the fix when a source cannot be read, and
//! init's output for a jj and bd repository loads as a config.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the repositories and configs andon reads"
)]
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

fn andon(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_andon"))
        .args(args)
        .current_dir(dir)
        .env("ANDON_CACHE", "off")
        .env("JJ_CONFIG", dir.join("jj-config.toml"))
        .output()
        .unwrap_or_else(|e| panic!("{e}"))
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn doctor_fails_naming_the_fix_when_a_source_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let line = r#"{"t":"1970-01-01T00:00:00Z","kind":"coverage","outcome":"pass","source":"x","attrs":{"covers":["land"]}}"#;
    std::fs::write(dir.path().join("events.sh"), format!("echo '{line}'\n"))
        .unwrap_or_else(|e| panic!("{e}"));
    let good = "[seats.x-1a]\n[[adapters.external]]\nname = \"x-good\"\ncommand = [\"sh\", \"events.sh\"]\n";
    std::fs::write(dir.path().join("andon.toml"), good).unwrap_or_else(|e| panic!("{e}"));
    let ok = andon(dir.path(), &["doctor"]);
    assert_eq!(ok.status.code(), Some(0), "{}", stdout(&ok));
    assert!(
        stdout(&ok).contains("ok  external x-good"),
        "{}",
        stdout(&ok)
    );
    assert!(
        stdout(&ok).ends_with("doctor: every configured source reads\n"),
        "{}",
        stdout(&ok)
    );

    let bad = format!(
        "{good}[[adapters.external]]\nname = \"x-bad\"\ncommand = [\"sh\", \"-c\", \"echo 'no store at /x' >&2; exit 3\"]\n"
    );
    std::fs::write(dir.path().join("andon.toml"), bad).unwrap_or_else(|e| panic!("{e}"));
    let failed = andon(dir.path(), &["doctor"]);
    let text = stdout(&failed);
    assert_eq!(failed.status.code(), Some(1), "{text}");
    assert!(text.contains("cannot  external x-bad"), "{text}");
    let last = text.lines().last().unwrap_or_default();
    assert!(
        last.starts_with("doctor: external x-bad cannot be read; fix: ")
            && last.contains("no store at /x"),
        "{last}"
    );
}

#[test]
fn init_detects_a_jj_and_bd_repository() {
    let jj_runs = Command::new("jj")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !jj_runs {
        eprintln!("skipped: jj is not available here");
        return;
    }
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path().join("demo");
    std::fs::create_dir_all(root.join(".beads")).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        root.join("jj-config.toml"),
        "[user]\nname = \"Test\"\nemail = \"test@example.com\"\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(root.join(".beads/config.yaml"), "issue-prefix: dm\n")
        .unwrap_or_else(|e| panic!("{e}"));
    let jj = |cwd: &Path, args: &[&str]| {
        let out = Command::new("jj")
            .args(args)
            .current_dir(cwd)
            .env("JJ_CONFIG", root.join("jj-config.toml"))
            .output()
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            out.status.success(),
            "jj {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    jj(&root, &["git", "init"]);
    jj(&root, &["commit", "-m", "init"]);
    jj(&root, &["bookmark", "set", "master", "-r", "@-"]);
    jj(
        &root,
        &["workspace", "add", "--name", "demo-1b", "../demo-1b"],
    );
    // run in the second workspace: the project is still the repository
    let second = dir.path().join("demo-1b");
    std::fs::copy(root.join("jj-config.toml"), second.join("jj-config.toml"))
        .unwrap_or_else(|e| panic!("{e}"));
    let out = andon(&second, &["init"]);
    let text = stdout(&out);
    assert!(out.status.success(), "{text}");
    let config =
        andon::config::Config::parse(&text, &root).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert_eq!(config.project.name, "demo", "{text}");
    let seats: Vec<&str> = config.seats.keys().map(String::as_str).collect();
    assert_eq!(seats, vec!["demo", "demo-1b"], "{text}");
    assert!(
        config.seats["demo"].aliases.contains(&"default".to_owned()),
        "{text}"
    );
    assert!(config.project.goal_pattern.contains("dm-"), "{text}");
    assert_eq!(config.messages.decision_kinds, vec!["ask".to_owned()]);
    let jj_config = config
        .adapters
        .jj
        .as_ref()
        .unwrap_or_else(|| panic!("{text}"));
    assert_eq!(jj_config.trunk, "master");
    assert!(config.adapters.bd.is_some(), "{text}");
}

#[test]
fn goals_and_seats_are_plain_on_a_pipe() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let ledger = [
        r#"{"t":"1970-01-01T00:00:00Z","kind":"coverage","outcome":"pass","source":"t","attrs":{"covers":["create","claim","close","reopen","assign","session_active"]}}"#,
        r#"{"t":"2026-10-13T07:00:00Z","kind":"create","subject":"x-abc","ids":{"goal":"x-abc"},"source":"t","attrs":{"budget":"4 agent-hours","title":"the board"}}"#,
        r#"{"t":"2026-10-13T08:00:00Z","kind":"claim","actor":"x-1a","subject":"x-abc","ids":{"goal":"x-abc"},"source":"t"}"#,
        r#"{"t":"2026-10-13T07:10:00Z","kind":"create","subject":"x-def","ids":{"goal":"x-def"},"source":"t","attrs":{"title":"nobody's yet"}}"#,
        r#"{"t":"2026-10-13T07:20:00Z","kind":"create","subject":"x-old","ids":{"goal":"x-old"},"source":"t","attrs":{"title":"done"}}"#,
        r#"{"t":"2026-10-13T07:30:00Z","kind":"close","subject":"x-old","ids":{"goal":"x-old"},"source":"t"}"#,
        r#"{"t":"2026-10-13T08:30:00Z","kind":"claim","actor":"Shared User","subject":"x-ghi","ids":{"goal":"x-ghi"},"source":"t"}"#,
    ]
    .join("\n");
    std::fs::write(dir.path().join("ledger.jsonl"), ledger + "\n")
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        dir.path().join("andon.toml"),
        "[goal]\n[seats.x-1a]\n[seats.x-1b]\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let goals = stdout(&andon(dir.path(), &["--ledger", "ledger.jsonl", "goals"]));
    let ids: Vec<&str> = goals
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    // claimed first, newest claim first; then open; closed only with --all
    assert_eq!(&ids[..3], ["x-ghi", "x-abc", "x-def"], "{goals}");
    assert!(!goals.contains("x-old"), "{goals}");
    assert!(goals.contains("Shared User*"), "{goals}");
    assert!(!goals.contains('\x1b'), "no color on a pipe: {goals}");
    let all = stdout(&andon(
        dir.path(),
        &["--ledger", "ledger.jsonl", "goals", "--all"],
    ));
    assert!(all.contains("x-old"), "{all}");
    let mine = stdout(&andon(
        dir.path(),
        &["--ledger", "ledger.jsonl", "goals", "--seat", "x-1a"],
    ));
    assert_eq!(mine.lines().count(), 2, "{mine}");
    let seats = stdout(&andon(dir.path(), &["--ledger", "ledger.jsonl", "seats"]));
    assert!(seats.starts_with("seat "), "{seats}");
    assert!(seats.contains("Shared User: 1 claim"), "{seats}");
}
