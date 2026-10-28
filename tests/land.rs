//! Milestone 2 (andon-8ig): the project's land verb refuses an uncited
//! commit. Runs the real `scripts/jj-land.sh` on a scratch jj repository with
//! andon as its check, and asserts the refusal, then the landing once the
//! commit cites a goal its seat holds.
//!
//! Needs jj and the script; where either is missing (the Nix build sandbox)
//! it says so on stderr and passes, because the gate that matters runs it.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the repositories and files andon reads"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/jj-land.sh")
}

fn have_jj() -> bool {
    Command::new("jj")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

struct Repo {
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        // an isolated jj config: the user's (trailer templates, aliases) must not leak in
        let config = dir.path().join("jj-config.toml");
        std::fs::write(
            &config,
            "[user]\nname = \"Test\"\nemail = \"test@example.com\"\n[ui]\npaginate = \"never\"\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let repo = Self { dir, config };
        repo.jj(&["git", "init", "repo"], repo.dir.path());
        repo
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn jj(&self, args: &[&str], cwd: &Path) -> Output {
        let out = Command::new("jj")
            .args(args)
            .current_dir(cwd)
            .env("JJ_CONFIG", &self.config)
            .output()
            .unwrap_or_else(|e| panic!("jj did not run: {e}"));
        assert!(
            out.status.success(),
            "jj {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.root().join(name), text).unwrap_or_else(|e| panic!("{e}"));
    }

    fn land(&self, ledger: &Path) -> Output {
        let check = format!(
            "{} --ledger {} check commit",
            env!("CARGO_BIN_EXE_andon"),
            ledger.display()
        );
        Command::new("bash")
            .arg(script())
            .current_dir(self.root())
            .env("JJ_CONFIG", &self.config)
            .env("LAND_GATE", "true")
            .env("LAND_CHECK", check)
            .output()
            .unwrap_or_else(|e| panic!("the land script did not run: {e}"))
    }
}

const CONFIG: &str = r#"
[project]
goal_pattern = '\bdemo-[a-z0-9]{3}\b'
[goal]
[seats.demo-1a]
[adapters.jj]
repo = "."
trunk = "main"
"#;

/// demo-abc created and claimed by demo-1a, by a tracker that covers it all.
const LEDGER: &str = concat!(
    r#"{"t":"1970-01-01T00:00:00Z","kind":"coverage","outcome":"pass","source":"test","attrs":{"covers":["create","claim","close"]}}"#,
    "\n",
    r#"{"t":"2026-10-13T09:00:00Z","kind":"create","subject":"demo-abc","ids":{"goal":"demo-abc"},"source":"test"}"#,
    "\n",
    r#"{"t":"2026-10-13T09:01:00Z","actor":"demo-1a","seat":"demo-1a","kind":"claim","subject":"demo-abc","ids":{"goal":"demo-abc"},"source":"test"}"#,
    "\n"
);

#[test]
fn land_refuses_an_uncited_commit_and_lands_a_cited_one() {
    if !have_jj() || !script().is_file() {
        eprintln!("skipped: jj or scripts/jj-land.sh is not available here");
        return;
    }
    let repo = Repo::new();
    let root = repo.root();
    repo.write("andon.toml", CONFIG);
    repo.jj(
        &[
            "describe",
            "-m",
            "init (demo-abc)\n\nJj-Workspace: demo-1a\n",
        ],
        &root,
    );
    repo.jj(&["bookmark", "create", "main", "-r", "@"], &root);
    repo.jj(&["new"], &root);
    let ledger = repo.dir.path().join("ledger.jsonl");
    std::fs::write(&ledger, LEDGER).unwrap_or_else(|e| panic!("{e}"));

    // an uncited change: the land verb refuses it, naming the fix
    repo.write("feature.txt", "x\n");
    repo.jj(
        &[
            "describe",
            "-m",
            "feature: thing\n\nJj-Workspace: demo-1a\n",
        ],
        &root,
    );
    let refused = repo.land(&ledger);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("doesn't reference a goal; add (demo-abc) to its subject"),
        "{stderr}"
    );
    let main_before = repo.jj(
        &["log", "--no-graph", "-r", "main", "-T", "commit_id"],
        &root,
    );

    // the same change, citing the goal its seat holds: it lands
    repo.jj(
        &[
            "describe",
            "-m",
            "feature: thing (demo-abc)\n\nJj-Workspace: demo-1a\n",
        ],
        &root,
    );
    let landed = repo.land(&ledger);
    let stdout = String::from_utf8_lossy(&landed.stdout);
    assert!(
        landed.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&landed.stderr)
    );
    assert!(stdout.contains("land: ok: "), "{stdout}");
    assert!(
        stdout.contains("references demo-abc, claimed by demo-1a"),
        "{stdout}"
    );
    let main_after = repo.jj(
        &["log", "--no-graph", "-r", "main", "-T", "commit_id"],
        &root,
    );
    assert_ne!(main_before.stdout, main_after.stdout, "main moved");
}

#[test]
fn land_refuses_a_goal_another_seat_holds() {
    if !have_jj() || !script().is_file() {
        eprintln!("skipped: jj or scripts/jj-land.sh is not available here");
        return;
    }
    let repo = Repo::new();
    let root = repo.root();
    repo.write("andon.toml", &format!("{CONFIG}[seats.demo-1b]\n"));
    repo.jj(
        &[
            "describe",
            "-m",
            "init (demo-abc)\n\nJj-Workspace: demo-1a\n",
        ],
        &root,
    );
    repo.jj(&["bookmark", "create", "main", "-r", "@"], &root);
    repo.jj(&["new"], &root);
    let ledger = repo.dir.path().join("ledger.jsonl");
    std::fs::write(&ledger, LEDGER).unwrap_or_else(|e| panic!("{e}"));
    repo.write("feature.txt", "x\n");
    repo.jj(
        &[
            "describe",
            "-m",
            "feature: thing (demo-abc)\n\nJj-Workspace: demo-1b\n",
        ],
        &root,
    );
    let refused = repo.land(&ledger);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("demo-abc is claimed by demo-1a, not demo-1b"),
        "{stderr}"
    );
}

#[test]
fn jj_reads_survive_divergent_operations_without_writing() {
    if !have_jj() {
        eprintln!("skipped: jj is not available here");
        return;
    }
    let repo = Repo::new();
    let root = repo.root();
    repo.write("andon.toml", CONFIG);
    repo.jj(
        &[
            "describe",
            "-m",
            "init (demo-abc)\n\nJj-Workspace: demo-1a\n",
        ],
        &root,
    );
    repo.jj(&["bookmark", "create", "main", "-r", "@"], &root);
    repo.jj(&["new"], &root);
    // two writers from one operation: jj now holds two operation heads
    repo.jj(&["describe", "-m", "one"], &root);
    repo.jj(&["--at-op", "@-", "describe", "-m", "two"], &root);
    let heads =
        || std::fs::read_dir(root.join(".jj/repo/op_heads/heads")).map_or(0, Iterator::count);
    assert_eq!(heads(), 2);
    let out = Command::new(env!("CARGO_BIN_EXE_andon"))
        .args(["ledger"])
        .current_dir(&root)
        .env("JJ_CONFIG", &repo.config)
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("\"kind\":\"land\""), "{stdout}");
    // andon read without merging them: still two heads
    assert_eq!(heads(), 2);
}
