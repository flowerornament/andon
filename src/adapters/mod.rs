//! Adapters: each reads one source and emits events. All of them are
//! read-only; none writes to the source it reads.

pub mod bd;
pub mod external;
pub mod ferry;
pub mod git;
pub mod github;
pub mod hail;
pub mod jj;
pub mod presence;
pub mod sessions;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::event::Event;
use crate::event::attr;
use crate::ledger::Ledger;
use crate::seat::Resolver;

/// The core adapters, by the name each one's coverage carries
/// (`attrs.adapter`). `tests/contract.rs` holds a corrupt-input case for
/// each, and for `external`.
pub const CORE: &[&str] = &[
    "jj", "git", "bd", "github", "ferry", "hail", "sessions", "presence",
];

/// Run every configured adapter, resolve seats, and build the ledger.
///
/// # Errors
/// When a configured source cannot be read; the error names the adapter.
pub fn collect(config: &Config) -> Result<Ledger> {
    let events = run_adapters(config, true, Some(&sessions::Scope::default()))?;
    Ok(resolve(config, events))
}

/// The ledger one seat's hook needs, read cheaply: every core source but the
/// messenger (a row shows no messages), and only this seat's own sessions,
/// from its earliest held claim (or the board's window) on.
///
/// # Errors
/// As [`collect`].
pub fn collect_for_seat(
    config: &Config,
    seat: &str,
    root: &std::path::Path,
    now: jiff::Timestamp,
) -> Result<Ledger> {
    // the seat's sessions from the configured `since`, read alongside the
    // rest (the fold counts only spans after each claim): no read waits on
    // another
    let scope = sessions::Scope {
        since: None,
        root: Some(root.to_string_lossy().into_owned()),
        seat: Some(seat.to_owned()),
    };
    let _ = now;
    let events = run_adapters(config, false, Some(&scope))?;
    Ok(resolve(config, events))
}

fn run_adapters(
    config: &Config,
    messages: bool,
    sessions: Option<&sessions::Scope>,
) -> Result<Vec<Event>> {
    type Job<'a> = (
        String,
        Box<dyn Fn() -> Result<Vec<Event>> + Send + Sync + 'a>,
    );
    let a = &config.adapters;
    let mut jobs: Vec<Job<'_>> = Vec::new();
    if let Some(c) = &a.jj {
        jobs.push(("jj".into(), Box::new(move || jj::collect(config, c))));
    }
    if let Some(c) = &a.git {
        jobs.push(("git".into(), Box::new(move || git::collect(config, c))));
    }
    if let Some(c) = &a.bd {
        jobs.push(("bd".into(), Box::new(move || bd::collect(config, c))));
    }
    if let Some(c) = &a.github {
        jobs.push((
            "github".into(),
            Box::new(move || github::collect(config, c)),
        ));
    }
    if let Some(c) = &a.ferry {
        jobs.push(("ferry".into(), Box::new(move || ferry::collect(config, c))));
    }
    if let Some(c) = a.hail.as_ref().filter(|_| messages) {
        jobs.push(("hail".into(), Box::new(move || hail::collect(config, c))));
    }
    if let (Some(c), Some(scope)) = (a.sessions.as_ref(), sessions) {
        jobs.push((
            "sessions".into(),
            Box::new(move || sessions::collect_scoped(config, c, scope)),
        ));
    }
    // the hook prints its own seat, which is running by definition
    if let Some(c) = a.presence.as_ref().filter(|_| messages) {
        jobs.push((
            "presence".into(),
            Box::new(move || presence::collect(config, c)),
        ));
    }
    for c in &a.external {
        jobs.push((
            format!("external {}", c.name),
            Box::new(move || external::collect(config, c)),
        ));
    }
    debug_assert!(
        jobs.iter()
            .all(|(name, _)| CORE.contains(&name.as_str()) || name.starts_with("external ")),
        "an adapter missing from CORE has no contract case"
    );
    // adapters read independent sources: run them at once; the ledger's sort
    // makes the result independent of which finishes first
    let trace = std::env::var_os("ANDON_TRACE").is_some();
    let results: Vec<(String, Result<Vec<Event>>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|(name, job)| {
                scope.spawn(move || {
                    let started = std::time::Instant::now();
                    let got = job();
                    if trace {
                        let n = got.as_ref().map_or(0, Vec::len);
                        eprintln!(
                            "andon: adapter {name}: {n} events in {:?}",
                            started.elapsed()
                        );
                    }
                    got
                })
            })
            .collect();
        jobs.iter()
            .zip(handles)
            .map(|((name, _), h)| {
                let got = h
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("the adapter stopped unexpectedly")));
                (name.clone(), got)
            })
            .collect()
    });
    let mut events: Vec<Event> = Vec::new();
    for (name, got) in results {
        let mut got = got
            .with_context(|| format!("adapter {name} (see its [adapters] table in andon.toml)"))?;
        // every coverage fact names the adapter that read it; an external
        // adapter's events already carry its own name
        for e in got.iter_mut().filter(|e| {
            e.kind == crate::event::Kind::Coverage && e.attr_str(attr::ADAPTER).is_none()
        }) {
            e.set_attr(attr::ADAPTER, name.clone());
        }
        events.extend(got);
    }
    Ok(events)
}

