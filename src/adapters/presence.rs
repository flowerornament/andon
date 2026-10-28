//! presence: which harness sessions are running now, read from the file a
//! harness keeps for each running session. Claude Code writes
//! `sessions/<pid>.json` under its config directory: the session id, its
//! working directory, `status` (`busy` or `idle`) and when the status last
//! changed (`statusUpdatedAt`).
//!
//! Emits one `presence` span per live session: from its status's change to
//! the capture instant, with the status. A file whose process is not running,
//! or is running but started at another time (`procStart`, in UTC), belongs to a
//! session that ended, and yields nothing. The files are current state, so
//! the read covers `state:presence` at the capture instant only: `asof` an
//! earlier instant cannot say who was running.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use jiff::Timestamp;
use serde_json::Value;

use crate::adapters::{Reads, command};
use crate::config::{Config, PresenceConfig};
use crate::event::attr;
use crate::event::{Event, Kind};

/// The kind the read covers, as current state.
const COVERS: &[&str] = &["state:presence"];

/// Read every configured sessions directory.
///
/// # Errors
/// When `ps` cannot run.
pub fn collect(config: &Config, pc: &PresenceConfig) -> Result<Vec<Event>> {
    let now = config.capture()?;
    let mut reads = Reads::default();
    let mut files = Vec::new();
    for pattern in &pc.claude {
        for dir in reads.glob(&config.path(pattern)) {
            files.extend(
                reads
                    .list(&dir)
                    .into_iter()
                    .filter(|p| p.extension().is_some_and(|x| x == "json")),
            );
        }
    }
    let running = processes(&ps()?);
    let mut events = Vec::new();
    for file in &files {
        let source = file.display().to_string();
        let read = std::fs::read_to_string(file)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()))
            .and_then(|v| session(&v, &running, now, &source));
        match read {
            Ok(Some(e)) => events.push(e),
            Ok(None) => {}
            // a file removed between the listing and the read: its session ended
            Err(_) if !file.exists() => {}
            Err(why) => events.push(crate::event::gap_at(
                now,
                COVERS,
                source,
                None,
                format!("{}: {why}", file.display()),
            )),
        }
    }
    for (path, why) in reads.unreadable {
        events.push(crate::event::gap_at(
            now,
            COVERS,
            path.display().to_string(),
            None,
            format!("{}: {why}", path.display()),
        ));
    }
    let source = pc.claude.join(", ");
    events.push(crate::event::coverage(now, Some(now), COVERS, source, None));
    Ok(events)
}

/// The process table: pid and start time. In UTC, as the harness records
/// `procStart`; `ps` would otherwise print local time.
fn ps() -> Result<String> {
    let fix = "install ps, or remove [adapters.presence]";
    let output = command("ps", Path::new("."))
        .env("TZ", "UTC")
        .args(["-A", "-o", "pid=", "-o", "lstart="])
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run ps ({e}); {fix}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "ps failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Running processes, by pid, to their start time as `ps` prints it, with
/// runs of spaces collapsed (`ps` pads the day of the month).
#[must_use]
pub fn processes(ps: &str) -> BTreeMap<u64, String> {
    ps.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?.parse().ok()?;
            Some((pid, words.collect::<Vec<_>>().join(" ")))
        })
        .collect()
}

/// The presence one session file records, or `None` when its session has
/// ended. Pure; tested without a process table.
///
/// # Errors
/// When the file is not a session record: no pid or working directory.
pub fn session(
    v: &Value,
    running: &BTreeMap<u64, String>,
    now: Timestamp,
    source: &str,
) -> Result<Option<Event>, String> {
    let pid = v["pid"].as_u64().ok_or("no pid")?;
    let cwd = v["cwd"].as_str().ok_or("no cwd")?;
    let Some(started) = running.get(&pid) else {
        return Ok(None);
    };
    // the pid is running, but a reused pid is another process
    if let Some(recorded) = v["procStart"].as_str()
        && recorded.split_whitespace().collect::<Vec<_>>().join(" ") != *started
    {
        return Ok(None);
    }
    let millis = |key: &str| {
        v[key]
            .as_i64()
            .and_then(|ms| Timestamp::from_millisecond(ms).ok())
    };
    // a status the harness does not record leaves only that it runs
    let (status, since) = match (v["status"].as_str(), millis("statusUpdatedAt")) {
        (Some(s), Some(t)) => (s.to_owned(), t),
        _ => (
            "live".to_owned(),
            millis("startedAt").ok_or("no start time")?,
        ),
    };
    let mut e = Event::new(since.min(now), Kind::Presence, source);
    e.t_end = Some(now);
    e.actor = Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    e.ids.session = v["sessionId"].as_str().map(str::to_owned);
    e.set_attr(attr::CWD, cwd.to_owned());
    e.set_attr(attr::HARNESS, "claude");
    e.set_attr(attr::STATUS, status);
    Ok(Some(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::parse_instant;

    const PS: &str = "    1 Mon Oct 12 10:10:24 2026\n53251 Mon Oct  19 04:29:52 2026\n";

    fn file(pid: u64, start: &str) -> Value {
        serde_json::json!({
            "pid": pid,
            "sessionId": "s1",
            "cwd": "/code/x-1a",
            "procStart": start,
            "startedAt": 1_792_384_193_762_i64,
            "status": "idle",
            "statusUpdatedAt": 1_792_475_018_039_i64,
        })
    }

    fn now() -> Timestamp {
        parse_instant("2026-10-20T06:00:00Z").unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_running_session_is_present_with_its_status_since_it_changed() {
        let running = processes(PS);
        let e = session(
            &file(53251, "Mon Oct 19 04:29:52 2026"),
            &running,
            now(),
            "f",
        )
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("present"));
        assert_eq!(e.kind, Kind::Presence);
        assert_eq!(e.actor.as_deref(), Some("x-1a"));
        assert_eq!(e.attr_str(attr::STATUS), Some("idle"));
        assert_eq!(
            e.t,
            Timestamp::from_millisecond(1_792_475_018_039).unwrap_or_else(|e| panic!("{e}"))
        );
        assert_eq!(e.t_end, Some(now()));
    }

    #[test]
    fn an_ended_or_reused_pid_is_no_session() {
        let running = processes(PS);
        let gone = session(&file(999, "Mon Oct 19 04:29:52 2026"), &running, now(), "f");
        assert_eq!(gone, Ok(None));
        // pid 1 runs, but it started at another time: the pid was reused
        let reused = session(&file(1, "Mon Oct 19 04:29:52 2026"), &running, now(), "f");
        assert_eq!(reused, Ok(None));
    }

    #[test]
    fn a_file_that_is_no_session_record_is_an_error() {
        let running = processes(PS);
        assert!(session(&serde_json::json!({"cwd": "/x"}), &running, now(), "f").is_err());
    }
}
