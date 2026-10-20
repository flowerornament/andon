//! The adapter contract: andon never claims a fact it could not read
//! (`.design/2026-10-14-coverage.md`). Every adapter is fed a corrupt or
//! unreadable input; the kinds it would have yielded must come back
//! uncovered (a gap, so every gauge reading them is `?`, never 0), or the
//! read must be refused, naming the adapter. A new adapter joins by adding
//! its name to `adapters::CORE` and a case here: a fixture, nothing else.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the files and tools andon reads"
)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use andon::adapters::CORE;
use andon::event::{ALL_HISTORY, attr};
use andon::fold::coverage::Coverage;
use andon::ledger::Ledger;

/// What the adapter must do with the corrupt input.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// Read the rest; these kinds are uncovered.
    Gap(&'static [&'static str]),
    /// Refuse the read: exit 2, naming the adapter.
    Refused,
}

struct Case {
    adapter: &'static str,
    input: &'static str,
    expect: Expect,
    /// Writes the fixture under the directory; returns andon.toml.
    build: fn(&Path) -> Option<String>,
}

fn write(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{e}"));
    }
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
}

/// A stand-in for a tool on PATH that prints `out` for any arguments.
fn tool(dir: &Path, name: &str, out: &str) {
    let path = dir.join("bin").join(name);
    write(
        &path,
        &format!("#!/bin/sh\ncat <<'ANDON_EOF'\n{out}\nANDON_EOF\n"),
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("{e}"));
}

/// Make `path` unreadable; `false` where permissions do not bind (root).
fn lock(path: &Path) -> bool {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::read(path).is_err()
}