/// Resolve seats on raw events and build the ledger.
#[must_use]
pub fn resolve(config: &Config, mut events: Vec<Event>) -> Ledger {
    if config.project.known_goals_only {
        known_goals(&mut events);
    }
    Resolver::new(config).resolve(&mut events);
    Ledger::new(events)
}

/// With a tracker in the ledger, a goal id it does not have is no goal: drop
/// it from every event's subject and goal list. Without one, nothing to check.
fn known_goals(events: &mut [Event]) {
    use std::collections::HashSet;
    let known: HashSet<String> = events
        .iter()
        .filter(|e| e.kind == crate::event::Kind::Create)
        .filter_map(|e| e.subject.clone())
        .collect();
    if known.is_empty() {
        return;
    }
    for e in events.iter_mut() {
        if e.kind == crate::event::Kind::Create {
            continue;
        }
        let goals: Vec<String> = e
            .attr_strs(attr::GOALS)
            .into_iter()
            .filter(|g| known.contains(*g))
            .map(str::to_owned)
            .collect();
        if e.has_attr(attr::GOALS) {
            e.remove_attr(attr::GOALS);
            e.set_attr(attr::GOALS, goals.clone());
        }
        if e.subject.as_ref().is_some_and(|s| !known.contains(s)) {
            e.subject = goals.first().cloned();
            e.ids.goal.clone_from(&e.subject);
        }
    }
}

/// Run a read-only tool and return its stdout. A missing tool or a failure
/// becomes an error that names the fix.
///
/// # Errors
/// When the tool is not on PATH or exits non-zero.
pub fn run_tool(tool: &str, args: &[&str], fix: &str) -> Result<String> {
    run_tool_in(std::path::Path::new("."), tool, args, fix)
}

/// [`run_tool`] in a working directory.
///
/// # Errors
/// When the tool is not on PATH or exits non-zero.
pub fn run_tool_in(dir: &std::path::Path, tool: &str, args: &[&str], fix: &str) -> Result<String> {
    let mut command = command(tool, dir);
    command.args(args);
    let output = command
        .output()
        .map_err(|e| anyhow::anyhow!("cannot run {tool} ({e}); {fix}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no message");
        let first = first.trim_start_matches("Error: ");
        anyhow::bail!("{tool} failed: {first}");
    }
    String::from_utf8(output.stdout).with_context(|| format!("{tool} printed non-UTF-8"))
}

/// The one way andon starts a process: a read-only tool, in `dir`, with no
/// stdin. Every adapter spawns through this (clippy.toml holds the line).
#[must_use]
#[allow(
    clippy::disallowed_methods,
    reason = "the subprocess wrapper: the one place a process is built"
)]
pub fn command(tool: &str, dir: &std::path::Path) -> std::process::Command {
    let mut command = std::process::Command::new(tool);
    command.current_dir(dir).stdin(std::process::Stdio::null());
    if tool == "git" {
        // a read answers for the path it names: an inherited GIT_DIR (or
        // work tree, index, object store) must not redirect it, and a read
        // takes no optional lock
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_NAMESPACE",
        ] {
            command.env_remove(var);
        }
        command.env("GIT_OPTIONAL_LOCKS", "0");
    }
    command
}

