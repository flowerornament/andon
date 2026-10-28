//! runner: Ferry. Reads caller-side receipts (`<workspace>/.ferry/receipts/*.json`).
//!
//! A receipt names its commit, recipe, machine, outcome, exit code and, when
//! Ferry kept one, the failure's reason. It names no seat (its `seat` is
//! Ferry's own slot), so the actor is the workspace the receipt lives in.
//!
//! A failed run's reason is kept when the receipt names one or the run's
//! `output.log` survives beside it; the receipt itself records only the
//! output's size. Receipts are deleted with their workspace; a kept ledger is
//! how a project keeps them.

use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use crate::adapters::Reads;
use crate::config::{Config, FerryConfig};
use crate::event::attr;
use crate::event::{Event, Kind, Outcome};
use crate::time::from_millis;

/// Read every receipt under the configured directories.
///
/// # Errors
/// Never for a bad receipt (that is a gap in the ledger); kept for the
/// adapter interface.
pub fn collect(config: &Config, fc: &FerryConfig) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut duplicates = 0_usize;
    let now = config.capture()?;
    let mut reads = Reads::default();
    for pattern in &fc.receipts {
        // a directory the pattern passes through that cannot be listed: whose
        // receipts it held is unknown, so it is a gap for every seat
        let mark = reads.unreadable.len();
        let dirs = reads.glob(&config.path(pattern));
        events.extend(reads.gaps_since(mark, now, &["run"], None));
        for dir in dirs {
            let workspace = workspace_of(&config.path(pattern), &dir);
            let mark = reads.unreadable.len();
            let files: Vec<_> = reads
                .list(&dir)
                .into_iter()
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect();
            // a receipts directory that cannot be listed is its workspace's gap
            events.extend(reads.gaps_since(mark, now, &["run"], workspace.as_deref()));
            for file in files {
                // one run can be kept in several places (a receipt copied into
                // a sweep, or into two desks): the first path read is its record
                let id = file.file_stem().map(|s| s.to_string_lossy().into_owned());
                if !seen.insert(id.unwrap_or_default()) {
                    duplicates += 1;
                    continue;
                }
                // a receipt that cannot be read is a gap in the runs, said
                // so in the ledger, not a run that quietly did not happen
                let receipt = std::fs::read_to_string(&file)
                    .map_err(|e| e.to_string())
                    .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()));
                let receipt = match receipt {
                    Ok(r) => r,
                    Err(why) => {
                        events.push(crate::event::gap(
                            now,
                            &["run"],
                            file.display().to_string(),
                            workspace.as_deref(),
                            format!("{}: {why}", file.display()),
                        ));
                        continue;
                    }
                };
                let Some(mut e) = receipt_event(&receipt, workspace.as_deref(), &file)? else {
                    // JSON, but not a receipt: what run it records is unknown
                    events.push(crate::event::gap(
                        now,
                        &["run"],
                        file.display().to_string(),
                        workspace.as_deref(),
                        format!("{}: not a Ferry receipt (no started_ms)", file.display()),
                    ));
                    continue;
                };
                {
                    // a failure's output survives only in the run's directory;
                    // the receipt keeps its size, not its text
                    let id = e.ids.receipt.clone().unwrap_or_default();
                    if let Some(log) = output_log(&dir, &id) {
                        e.set_attr(attr::FAILURE_TAIL, log.display().to_string());
                    }
                    events.push(e);
                }
            }
        }
    }
    if duplicates > 0 && std::env::var_os("ANDON_TRACE").is_some() {
        eprintln!(
            "warning: ferry: {duplicates} receipts are kept in more than one place; each is read once"
        );
    }
    // runs are covered from the oldest receipt kept to now; before it, or
    // with no receipts at all, the ledger cannot say how many runs there were
    let oldest = events
        .iter()
        .filter(|e| e.kind == Kind::Run)
        .map(|e| e.t)
        .min();
    if let Some(oldest) = oldest {
        events.push(crate::event::coverage(
            oldest,
            Some(now),
            &["run"],
            fc.receipts.join(", "),
            None,
        ));
    }
    Ok(events)
}

/// Where Ferry keeps a local run's output: `local/v1/runs/<id>`,
/// `local/runs/<id>` or `runs/<id>` in the store that holds the receipts
/// directory, or one level up (a receipt filed one directory deeper).
fn output_log(receipts: &Path, id: &str) -> Option<std::path::PathBuf> {
    if id.is_empty() {
        return None;
    }
    let stores = receipts.ancestors().skip(1).take(2);
    for store in stores {
        for layout in [&["local", "v1", "runs"][..], &["local", "runs"], &["runs"]] {
            let mut path = store.to_path_buf();
            path.extend(layout);
            let log = path.join(id).join("output.log");
            if log.is_file() {
                return Some(log);
            }
        }
    }
    None
}

/// The workspace a receipt directory belongs to. When the configured
/// pattern has a wildcard, it is the path component the pattern's last
/// wildcard matched (`~/code/*/.ferry/receipts`, or a project's own layout
/// such as `<store>/*/receipts`); otherwise the directory holding `.ferry`.
fn workspace_of(pattern: &Path, receipts: &Path) -> Option<String> {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    let parts: Vec<_> = pattern.components().collect();
    let matched: Vec<_> = receipts.components().collect();
    if parts.len() == matched.len()
        && let Some(i) = parts
            .iter()
            .rposition(|c| c.as_os_str().to_string_lossy().contains('*'))
    {
        return Some(matched[i].as_os_str().to_string_lossy().into_owned());
    }
    receipts
        .ancestors()
        .find(|a| name(a).as_deref() == Some(".ferry"))
        .and_then(Path::parent)
        .and_then(name)
}

