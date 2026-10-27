//! Progress and timings go to stderr, never to stdout (andon-j4a): stdout
//! is the API, and a slow read's line must not reach it.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the slow adapter andon reads"
)]
#![cfg(unix)]

use std::process::{Command, Output};

fn andon(dir: &std::path::Path, args: &[&str], progress: bool) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_andon"));
    c.args(args).current_dir(dir);
    if progress {
        c.env("ANDON_PROGRESS", "1");
    }
    c.output().unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn an_adapters_progress_never_reaches_stdout() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    // an adapter that takes a second, so the progress line shows
    std::fs::write(
        dir.path().join("andon.toml"),
        "[seats.x-1a]\n[[adapters.external]]\nname = \"x-slow\"\ncommand = [\"sh\", \"-c\", \"sleep 1; echo '{\\\"t\\\":\\\"2026-10-13T09:00:00Z\\\",\\\"kind\\\":\\\"land\\\",\\\"actor\\\":\\\"x-1a\\\",\\\"source\\\":\\\"x\\\"}'\"]\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let quiet = andon(dir.path(), &["ledger"], false);
    let shown = andon(dir.path(), &["ledger", "--timings"], true);
    assert!(
        quiet.status.success(),
        "{}",
        String::from_utf8_lossy(&quiet.stdout)
    );
    let err = String::from_utf8_lossy(&shown.stderr);
    assert!(err.contains("reading 0/1 · external x-slow"), "{err}");
    assert!(err.contains("external x-slow"), "{err}");
    assert!(err.contains("1 events"), "timings: {err}");
    // the same stdout, byte for byte, with the line and timings shown
    assert_eq!(
        String::from_utf8_lossy(&shown.stdout),
        String::from_utf8_lossy(&quiet.stdout)
    );
    assert!(!String::from_utf8_lossy(&shown.stdout).contains("reading"));
    // piped, with nothing asked for, stderr gets nothing
    assert_eq!(String::from_utf8_lossy(&quiet.stderr), "");
}