/// Every directory an adapter lists goes through one of these, so a
/// directory it could not list is recorded as a gap, never skipped
/// (`.design/2026-10-14-coverage.md`: coverage is what the adapter read).
#[derive(Debug, Default)]
pub struct Reads {
    /// Paths that exist but could not be listed or read, with the reason.
    pub unreadable: Vec<(std::path::PathBuf, String)>,
}

impl Reads {
    /// The entries of `dir`, sorted. A missing directory has none; one that
    /// exists but cannot be listed is recorded.
    pub fn list(&mut self, dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        match std::fs::read_dir(dir) {
            Ok(entries) => {
                let mut out = Vec::new();
                for entry in entries {
                    match entry {
                        Ok(e) => out.push(e.path()),
                        Err(e) => self.unreadable.push((dir.to_path_buf(), e.to_string())),
                    }
                }
                out.sort();
                out
            }
            // absent, or a file a wildcard matched: nothing there to read
            Err(e) if absent(&e) => Vec::new(),
            Err(e) => {
                self.unreadable.push((dir.to_path_buf(), e.to_string()));
                Vec::new()
            }
        }
    }

    /// Expand `*` in any path component (`~/.claude/projects/-Users-me-code-x*`).
    /// Matches are sorted; a path without `*` is returned if it exists.
    pub fn glob(&mut self, pattern: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut current = vec![std::path::PathBuf::new()];
        for component in pattern.components() {
            let part = component.as_os_str().to_string_lossy();
            let mut next = Vec::new();
            for base in &current {
                if part.contains('*') {
                    next.extend(self.list(base).into_iter().filter(|p| {
                        p.file_name()
                            .is_some_and(|n| crate::config::wildcard(&part, &n.to_string_lossy()))
                    }));
                } else {
                    next.push(base.join(component.as_os_str()));
                }
            }
            current = next;
        }
        current
            .into_iter()
            .filter(|p| match p.try_exists() {
                Ok(exists) => exists,
                Err(e) if absent(&e) => false,
                Err(e) => {
                    self.unreadable.push((p.clone(), e.to_string()));
                    false
                }
            })
            .collect()
    }

    /// A gap for each path recorded since `mark` (an earlier length of
    /// `unreadable`), covering `kinds` for `seat` (`None`: every seat).
    #[must_use]
    pub fn gaps_since(
        &self,
        mark: usize,
        at: jiff::Timestamp,
        kinds: &[&str],
        seat: Option<&str>,
    ) -> Vec<Event> {
        self.unreadable[mark.min(self.unreadable.len())..]
            .iter()
            .map(|(path, why)| {
                crate::event::gap(
                    at,
                    kinds,
                    path.display().to_string(),
                    seat,
                    format!("{}: {why}", path.display()),
                )
            })
            .collect()
    }
}

/// An error that means the path is not there (a component is missing, or
/// is a file), as against there and unreadable.
fn absent(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// [`Reads::glob`] for a caller that refuses any unreadable path: the first
/// one is the error.
///
/// # Errors
/// When a path the pattern names exists but cannot be listed.
pub fn glob_all(pattern: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    let mut reads = Reads::default();
    let found = reads.glob(pattern);
    if let Some((path, why)) = reads.unreadable.first() {
        anyhow::bail!("cannot list {}: {why}", path.display());
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Kind;
    use crate::time::parse_instant;

    #[test]
    fn a_goal_id_the_tracker_lacks_is_no_goal() {
        let t = parse_instant("2026-10-13T00:00:00Z").unwrap_or_else(|e| panic!("{e}"));
        let mut create = Event::new(t, Kind::Create, "bd");
        create.subject = Some("x-pfgt".into());
        let mut message = Event::new(t, Kind::Message, "hail");
        message.subject = Some("x-review".into());
        message.set_attr(
            attr::GOALS,
            vec!["x-review".to_owned(), "x-pfgt".to_owned()],
        );
        let ledger = resolve(&Config::default(), vec![create, message]);
        let m = ledger
            .events()
            .iter()
            .find(|e| e.kind == Kind::Message)
            .unwrap_or_else(|| panic!("the message"));
        assert_eq!(m.subject.as_deref(), Some("x-pfgt"));
        assert_eq!(m.attr_strs(attr::GOALS), vec!["x-pfgt"]);
    }
}
