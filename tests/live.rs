//! Acceptance against a real project, in two parts.
//!
//! The suites come from a machine-local file, never from this repository
//! (andon's code knows no project): `$ANDON_LIVE`, else
//! `~/.config/andon/live.toml`. Each suite names a project's own andon.toml, a
//! ledger captured from its stores once and kept (`andon ledger > FILE`), a
//! window, and the scorecard values that window must give:
//!
//! ```toml
//! [[suite]]
//! name = "a project's study window"
//! config = "~/path/to/the/project/andon.toml"
//! ledger = "~/.local/state/andon/acceptance/project-2026-10-19.jsonl"
//! window = "2026-10-11..2026-10-13T00:30:00Z"
//! [suite.expect]
//! "total.landings" = 117
//! "flow.lead_median_min" = 32.8      # numbers match to 0.05
//! ```
//!
//! - **Acceptance** folds the kept ledger and compares every value. The
//!   ledger never changes, so a value that moves is a change in andon's
//!   folds, never new data in the stores. It runs with `cargo test`
//!   wherever the suites file exists, and is skipped elsewhere (CI).
//! - **The live smoke test** reads each suite's project from its live
//!   stores and checks only that every configured adapter read and
//!   reported its coverage. It counts nothing, so stores that grow never
//!   fail it. It is ignored by `cargo test` and run on purpose with
//!   `just live`.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the repositories and files andon reads"
)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use andon::adapters;
use andon::config::{Config, expand};
use andon::event::{Kind, Outcome, attr};
use andon::fold::scorecard::scorecard;
use andon::ledger::Ledger;
use andon::time::Window;
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Suites {
    #[serde(default)]
    suite: Vec<Suite>,
}

#[derive(Debug, Deserialize)]
struct Suite {
    name: String,
    config: String,
    /// The kept ledger the values are pinned against.
    #[serde(default)]
    ledger: Option<String>,
    window: String,
    expect: toml::Table,
}

/// The suites, and the directory their paths resolve against; `None` on a
/// machine with none.
fn suites() -> Option<(Vec<Suite>, PathBuf)> {
    let file = match std::env::var("ANDON_LIVE") {
        Ok(v) if v == "off" => return None,
        Ok(v) => PathBuf::from(v),
        Err(_) => std::env::var_os("HOME")
            .map(|h| Path::new(&h).join(".config/andon/live.toml"))
            .filter(|p| p.is_file())?,
    };
    let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let suites: Suites =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let root = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    Some((suites.suite, root))
}

fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |v, key| v.get(key))
}

fn matches(want: &toml::Value, got: Option<&Value>) -> bool {
    match (want, got) {
        (toml::Value::Integer(w), Some(g)) => g.as_i64() == Some(*w),
        (toml::Value::Float(w), Some(g)) => g.as_f64().is_some_and(|g| (g - w).abs() < 0.05),
        // "unknown": the gauge must say it cannot know, with its reason
        (toml::Value::String(w), Some(g)) if w == "unknown" => g.get("unknown").is_some(),
        (toml::Value::String(w), Some(g)) => g.as_str() == Some(w.as_str()),
        _ => false,
    }
}

#[test]
fn kept_ledgers_reproduce_their_numbers() {
    let Some((suites, root)) = suites() else {
        eprintln!("skipped: no acceptance suites on this machine (~/.config/andon/live.toml)");
        return;
    };
    let mut failures = Vec::new();
    for suite in &suites {
        let Some(kept) = &suite.ledger else {
            failures.push(format!(
                "{}: names no kept ledger; capture one (andon --config {} ledger > FILE) and set ledger = FILE",
                suite.name, suite.config
            ));
            continue;
        };
        let (config, ledger) = (expand(&suite.config, &root), expand(kept, &root));
        if !config.is_file() || !ledger.is_file() {
            failures.push(format!(
                "{}: {} or {} is gone",
                suite.name,
                config.display(),
                ledger.display()
            ));
            continue;
        }
        let config = common::load(&config);
        let ledger = Ledger::open(&ledger).unwrap_or_else(|e| panic!("{}: {e:#}", suite.name));
        let window = Window::parse(&suite.window, Timestamp::MAX)
            .unwrap_or_else(|e| panic!("{}: {e}", suite.name));
        let card = serde_json::to_value(scorecard(&ledger, &config, window)).unwrap_or_default();
        for (key, want) in &suite.expect {
            let got = at(&card, key);
            if !matches(want, got) {
                failures.push(format!(
                    "{}: {key} is {}, expected {want}",
                    suite.name,
                    got.map_or_else(|| "absent".to_owned(), ToString::to_string)
                ));
            }
        }
        eprintln!(
            "acceptance: {}: {} values checked",
            suite.name,
            suite.expect.len()
        );
    }
    assert!(failures.is_empty(), "acceptance:\n{}", failures.join("\n"));
}

/// The adapters a config turns on, by the name their coverage carries.
fn configured(config: &Config) -> BTreeSet<String> {
    let a = &config.adapters;
    let core = [
        ("jj", a.jj.is_some()),
        ("git", a.git.is_some()),
        ("bd", a.bd.is_some()),
        ("github", a.github.is_some()),
        ("ferry", a.ferry.is_some()),
        ("hail", a.hail.is_some()),
        ("sessions", a.sessions.is_some()),
    ];
    core.into_iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| name.to_owned())
        .chain(a.external.iter().map(|e| e.name.clone()))
        .collect()
}

#[test]
#[ignore = "reads another project's live stores; run it with `just live`"]
fn live_adapters_read_and_report_coverage() {
    let Some((suites, root)) = suites() else {
        eprintln!("skipped: no acceptance suites on this machine (~/.config/andon/live.toml)");
        return;
    };
    let configs: BTreeSet<PathBuf> = suites.iter().map(|s| expand(&s.config, &root)).collect();
    let mut failures = Vec::new();
    for path in configs {
        let config = common::load(&path);
        let ledger = match adapters::collect(&config) {
            Ok(l) => l,
            Err(e) => {
                failures.push(format!("{}: {e:#}", path.display()));
                continue;
            }
        };
        let reported: BTreeSet<String> = ledger
            .events()
            .iter()
            .filter(|e| e.kind == Kind::Coverage && e.outcome == Some(Outcome::Pass))
            .filter_map(|e| e.attr_str(attr::ADAPTER).map(str::to_owned))
            .collect();
        for name in configured(&config).difference(&reported) {
            failures.push(format!(
                "{}: adapter {name} read nothing it could vouch for",
                path.display()
            ));
        }
        eprintln!(
            "live: {}: {} adapters read, {} events",
            path.display(),
            reported.len(),
            ledger.len()
        );
    }
    assert!(failures.is_empty(), "live:\n{}", failures.join("\n"));
}