fn has(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

const BD_VERDICTS: &str = "[adapters.bd]\ndir = \".\"\n[adapters.bd.verdicts]\nprefix = \"Verdict:\"\noutcome = \"verdict\"\n";

fn bd_issue(comment: &str) -> String {
    format!(
        r#"{{"id":"x-1","title":"t","status":"open","created_at":"2026-10-01T00:00:00Z","comments":[{comment}]}}"#
    )
}

const MSG: &str =
    "from: x-1b\nkind: ask\nid: 1013T100000-0001\ntime: 2026-10-13T10:00:00Z\nask: x\n\n";

const TURN: &str = r#"{"type":"assistant","timestamp":"2026-10-13T10:00:00Z","sessionId":"s","cwd":"/x-1a","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#;

const CASES: &[Case] = &[
    Case {
        adapter: "bd",
        input: "a verdict comment that is not JSON",
        expect: Expect::Gap(&["verdict"]),
        build: |dir| {
            let comment = r#"{"id":"c1","author":"x-1a","text":"Verdict: {\"verdict\": GO","created_at":"2026-10-02T00:00:00Z"}"#;
            tool(dir, "bd", &bd_issue(comment));
            Some(BD_VERDICTS.to_owned())
        },
    },
    Case {
        adapter: "bd",
        input: "a comment with no time",
        expect: Expect::Gap(&["other:comment"]),
        build: |dir| {
            tool(
                dir,
                "bd",
                &bd_issue(r#"{"id":"c1","author":"x-1a","text":"a finding"}"#),
            );
            Some("[adapters.bd]\ndir = \".\"\n".to_owned())
        },
    },
    Case {
        adapter: "github",
        input: "a snapshot that holds no prs.json",
        expect: Expect::Gap(&["verdict", "other:pr"]),
        build: |dir| {
            write(&dir.join("gh/issues.json"), "[]");
            Some("[adapters.github]\nrepo = \"o/r\"\nsnapshot = \"gh\"\nruns = false\n".to_owned())
        },
    },
    Case {
        adapter: "github",
        input: "an issue with no creation time",
        expect: Expect::Gap(&["create", "claim"]),
        build: |dir| {
            write(
                &dir.join("gh/issues.json"),
                r#"[{"number":1,"title":"t","state":"OPEN"}]"#,
            );
            write(&dir.join("gh/prs.json"), "[]");
            Some("[adapters.github]\nrepo = \"o/r\"\nsnapshot = \"gh\"\nruns = false\n".to_owned())
        },
    },
    Case {
        adapter: "ferry",
        input: "a receipt that is not JSON",
        expect: Expect::Gap(&["run"]),
        build: |dir| {
            write(&dir.join("x-1a/.ferry/receipts/r.json"), "{not json");
            Some(format!(
                "[adapters.ferry]\nreceipts = [\"{}/*/.ferry/receipts\"]\n",
                dir.display()
            ))
        },
    },
    Case {
        adapter: "hail",
        input: "an unreadable message in another project's seat",
        expect: Expect::Gap(&["message"]),
        build: |dir| {
            let file = dir.join("hail/seats/y-1a/cur/1013T100000-0001.read.md");
            write(&file, MSG);
            lock(&file)
                .then(|| "[adapters.hail]\nstate = \"hail\"\nlabels = [\"x-*\"]\n".to_owned())
        },
    },
    Case {
        adapter: "sessions",
        input: "a transcript line mid-file that is not JSON",
        expect: Expect::Gap(&["session_active", "edit"]),
        build: |dir| {
            write(
                &dir.join("projects/-x-1a/s.jsonl"),
                &format!(
                    "{TURN}\n{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\" truncated\n{TURN}\n"
                ),
            );
            Some(format!(
                "[seats.x-1a]\ncwd = [\"/x-1a\"]\n[adapters.sessions]\nclaude = [\"{}/projects/*\"]\n",
                dir.display()
            ))
        },
    },
    Case {
        adapter: "presence",
        input: "a session file that is not JSON",
        expect: Expect::Gap(&["state:presence"]),
        build: |dir| {
            if !has("ps") {
                return None;
            }
            write(&dir.join("sessions/1.json"), "{not json");
            Some("[adapters.presence]\nclaude = [\"sessions\"]\n".to_owned())
        },
    },
    Case {
        adapter: "jj",
        input: "a store whose operation heads cannot be listed",
        expect: Expect::Refused,
        build: |dir| {
            if !has("jj") {
                return None;
            }
            let repo = dir.join("repo");
            std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
            let ok = Command::new("jj")
                .args(["git", "init", "--quiet"])
                .current_dir(&repo)
                .status()
                .is_ok_and(|s| s.success());
            let heads = repo.join(".jj/repo/op_heads/heads");
            (ok && lock(&heads)).then(|| "[adapters.jj]\nrepo = \"repo\"\n".to_owned())
        },
    },
    Case {
        adapter: "git",
        input: "a path inside a repository, not its top level",
        expect: Expect::Refused,
        build: |dir| {
            if !has("git") {
                return None;
            }
            let ok = Command::new("git")
                .args(["init", "--quiet", "repo"])
                .current_dir(dir)
                .status()
                .is_ok_and(|s| s.success());
            std::fs::create_dir_all(dir.join("repo/sub")).unwrap_or_else(|e| panic!("{e}"));
            ok.then(|| "[adapters.git]\nrepo = \"repo/sub\"\n".to_owned())
        },
    },
    Case {
        adapter: "external",
        input: "a line that is not an event",
        expect: Expect::Refused,
        build: |dir| {
            write(&dir.join("x.jsonl"), "{\"kind\":\"land\"}\n");
            Some("[[adapters.external]]\nname = \"x-review\"\nfiles = [\"x.jsonl\"]\n".to_owned())
        },
    },
];

/// Run one case; `None` where its fixture cannot be built here.
fn run(case: &Case) -> Option<Result<(), String>> {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let toml = (case.build)(dir.path())?;
    Some(judge(case, dir.path(), &toml))
}

fn judge(case: &Case, dir: &Path, toml: &str) -> Result<(), String> {
    write(&dir.join("andon.toml"), toml);
    let path = format!(
        "{}:{}",
        dir.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_andon"))
        .args(["--config", "andon.toml", "ledger"])
        .current_dir(dir)
        .env("PATH", path)
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    // let tempdir clean up what the fixture locked
    let _ = Command::new("chmod")
        .args(["-R", "u+rwx"])
        .arg(dir)
        .status();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    match (case.expect, out.status.code()) {
        (Expect::Refused, Some(2)) if said.contains(&format!("adapter {}", case.adapter)) => Ok(()),
        (Expect::Refused, code) => Err(format!(
            "expected a refusal naming the adapter, got {code:?}: {said}"
        )),
        (Expect::Gap(kinds), Some(0)) => {
            let ledger = Ledger::read(out.stdout.as_slice()).map_err(|e| e.to_string())?;
            let coverage = Coverage::new(&ledger);
            // the capture instant: where every span the read vouches for ends
            let capture = ledger
                .events()
                .iter()
                .filter_map(|e| e.t_end)
                .max()
                .unwrap_or(ALL_HISTORY);
            let covered: Vec<&str> = kinds
                .iter()
                .copied()
                .filter(|k| coverage.check(&[k], None, ALL_HISTORY, capture).is_none())
                .collect();
            let named = ledger.events().iter().any(|e| {
                e.attr_str(attr::ADAPTER) == Some(case.adapter)
                    && e.attr_str(attr::GAP_REASON).is_some()
            });
            if !covered.is_empty() {
                Err(format!("claims {covered:?} covered"))
            } else if !named {
                Err("records no gap of its own".to_owned())
            } else {
                Ok(())
            }
        }
        (Expect::Gap(_), code) => Err(format!("expected a gap, got exit {code:?}: {said}")),
    }
}

#[test]
fn every_adapter_reports_what_it_could_not_read() {
    let mut failed = Vec::new();
    for case in CASES {
        match run(case) {
            Some(Ok(())) => {}
            Some(Err(why)) => failed.push(format!("{} ({}): {why}", case.adapter, case.input)),
            None => eprintln!("contract: {} ({}) skipped here", case.adapter, case.input),
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn every_adapter_has_a_case() {
    for name in CORE.iter().chain(&["external"]) {
        assert!(
            CASES.iter().any(|c| c.adapter == *name),
            "adapter {name} has no contract case; add a fixture to CASES"
        );
    }
}
