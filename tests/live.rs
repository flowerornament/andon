//! Live acceptance: andon's adapters against a real project's stores.
//!
//! The suites come from a machine-local file, never from this repository
//! (andon's code knows no project): `$ANDON_LIVE`, else
//! `~/.config/andon/live.toml`. Each suite names a project's own andon.toml, a
//! window, and the scorecard values that window must give:
//!
//! ```toml
//! [[suite]]
//! name = "a project's study window"
//! config = "~/path/to/the/project/andon.toml"
//! window = "2026-10-11..2026-10-13T00:30:00Z"
//! [suite.expect]
//! "total.landings" = 117
//! "flow.lead_median_min" = 32.8      # numbers match to 0.05
//! ```
//!
//! Paths are dotted keys into `andon scorecard --json`. A suite may also
//! declare `[suite.stores]`: what its window's stores held when it was
//! recorded (runs, sessions). Those are checked first; when they differ, the
//! stores changed (a desk swept, a receipt deleted) and the values are not
//! compared, so drift is never reported as an adapter regression.
//!
//! Another project's stores change under andon, so this is not part of the
//! land gate: it is ignored by `cargo test` and run on purpose with
//! `just live` (at acceptance time).

use std::path::{Path, PathBuf};

use andon::adapters;
use andon::config::{Config, expand};
use andon::fold::scorecard::scorecard;
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
    window: String,
    expect: toml::Table,
    #[serde(default)]
    stores: toml::Table,
}

fn suites_file() -> Option<PathBuf> {
    match std::env::var("ANDON_LIVE") {
        Ok(v) if v == "off" => None,
        Ok(v) => Some(PathBuf::from(v)),
        Err(_) => std::env::var_os("HOME")
            .map(|h| Path::new(&h).join(".config/andon/live.toml"))
            .filter(|p| p.is_file()),
    }
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
#[ignore = "reads another project's live stores; run it with `just live`"]
fn live_suites_reproduce_their_numbers() {
    let Some(file) = suites_file() else {
        eprintln!("skipped: no live acceptance suites on this machine (~/.config/andon/live.toml)");
        return;
    };
    let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let suites: Suites =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let root = file.parent().unwrap_or(Path::new("/"));
    let mut failures = Vec::new();
    // suites over one config share its ledger: read each store once
    let mut ledgers: std::collections::HashMap<PathBuf, (Config, andon::ledger::Ledger)> =
        std::collections::HashMap::new();
    for suite in &suites.suite {
        let path = expand(&suite.config, root);
        if !path.is_file() {
            failures.push(format!(
                "{}: its config {} is gone",
                suite.name,
                path.display()
            ));
            continue;
        }
        let (config, ledger) = ledgers.entry(path.clone()).or_insert_with(|| {
            let config = Config::load(&path).unwrap_or_else(|e| panic!("{}: {e:#}", suite.name));
            let ledger =
                adapters::collect(&config).unwrap_or_else(|e| panic!("{}: {e:#}", suite.name));
            (config, ledger)
        });
        let window = Window::parse(&suite.window, Timestamp::now())
            .unwrap_or_else(|e| panic!("{}: {e}", suite.name));
        let card = serde_json::to_value(scorecard(ledger, config, window)).unwrap_or_default();
        let drift: Vec<String> = suite
            .stores
            .iter()
            .filter(|(key, want)| !matches(want, at(&card, key)))
            .map(|(key, want)| {
                let got = at(&card, key).map_or_else(|| "absent".to_owned(), ToString::to_string);
                format!("{key} {want} → {got}")
            })
            .collect();
        if !drift.is_empty() {
            failures.push(format!(
                "{}: the stores changed since the suite was recorded, so its values were not compared ({}); re-record the suite",
                suite.name,
                drift.join(", ")
            ));
            continue;
        }
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
            "live: {}: {} values checked",
            suite.name,
            suite.expect.len()
        );
    }
    assert!(
        failures.is_empty(),
        "live acceptance:\n{}",
        failures.join("\n")
    );
}
