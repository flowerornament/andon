//! Overlapping unlanded edits, read from jj (`.design/2026-10-19-overlap-and-presence.md`
//! §1): two workspaces changing one file are flagged whatever tool wrote
//! it; a landing clears the landed side; a change undone is no change.

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

    fn jj(&self, cwd: &Path, args: &[&str]) {
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
    }

    /// Write `text` to `file` in workspace `ws`, by any means (here, not an
    /// edit tool), and let jj snapshot it.
    fn write(&self, ws: &str, file: &str, text: &str) {
        std::fs::write(self.path(ws).join(file), text).unwrap_or_else(|e| panic!("{e}"));
        self.jj(&self.path(ws), &["status"]);
    }

    fn flags(&self) -> Vec<Vec<String>> {
        let out: Output = Command::new(env!("CARGO_BIN_EXE_andon"))
            .args(["board", "--json"])
            .current_dir(self.dir.path())
            .env("JJ_CONFIG", &self.jj_config)
            .output()
            .unwrap_or_else(|e| panic!("{e}"));
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
        v["seats"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| {
                r["flags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f.as_str().map(str::to_owned))
                    .collect()
            })
            .collect()
    }
}

#[test]
fn two_workspaces_changing_one_file_overlap_until_one_lands() {
    if !have_jj() {
        eprintln!("skipped: jj is not available here");
        return;
    }
    let s = Scratch::new();
    let main = s.path("demo-1a");
    std::fs::create_dir_all(&main).unwrap_or_else(|e| panic!("{e}"));
    s.jj(&main, &["git", "init"]);
    std::fs::write(main.join("a.txt"), "a\n").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(main.join("b.txt"), "b\n").unwrap_or_else(|e| panic!("{e}"));
    s.jj(&main, &["commit", "-m", "base"]);
    s.jj(&main, &["bookmark", "set", "main", "-r", "@-"]);
    s.jj(
        &main,
        &["workspace", "add", "--name", "demo-1b", "../demo-1b"],
    );
    std::fs::write(
        s.path("andon.toml"),
        "[seats.demo-1a]\naliases = [\"default\"]\n[seats.demo-1b]\n[adapters.jj]\nrepo = \"demo-1a\"\ntrunk = \"main\"\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.flags(), vec![Vec::<String>::new(), Vec::new()]);

    s.write("demo-1a", "a.txt", "a from 1a\n");
    s.write("demo-1b", "a.txt", "a from 1b\n");
    // a change undone is no change: b.txt is back as it was
    s.write("demo-1b", "b.txt", "b changed\n");
    s.write("demo-1b", "b.txt", "b\n");
    assert_eq!(
        s.flags(),
        vec![
            vec!["overlaps demo-1b on 1 file (a.txt)".to_owned()],
            vec!["overlaps demo-1a on 1 file (a.txt)".to_owned()],
        ]
    );

    // demo-1a lands its change: it holds nothing unlanded, so nothing
    // overlaps, though demo-1b still holds a.txt
    s.jj(&main, &["commit", "-m", "a: from 1a"]);
    s.jj(&main, &["bookmark", "set", "main", "-r", "@-"]);
    assert_eq!(s.flags(), vec![Vec::<String>::new(), Vec::new()]);
}
