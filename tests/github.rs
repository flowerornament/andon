//! Milestone 3 (Generalize): a GitHub-native repository shows a board with
//! no code change in andon. The repository is a scratch git repository with a
//! merged pull request; GitHub's side (issues, reviews, Actions runs) is a
//! snapshot of the JSON `gh` prints, so the test needs no network. Everything
//! project-specific is in the config below.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the repositories and files andon reads"
)]

mod common;

use std::path::Path;
use std::process::Command;

use andon::adapters;
use andon::config::Config;
use andon::fold::board::board;
use andon::fold::scorecard::scorecard;
use andon::time::{Window, parse_instant};

fn git(dir: &Path, args: &[&str], when: &str) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=octo-bot",
            "-c",
            "user.email=octo@example.com",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", when)
        .env("GIT_COMMITTER_DATE", when)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap_or_else(|e| panic!("git did not run: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// main: init; branch bot-1/board: two commits; merged as pull request #7.
fn repository(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{e}"));
    git(dir, &["init", "-q"], "2026-10-12T00:00:00Z");
    std::fs::write(dir.join("README"), "demo\n").unwrap_or_else(|e| panic!("{e}"));
    git(dir, &["add", "README"], "2026-10-12T00:00:00Z");
    git(dir, &["commit", "-q", "-m", "init"], "2026-10-12T00:00:00Z");
    git(
        dir,
        &["checkout", "-q", "-b", "bot-1/board"],
        "2026-10-13T09:20:00Z",
    );
    std::fs::write(dir.join("board"), "1\n").unwrap_or_else(|e| panic!("{e}"));
    git(dir, &["add", "board"], "2026-10-13T09:20:00Z");
    git(
        dir,
        &["commit", "-q", "-m", "board: rows"],
        "2026-10-13T09:20:00Z",
    );
    std::fs::write(dir.join("board"), "2\n").unwrap_or_else(|e| panic!("{e}"));
    git(
        dir,
        &["commit", "-q", "-am", "board: goal column"],
        "2026-10-13T11:20:00Z",
    );
    git(dir, &["checkout", "-q", "main"], "2026-10-13T12:00:00Z");
    git(
        dir,
        &[
            "merge",
            "-q",
            "--no-ff",
            "bot-1/board",
            "-m",
            "Merge pull request #7 from octo/bot-1/board\n\nBoard rows",
        ],
        "2026-10-13T12:00:00Z",
    );
}

fn config(root: &Path) -> Config {
    let snapshot = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/github");
    let text = format!(
        r#"
[project]
name = "demo"
goal_pattern = '(?:[Cc]loses|[Ff]ixes|[Rr]esolves) (#\d+)'
goal_in = "message"

[goal]

[seats.bot-1]
aliases = ["octo-bot"]
[seats.bot-2]
aliases = ["helper-bot"]

[adapters.git]
repo = "repo"
trunk = "main"
branch_seats = {{ "bot-1/" = "bot-1", "bot-2/" = "bot-2" }}

[adapters.github]
repo = "octo/demo"
snapshot = "{}"
branch_seats = {{ "bot-1/" = "bot-1", "bot-2/" = "bot-2" }}
"#,
        snapshot.display()
    );
    common::config(&text, root)
}

#[test]
fn a_github_native_repository_shows_a_board() {
    if !have_git() {
        eprintln!("skipped: git is not available here");
        return;
    }
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    repository(&dir.path().join("repo"));
    let config = config(dir.path());
    let ledger = adapters::collect(&config).unwrap_or_else(|e| panic!("{e:#}"));
    let at = |s: &str| parse_instant(s).unwrap_or_else(|e| panic!("{e}"));

    // mid-morning: bot-1 holds #3, its change was returned, CI failed once
    let morning = board(&ledger, &config, at("2026-10-13T10:30:00Z"));
    let bot1 = &morning.seats[0];
    assert_eq!(bot1.seat, "bot-1");
    assert_eq!(
        bot1.goals
            .get()
            .and_then(|g| g.first())
            .map(|g| g.id.as_str()),
        Some("#3")
    );
    assert_eq!(
        bot1.goals
            .get()
            .and_then(|g| g.first())
            .and_then(|g| g.amount),
        Some(4.0)
    );
    assert_eq!(
        bot1.last_verdict
            .get()
            .and_then(Option::as_ref)
            .and_then(|v| v.outcome)
            .map(andon::event::Outcome::as_str),
        Some("return")
    );
    assert_eq!(
        (bot1.runs.get(), bot1.runs_pass.get()),
        (Some(&1), Some(&0))
    );
    assert_eq!(bot1.findings, 1);

    // after the merge: #3 closed, the landing is bot-1's, through its pull request
    let afternoon = board(&ledger, &config, at("2026-10-13T13:00:00Z"));
    let bot1 = &afternoon.seats[0];
    assert!(bot1.goals.get().is_some_and(Vec::is_empty), "#3 is closed");
    assert_eq!(bot1.landings, 1);
    assert_eq!(
        bot1.last_verdict
            .get()
            .and_then(Option::as_ref)
            .and_then(|v| v.outcome)
            .map(andon::event::Outcome::as_str),
        Some("go")
    );
    assert_eq!(
        (bot1.runs.get(), bot1.runs_pass.get()),
        (Some(&2), Some(&1))
    );
    let bot2 = &afternoon.seats[1];
    assert_eq!(bot2.seat, "bot-2");
    assert_eq!(
        bot2.goals
            .get()
            .and_then(|g| g.first())
            .map(|g| g.id.as_str()),
        Some("#4")
    );
    assert_eq!(bot2.landings, 0);
    assert_eq!(
        bot2.last_verdict
            .get()
            .and_then(Option::as_ref)
            .and_then(|v| v.outcome)
            .map(andon::event::Outcome::as_str),
        Some("return")
    );

    // the scorecard joins the landing to its reviews through the pull request
    let window =
        Window::parse("2026-10-13", at("2026-10-14T00:00:00Z")).unwrap_or_else(|e| panic!("{e}"));
    let card = scorecard(&ledger, &config, window);
    assert_eq!(card.total.landings, 1);
    assert_eq!(card.joins.landings_citing_verdict, 1);
    // change start 09:20 → landing 12:00; → first review 10:00
    assert_eq!(card.flow.lead_median_min, Some(160.0));
    assert_eq!(card.flow.review_median_min, Some(40.0));
    // CI keeps its logs, so no failure lost its reason
    assert_eq!(card.total.runs.map(|r| r.fail), 2);
    assert_eq!(card.total.runs.map(|r| r.fail_without_reason), 0);
}

#[test]
fn check_commit_reads_a_git_repository() {
    if !have_git() {
        eprintln!("skipped: git is not available here");
        return;
    }
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    repository(&dir.path().join("repo"));
    let config = config(dir.path());
    let gc = config.adapters.git.clone().unwrap_or_else(|| panic!("git"));
    let commit = adapters::git::describe(&config, &gc, "main").unwrap_or_else(|e| panic!("{e:#}"));
    assert!(commit.title.starts_with("Merge pull request #7"));
    let ledger = adapters::collect(&config).unwrap_or_else(|e| panic!("{e:#}"));
    let answer = andon::check::commit(
        &andon::commit::Commit {
            seat: Some("bot-1".into()),
            ..commit
        },
        &ledger,
        &config,
        parse_instant("2026-10-13T12:01:00Z").unwrap_or_else(|e| panic!("{e}")),
    );
    // the merge names no goal in its message: refused, naming the fix
    assert!(answer.refused());
    assert!(answer.line().contains("cites no goal"), "{}", answer.line());
}
