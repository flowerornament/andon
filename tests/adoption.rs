//! The first adopter's report (andon-e10): what bit on first real use. Each
//! test failed before its fix.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the repositories and files andon reads"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn have_jj() -> bool {
    Command::new("jj")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

struct Scratch {
    dir: tempfile::TempDir,
    jj_config: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let jj_config = dir.path().join("jj-config.toml");
        std::fs::write(
            &jj_config,
            "[user]\nname = \"Test\"\nemail = \"test@example.com\"\n[ui]\npaginate = \"never\"\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        Self { dir, jj_config }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn jj(&self, cwd: &Path, args: &[&str]) -> Output {
        let out = Command::new("jj")
            .args(args)
            .current_dir(cwd)
            .env("JJ_CONFIG", &self.jj_config)
            .output()
            .unwrap_or_else(|e| panic!("jj did not run: {e}"));
        assert!(
            out.status.success(),
            "jj {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn andon(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_andon"))
            .args(args)
            .current_dir(cwd)
            .env("JJ_CONFIG", &self.jj_config)
            .output()
            .unwrap_or_else(|e| panic!("{e}"))
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// A tracker that covers everything, with demo-abc claimed by `seat`.
fn ledger(seat: &str) -> String {
    format!(
        concat!(
            r#"{{"t":"1970-01-01T00:00:00Z","kind":"coverage","outcome":"pass","source":"test","attrs":{{"covers":["create","claim","close"]}}}}"#,
            "\n",
            r#"{{"t":"2026-10-13T09:00:00Z","kind":"create","subject":"demo-abc","ids":{{"goal":"demo-abc"}},"source":"test"}}"#,
            "\n",
            r#"{{"t":"2026-10-13T09:01:00Z","actor":"{seat}","kind":"claim","subject":"demo-abc","ids":{{"goal":"demo-abc"}},"source":"test"}}"#,
            "\n"
        ),
        seat = seat
    )
}

const CONFIG: &str = r#"
[project]
goal_pattern = '\bdemo-[a-z0-9]{3}\b'
[seats.demo-1a]
[seats.demo-1b]
[adapters.jj]
repo = "."
trunk = "main"
"#;

/// Item 1: `check commit @` is the caller's `@`: from another workspace of the
/// configured repository, that workspace's change, not the anchor's.
#[test]
fn check_commit_resolves_at_in_the_callers_workspace() {
    if !have_jj() {
        eprintln!("skipped: jj is not available here");
        return;
    }
    let s = Scratch::new();
    let repo = s.path("demo-1a");
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
    s.jj(&repo, &["git", "init"]);
    std::fs::write(repo.join("andon.toml"), CONFIG).unwrap_or_else(|e| panic!("{e}"));
    s.jj(&repo, &["describe", "-m", "init (demo-abc)"]);
    s.jj(&repo, &["bookmark", "create", "main", "-r", "@"]);
    s.jj(&repo, &["new"]); // the anchor's @: empty, undescribed
    s.jj(
        &repo,
        &["workspace", "add", "--name", "demo-1b", "../demo-1b"],
    );
    let ws = s.path("demo-1b");
    s.jj(
        &ws,
        &[
            "describe",
            "-m",
            "feature: thing (demo-abc)\n\nJj-Workspace: demo-1b\n",
        ],
    );
    let kept = s.path("ledger.jsonl");
    std::fs::write(&kept, ledger("demo-1b")).unwrap_or_else(|e| panic!("{e}"));
    let config = repo.join("andon.toml");
    let args = [
        "--config",
        config.to_str().unwrap_or_default(),
        "--ledger",
        kept.to_str().unwrap_or_default(),
        "check",
        "commit",
        "@",
    ];
    let mine = s.jj(
        &ws,
        &["log", "--no-graph", "-r", "@", "-T", "commit_id.short(12)"],
    );
    let mine = String::from_utf8_lossy(&mine.stdout).into_owned();
    let out = s.andon(&ws, &args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout(&out).contains(&mine),
        "checked another change: {}",
        stdout(&out)
    );
}

/// A stand-in `bd` on PATH: `export` prints `issues`, `show <id> --json`
/// prints the one named.
#[cfg(unix)]
fn bd_shim(dir: &Path, issues: &[&str]) -> String {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e}"));
    let data = dir.join("issues.jsonl");
    std::fs::write(&data, format!("{}\n", issues.join("\n"))).unwrap_or_else(|e| panic!("{e}"));
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\n  export) cat '{d}' ;;\n  show) grep \"\\\"id\\\":\\\"$2\\\"\" '{d}' | sed 's/^/[/; s/$/]/' ;;\nesac\n",
        d = data.display()
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

#[cfg(unix)]
fn run(dir: &Path, path: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_andon"))
        .args(args)
        .current_dir(dir)
        .env("PATH", path)
        .output()
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Item 2: A goal's done-when kept in the tracker's acceptance field passes
/// `check goal` where config maps the section to that field.
#[cfg(unix)]
#[test]
fn a_section_can_come_from_a_tracker_field() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let issue = r###"{"id":"demo-abc","title":"t","description":"## Why\n\nbecause\n\n## Budget\n\n2 agent-hours\n","acceptance_criteria":"Done when: `cargo test` passes","status":"open","created_at":"2026-10-13T09:00:00Z"}"###;
    let path = bd_shim(dir.path(), &[issue]);
    let config = "[project]\ngoal_pattern = '\\bdemo-[a-z0-9]{3}\\b'\n[adapters.bd]\ndir = \".\"\n";
    std::fs::write(dir.path().join("andon.toml"), config).unwrap_or_else(|e| panic!("{e}"));
    let before = run(dir.path(), &path, &["check", "goal", "demo-abc"]);
    assert_eq!(
        before.status.code(),
        Some(1),
        "unmapped, the section is missing: {}",
        stdout(&before)
    );
    std::fs::write(
        dir.path().join("andon.toml"),
        format!("{config}[goal.fields]\n\"Done when\" = \"acceptance_criteria\"\n"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let after = run(dir.path(), &path, &["check", "goal", "demo-abc"]);
    assert_eq!(
        after.status.code(),
        Some(0),
        "{}{}",
        stdout(&after),
        String::from_utf8_lossy(&after.stderr)
    );
}
