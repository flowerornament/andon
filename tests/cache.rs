//! The cache changes no output (andon-j4a): a read that resumes from what
//! it kept gives what a whole read gives, and deleting the cache is safe.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the repository and transcripts andon reads"
)]
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

fn run(dir: &Path, program: &str, args: &[&str], cache: &str) -> String {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("ANDON_CACHE", cache)
        .env("JJ_CONFIG", dir.join("jj-config.toml"))
        .output()
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    assert!(
        out.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The ledger without its capture instants, which differ run to run.
fn ledger(dir: &Path, cache: &str) -> Vec<String> {
    let text = run(dir, env!("CARGO_BIN_EXE_andon"), &["ledger"], cache);
    text.lines()
        .filter(|l| !l.contains("\"kind\":\"coverage\""))
        .map(|l| {
            let mut v: serde_json::Value =
                serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}"));
            if v["kind"] == "unlanded" {
                v["t_end"] = serde_json::Value::Null;
            }
            v.to_string()
        })
        .collect()
}

#[test]
fn a_kept_read_is_a_whole_read() {
    if !Command::new("jj")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("skipped: jj is not available here");
        return;
    }
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path();
    std::fs::write(
        root.join("jj-config.toml"),
        "[user]\nname = \"Test\"\nemail = \"test@example.com\"\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let repo = root.join("x-1a");
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
    let jj = |args: &[&str]| run(&repo, "jj", args, "off");
    jj(&["git", "init"]);
    std::fs::write(repo.join("a"), "a\n").unwrap_or_else(|e| panic!("{e}"));
    jj(&["commit", "-m", "a (x-abc)"]);
    jj(&["bookmark", "set", "main", "-r", "@-"]);
    let transcript = root.join("projects/x/s.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap_or(root)).unwrap_or_else(|e| panic!("{e}"));
    let call = |t: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"{t}","sessionId":"s1","cwd":"{}","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{}}}}]}}}}"#,
            repo.display()
        )
    };
    std::fs::write(&transcript, call("2026-10-13T10:00:00Z") + "\n")
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        root.join("andon.toml"),
        format!(
            "[project]\ngoal_pattern = '\\bx-[a-z]{{3}}\\b'\n[seats.x-1a]\n[adapters.jj]\nrepo = \"x-1a\"\ntrunk = \"main\"\n[adapters.sessions]\nclaude = [\"{}/projects/*\"]\n",
            root.display()
        ),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let cache = root.join("cache");
    let cache = cache.to_string_lossy();
    assert_eq!(ledger(root, &cache), ledger(root, "off"), "cold");
    // more operations, and the transcript grows: the warm read resumes
    std::fs::write(repo.join("b"), "b\n").unwrap_or_else(|e| panic!("{e}"));
    jj(&["commit", "-m", "b (x-abc)"]);
    jj(&["bookmark", "set", "main", "-r", "@-"]);
    let mut text = std::fs::read_to_string(&transcript).unwrap_or_default();
    text.push_str(&(call("2026-10-13T10:05:00Z") + "\n"));
    std::fs::write(&transcript, text).unwrap_or_else(|e| panic!("{e}"));
    let warm = ledger(root, &cache);
    assert_eq!(warm, ledger(root, "off"), "warm");
    assert!(
        warm.iter()
            .filter(|l| l.contains("\"kind\":\"land\""))
            .count()
            >= 2
    );
    // the cache deleted: the same again
    std::fs::remove_dir_all(root.join("cache")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(ledger(root, &cache), warm, "deleted");
}