/// One receipt as a `run`. Pure; tested without Ferry.
///
/// # Errors
/// When its start time is out of range.
pub fn receipt_event(
    receipt: &Value,
    workspace: Option<&str>,
    file: &Path,
) -> Result<Option<Event>> {
    let Some(started) = receipt["started_ms"].as_i64() else {
        return Ok(None);
    };
    let mut e = Event::new(from_millis(started)?, Kind::Run, file.display().to_string());
    if let Some(elapsed) = receipt["elapsed_ms"].as_i64() {
        e.t_end = Some(from_millis(started + elapsed)?);
    }
    e.actor = workspace.map(str::to_owned);
    e.ids.commit = receipt["commit"].as_str().map(str::to_owned);
    e.ids.receipt = receipt["id"].as_str().map(str::to_owned);
    let ferry_outcome = receipt["outcome"].as_str().unwrap_or_default();
    let code = receipt["exit_code"].as_i64();
    e.outcome = Some(outcome(ferry_outcome, code));
    e.set_attr(attr::FERRY_OUTCOME, ferry_outcome.to_owned());
    if let Some(bytes) = receipt["output"]["bytes"].as_i64() {
        e.set_attr(attr::OUTPUT_BYTES, bytes);
    }
    for (attr, key) in [
        (attr::RECIPE, "recipe"),
        (attr::MACHINE, "machine"),
        (attr::RUN_REASON, "reason"),
    ] {
        e.set_attr(attr, receipt[key].as_str().unwrap_or_default().to_owned());
    }
    if let Some(code) = code {
        e.set_attr(attr::EXIT_CODE, code);
    }
    Ok(Some(e))
}

/// Ferry's outcome, or its exit-code convention when the outcome is absent:
/// 125 refused, 124 timed out, 128+n stopped by a signal, 255 lost.
#[must_use]
pub fn outcome(ferry: &str, code: Option<i64>) -> Outcome {
    match ferry {
        "ran-pass" => Outcome::Pass,
        "ran-fail" | "timeout" => Outcome::Fail,
        "refused" => Outcome::Refused,
        "stopped" => Outcome::Stopped,
        "lost" => Outcome::Unknown,
        _ => match code {
            Some(0) => Outcome::Pass,
            Some(125) => Outcome::Refused,
            Some(124) => Outcome::Fail,
            Some(255) | None => Outcome::Unknown,
            Some(c) if c > 128 => Outcome::Stopped,
            Some(_) => Outcome::Fail,
        },
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "fixtures write the receipts and logs they read"
)]
mod tests {
    use super::*;

    #[test]
    fn a_receipt_is_a_run_by_its_workspace() {
        let receipt = serde_json::json!({
            "id": "1791494227543-63", "commit": "3d24", "recipe": "smoke", "machine": "bench",
            "outcome": "ran-fail", "exit_code": 101, "reason": null,
            "started_ms": 1_791_494_227_544_i64, "elapsed_ms": 700
        });
        let e = receipt_event(&receipt, Some("andon-1b"), Path::new("r.json"))
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("a run"));
        assert_eq!(e.kind, Kind::Run);
        assert_eq!(e.outcome, Some(Outcome::Fail));
        assert_eq!(e.actor.as_deref(), Some("andon-1b"));
        assert_eq!(e.attr_str(attr::RUN_REASON), None);
        assert_eq!(e.attr_str(attr::RECIPE), Some("smoke"));
        assert!(e.t_end.is_some());
    }

    #[test]
    fn output_logs_are_found_in_each_of_ferrys_layouts() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let touch = |rel: &str| {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap_or(dir.path()))
                .unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(&p, "error: boom\n").unwrap_or_else(|e| panic!("{e}"));
        };
        touch("a/.ferry/local/v1/runs/r1/output.log");
        touch("b/.ferry/local/runs/r2/output.log");
        touch("c/.ferry/runs/r3/output.log");
        touch("d/.ferry/local/runs/r4/output.log");
        let found = |rel: &str, id: &str| output_log(&dir.path().join(rel), id).is_some();
        assert!(found("a/.ferry/receipts", "r1"));
        assert!(found("b/.ferry/receipts", "r2"));
        assert!(found("c/.ferry/receipts", "r3"));
        // a receipt filed one level deeper finds its store's runs
        assert!(found("d/.ferry/kept/desk", "r4"));
        assert!(!found("a/.ferry/receipts", "r2"));
    }

    #[test]
    fn exit_codes_stand_in_for_a_missing_outcome() {
        assert_eq!(outcome("", Some(125)), Outcome::Refused);
        assert_eq!(outcome("", Some(130)), Outcome::Stopped);
        assert_eq!(outcome("", Some(1)), Outcome::Fail);
        assert_eq!(outcome("", None), Outcome::Unknown);
    }

    #[test]
    fn the_workspace_is_the_receipt_directory_owner() {
        let ws = |pattern: &str, dir: &str| workspace_of(Path::new(pattern), Path::new(dir));
        assert_eq!(
            ws("/code/*/.ferry/receipts", "/code/x-1b/.ferry/receipts").as_deref(),
            Some("x-1b")
        );
        // a project's own layout: the last wildcard names the desk
        assert_eq!(
            ws(
                "/code/x*/.ferry/kept/*",
                "/code/x-4b/.ferry/kept/x-4b-review"
            )
            .as_deref(),
            Some("x-4b-review")
        );
        // no wildcard: the directory holding .ferry
        assert_eq!(
            ws("/code/x-1b/.ferry/receipts", "/code/x-1b/.ferry/receipts").as_deref(),
            Some("x-1b")
        );
    }
}
