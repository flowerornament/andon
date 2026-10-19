//! sessions: harness transcripts (Claude Code JSONL, Codex rollouts).
//!
//! A transcript is an artifact the harness writes as the agent works; andon
//! reads the facts in it (when tools were called, from which directory, which
//! subagents started) and nothing the agent asserted. Tool-call times become
//! activity spans: calls closer than `gap_minutes` belong to one span.
//!
//! Emits `session_active` spans (the actor is the working directory's name,
//! so a workspace is a seat) and `spawn` for each subagent started. A
//! subagent's spans carry its root session in `ids.session` and its own id in
//! `ids.critic_session`, so its effort counts for the seat that started it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Result;
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;

use crate::adapters::Reads;
use crate::config::{Config, SessionsConfig, wildcard};
use crate::event::attr;
use crate::event::{Event, Kind};
use crate::time::parse_instant;

/// What one transcript says.
#[derive(Debug, Default)]
pub struct Transcript {
    pub harness: &'static str,
    pub session: Option<String>,
    /// The root session, for a subagent.
    pub root: Option<String>,
    pub subagent: bool,
    pub cwd: BTreeMap<String, usize>,
    pub calls: Vec<Timestamp>,
    pub spawns: Vec<(Timestamp, String)>,
    pub source: String,
}

/// What to read: transcripts from `since` on, and with `root`, only those
/// whose working directory is under it (the hook reads one seat's).
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub since: Option<Timestamp>,
    pub root: Option<String>,
    /// The seat the scoped read is for: its coverage covers only that seat.
    pub seat: Option<String>,
}

/// Read every configured transcript.
///
/// # Errors
/// When a configured directory cannot be listed.
pub fn collect(config: &Config, sc: &SessionsConfig) -> Result<Vec<Event>> {
    collect_scoped(config, sc, &Scope::default())
}

/// Read the configured transcripts within a scope.
///
/// # Errors
/// When the configured `since` is not a time.
pub fn collect_scoped(config: &Config, sc: &SessionsConfig, scope: &Scope) -> Result<Vec<Event>> {
    let configured = sc.since.as_deref().map(parse_instant).transpose()?;
    let since = configured.max(scope.since);
    let configured = |cwd: &str| {
        sc.cwd.is_empty()
            || sc
                .cwd
                .iter()
                .any(|p| wildcard(&config.path(p).to_string_lossy(), cwd))
    };
    // the seat filter applies to the directory a session is attributed to
    // (where it mostly worked), never to where it happened to start, so a
    // scoped read keeps exactly the sessions the full read gives the seat
    let keep_cwd = |cwd: &str| {
        configured(cwd)
            && scope
                .root
                .as_deref()
                .is_none_or(|r| cwd == r || cwd.starts_with(&format!("{r}/")))
    };
    let mut files: Vec<(PathBuf, bool)> = Vec::new();
    let mut reads = Reads::default();
    for (patterns, is_codex) in [(&sc.claude, false), (&sc.codex, true)] {
        for pattern in patterns {
            for dir in reads.glob(&config.path(pattern)) {
                for file in jsonl_files(&mut reads, &dir) {
                    if fresh(&file, since) {
                        files.push((file, is_codex));
                    }
                }
            }
        }
    }
    // transcripts are independent: read them at once, in file order after
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let chunk = files.len().div_ceil(workers.max(1)).max(1);
    let root = scope.root.as_deref();
    let read: Vec<(PathBuf, Result<Option<Transcript>, String>)> = std::thread::scope(|scope_| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                let configured = &configured;
                scope_.spawn(move || {
                    part.iter()
                        .map(|(file, is_codex)| {
                            let got = if *is_codex {
                                codex(file, configured, root, since)
                            } else {
                                claude_kept(file, configured, since)
                            };
                            (file.clone(), got)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        // a reader that stopped loses its transcripts: that is an error,
        // never a quietly shorter ledger
        let mut all = Vec::new();
        for h in handles {
            all.extend(
                h.join()
                    .map_err(|_| anyhow::anyhow!("a transcript reader stopped unexpectedly"))?,
            );
        }
        Ok::<_, anyhow::Error>(all)
    })?;
    let now = config.capture()?;
    let gap = SignedDuration::from_mins(i64::from(sc.gap_minutes));
    // a directory that could not be listed held transcripts of unknown
    // seats: a gap for every seat, as an unreadable transcript is
    let mut events = reads.gaps_since(0, now, &["session_active", "spawn"], None);
    for (file, got) in read {
        let t = match got {
            Ok(Some(t)) => t,
            Ok(None) => continue,
            // an unreadable transcript is a gap: whose sessions it held is
            // unknown, so every seat's effort is unknown while it stands
            Err(why) => {
                let reason = format!("{}: {why}", file.display());
                let kinds = &["session_active", "spawn"];
                let source = file.display().to_string();
                // its turns were all written before its last write: the gap
                // stands over spans up to then, not over every window after
                events.push(match last_write(&file) {
                    Some(t) => crate::event::gap_over(
                        crate::event::ALL_HISTORY,
                        t,
                        kinds,
                        source,
                        None,
                        reason,
                    ),
                    None => crate::event::gap(now, kinds, source, None, reason),
                });
                continue;
            }
        };
        let Some(cwd) = most(&t.cwd) else { continue };
        if !keep_cwd(&cwd) {
            continue;
        }
        events.extend(spans(&t, &cwd, gap, since));
    }
    // sessions are covered from `since` (or all history) to now; a scoped
    // read covers only its seat
    events.push(crate::event::coverage(
        since.unwrap_or(crate::event::ALL_HISTORY),
        Some(now),
        &["session_active", "spawn"],
        "transcripts",
        scope.seat.as_deref(),
    ));
    Ok(events)
}

/// When a file was last written, if that can be read.
fn last_write(file: &Path) -> Option<Timestamp> {
    let modified = std::fs::metadata(file).and_then(|m| m.modified()).ok()?;
    Timestamp::try_from(modified).ok()
}

fn fresh(file: &Path, since: Option<Timestamp>) -> bool {
    let Some(since) = since else { return true };
    let modified = std::fs::metadata(file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| Timestamp::try_from(m).ok());
    modified.is_none_or(|m| m >= since)
}

/// `*.jsonl` files under a directory, recursively, sorted.
fn jsonl_files(reads: &mut Reads, dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for path in reads.list(&d) {
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The `"timestamp"` of a raw transcript line, without parsing the rest.
fn line_time(line: &[u8]) -> Option<Timestamp> {
    const KEY: &[u8] = b"\"timestamp\":\"";
    let at = memchr::memmem::find(line, KEY)? + KEY.len();
    let end = line[at..].iter().position(|b| *b == b'"')? + at;
    parse_instant(std::str::from_utf8(&line[at..end]).ok()?).ok()
}

/// Where in an append-only transcript the lines from `since` on begin:
/// a binary search on line times, for files too big to read from the top.
fn seek_since(file: &mut File, since: Timestamp) -> u64 {
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return 0;
    };
    let time_at = |file: &mut File, offset: u64| -> Option<Timestamp> {
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = vec![0_u8; 256 * 1024];
        let n = file.read(&mut buf).ok()?;
        let buf = &buf[..n];
        let start = if offset == 0 {
            0
        } else {
            buf.iter().position(|b| *b == b'\n')? + 1
        };
        buf[start..]
            .split(|b| *b == b'\n')
            .take(64)
            .find_map(line_time)
    };
    let (mut lo, mut hi) = (0_u64, len);
    while hi - lo > 1 << 20 {
        let mid = lo + (hi - lo) / 2;
        match time_at(file, mid) {
            Some(t) if t < since => lo = mid,
            _ => hi = mid,
        }
    }
    lo
}

/// The transcript's lines that contain one of `needles`, as JSON, from
/// `since` on (roughly: the seek lands up to a megabyte early). The first
/// line is always yielded: it names the session.
fn lines(file: &Path, since: Option<Timestamp>, needles: &[&str]) -> Result<Vec<Value>, String> {
    let mut f = File::open(file).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut first = String::new();
    {
        let mut reader = BufReader::new(&mut f);
        reader.read_line(&mut first).map_err(|e| e.to_string())?;
    }
    if first.trim().is_empty() {
        return Ok(out);
    }
    // a transcript is JSON lines; a first line that is not is no transcript
    let head: Value =
        serde_json::from_str(&first).map_err(|_| "its first line is not JSON".to_owned())?;
    out.push(head);
    let start = match since {
        Some(t) if f.metadata().is_ok_and(|m| m.len() > 8 << 20) => seek_since(&mut f, t),
        _ => 0,
    };
    f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let finders: Vec<memchr::memmem::Finder<'_>> = needles
        .iter()
        .map(|n| memchr::memmem::Finder::new(n.as_bytes()))
        .collect();
    let mut reader = BufReader::with_capacity(1 << 20, f);
    let mut buf = Vec::new();
    // skip the first line (yielded above) or the partial line the seek landed in
    let skipped = reader
        .read_until(b'\n', &mut buf)
        .map_err(|e| e.to_string())?;
    let mut malformed = None;
    let mut offset = start + skipped as u64;
    loop {
        buf.clear();
        // a read error loses the rest of the transcript: the reader says so
        if reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| e.to_string())?
            == 0
        {
            break;
        }
        // only the last line may be malformed: it is being written
        if let Some(line) = malformed.take() {
            return Err(format!("a line {line} bytes in is not JSON"));
        }
        let at = offset;
        offset += buf.len() as u64;
        let wanted = finders.iter().any(|f| f.find(&buf).is_some());
        if wanted {
            match serde_json::from_slice::<Value>(&buf) {
                Ok(v) => out.push(v),
                Err(_) if buf.ends_with(b"\n") => malformed = Some(at),
                Err(_) => {}
            }
        }
    }
    Ok(out)
}

/// A Claude Code transcript, if its working directory is kept.
fn claude_kept(
    file: &Path,
    keep_cwd: &dyn Fn(&str) -> bool,
    since: Option<Timestamp>,
) -> Result<Option<Transcript>, String> {
    let t = claude_since(file, since)?;
    Ok(most(&t.cwd).filter(|cwd| keep_cwd(cwd)).map(|_| t))
}

/// A Claude Code transcript. Subagent transcripts live under `subagents/`.
#[must_use]
pub fn claude(file: &Path) -> Transcript {
    claude_since(file, None).unwrap_or_default()
}

fn claude_since(file: &Path, since: Option<Timestamp>) -> Result<Transcript, String> {
    let mut t = Transcript {
        harness: "claude",
        subagent: file.components().any(|c| c.as_os_str() == "subagents"),
        source: file.display().to_string(),
        ..Transcript::default()
    };
    for o in lines(file, since, &["\"tool_use\""])? {
        if t.session.is_none() {
            t.session = o["sessionId"].as_str().map(str::to_owned);
        }
        if t.subagent && t.root.is_none() && t.session.is_some() {
            t.root.clone_from(&t.session);
            if let Some(agent) = o["agentId"].as_str() {
                t.session = Some(format!("agent-{agent}"));
            }
        }
        // a line with no time is not a turn (a summary, a header); a time
        // that does not parse is a transcript andon cannot read
        let Some(ts) = o["timestamp"].as_str() else {
            continue;
        };
        let ts = parse_instant(ts).map_err(|e| format!("a turn's time {ts:?}: {e}"))?;
        if let Some(cwd) = o["cwd"].as_str() {
            *t.cwd.entry(cwd.to_owned()).or_default() += 1;
        }
        if o["type"] != "assistant" {
            continue;
        }
        for c in o["message"]["content"].as_array().into_iter().flatten() {
            if c["type"] != "tool_use" {
                continue;
            }
            t.calls.push(ts);
            let name = c["name"].as_str().unwrap_or_default();
            if matches!(name, "Agent" | "Task") {
                let what = c["input"]["description"].as_str().unwrap_or_default();
                t.spawns.push((ts, what.to_owned()));
            }
        }
    }
    Ok(t)
}

/// A Codex rollout; `Ok(None)` when its first line names a directory not
/// kept, or (with `root`) when the directory it is attributed to is not
/// under `root`. The attribution is read first from the rollout's small
/// `turn_context` lines, so a scoped read parses only its seat's rollouts.
///
/// # Errors
/// When the file cannot be read or is not a rollout.
pub fn codex(
    file: &Path,
    keep_cwd: &dyn Fn(&str) -> bool,
    root: Option<&str>,
    since: Option<Timestamp>,
) -> Result<Option<Transcript>, String> {
    let mut t = Transcript {
        harness: "codex",
        source: file.display().to_string(),
        ..Transcript::default()
    };
    // the first line says whose session this is: read it before the rest
    let meta = {
        let mut first = String::new();
        BufReader::new(File::open(file).map_err(|e| e.to_string())?)
            .read_line(&mut first)
            .map_err(|e| e.to_string())?;
        if first.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str::<Value>(&first)
            .map_err(|_| "its first line is not JSON".to_owned())?
    };
    if meta["type"] == "session_meta" {
        let payload = &meta["payload"];
        let cwd = payload["cwd"].as_str().unwrap_or_default();
        if !keep_cwd(cwd) {
            return Ok(None);
        }
        t.session = payload["id"].as_str().map(str::to_owned);
        t.subagent = payload["thread_source"] == "subagent";
        if t.subagent {
            t.root = payload["session_id"].as_str().map(str::to_owned);
        }
        *t.cwd.entry(cwd.to_owned()).or_default() += 1;
    }
    let turns = lines(file, since, &["\"turn_context\""])?;
    for o in turns.iter().skip(1) {
        if o["type"] == "turn_context"
            && let Some(cwd) = o["payload"]["cwd"].as_str()
        {
            *t.cwd.entry(cwd.to_owned()).or_default() += 1;
        }
    }
    if let Some(root) = root {
        let attributed = most(&t.cwd).unwrap_or_default();
        if !(attributed == root || attributed.starts_with(&format!("{root}/"))) {
            return Ok(None);
        }
    }
    let needles = ["\"function_call\"", "\"custom_tool_call\""];
    for o in lines(file, since, &needles)?.into_iter().skip(1) {
        let payload = &o["payload"];
        // a line with no time is not a turn (a summary, a header); a time
        // that does not parse is a transcript andon cannot read
        let Some(ts) = o["timestamp"].as_str() else {
            continue;
        };
        let ts = parse_instant(ts).map_err(|e| format!("a turn's time {ts:?}: {e}"))?;
        let kind = payload["type"].as_str().unwrap_or_default();
        if matches!(kind, "function_call" | "custom_tool_call") {
            t.calls.push(ts);
            let name = payload["name"].as_str().unwrap_or_default();
            if name.contains("spawn") {
                t.spawns.push((ts, name.to_owned()));
            }
        }
    }
    Ok(Some(t))
}

/// Activity spans and spawns for one transcript.
#[must_use]
pub fn spans(
    t: &Transcript,
    cwd: &str,
    gap: SignedDuration,
    since: Option<Timestamp>,
) -> Vec<Event> {
    let mut calls = t.calls.clone();
    calls.sort();
    let actor = Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    let (session, critic) = if t.subagent {
        (
            t.root.clone().or_else(|| t.session.clone()),
            t.session.clone(),
        )
    } else {
        (t.session.clone(), None)
    };
    let make = |start: Timestamp, kind: Kind| {
        let mut e = Event::new(start, kind, t.source.clone());
        e.actor.clone_from(&actor);
        e.ids.session.clone_from(&session);
        e.ids.critic_session.clone_from(&critic);
        e.set_attr(attr::CWD, cwd.to_owned());
        e.set_attr(attr::HARNESS, t.harness);
        e
    };
    let mut out = Vec::new();
    let mut span: Option<(Timestamp, Timestamp, usize)> = None;
    for call in calls {
        span = match span {
            Some((start, end, n)) if call.duration_since(end) <= gap => Some((start, call, n + 1)),
            Some((start, end, n)) => {
                out.push((start, end, n));
                Some((call, call, 1))
            }
            None => Some((call, call, 1)),
        };
    }
    out.extend(span);
    let mut events: Vec<Event> = out
        .into_iter()
        .filter(|(_, end, _)| since.is_none_or(|s| *end >= s))
        .map(|(start, end, n)| {
            let mut e = make(start, Kind::SessionActive);
            e.t_end = Some(end);
            #[allow(clippy::cast_possible_wrap)]
            e.set_attr(attr::CALLS, n as i64);
            e
        })
        .collect();
    for (ts, what) in &t.spawns {
        if since.is_none_or(|s| *ts >= s) {
            let mut e = make(*ts, Kind::Spawn);
            e.set_attr(attr::DESCRIPTION, what.clone());
            events.push(e);
        }
    }
    events
}

fn most(counts: &BTreeMap<String, usize>) -> Option<String> {
    let mut best: Option<(&String, usize)> = None;
    for (k, &n) in counts {
        if best.is_none_or(|(_, m)| n > m) {
            best = Some((k, n));
        }
    }
    best.map(|(k, _)| k.clone())
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "fixtures write the transcripts they read"
)]
mod tests {
    use super::*;

    #[test]
    fn calls_closer_than_the_gap_form_one_span() {
        let ts = |s: &str| parse_instant(s).unwrap_or_else(|e| panic!("{e}"));
        let t = Transcript {
            harness: "claude",
            session: Some("s1".into()),
            calls: vec![
                ts("2026-10-13T10:00:00Z"),
                ts("2026-10-13T10:05:00Z"),
                ts("2026-10-13T10:30:00Z"),
            ],
            spawns: vec![(ts("2026-10-13T10:05:00Z"), "critic".into())],
            source: "t.jsonl".into(),
            ..Transcript::default()
        };
        let events = spans(&t, "/code/andon-1b", SignedDuration::from_mins(10), None);
        let spans: Vec<(Timestamp, Option<Timestamp>)> = events
            .iter()
            .filter(|e| e.kind == Kind::SessionActive)
            .map(|e| (e.t, e.t_end))
            .collect();
        assert_eq!(
            spans,
            vec![
                (ts("2026-10-13T10:00:00Z"), Some(ts("2026-10-13T10:05:00Z"))),
                (ts("2026-10-13T10:30:00Z"), Some(ts("2026-10-13T10:30:00Z"))),
            ]
        );
        assert_eq!(events[0].actor.as_deref(), Some("andon-1b"));
        assert_eq!(events.iter().filter(|e| e.kind == Kind::Spawn).count(), 1);
    }

    #[test]
    fn a_claude_transcript_yields_calls_cwd_and_spawns() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let file = dir.path().join("s.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"type":"user","timestamp":"2026-10-13T10:00:00Z","sessionId":"s1","cwd":"/code/andon-1b"}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-10-13T10:00:05Z","sessionId":"s1","cwd":"/code/andon-1b","message":{"content":[{"type":"tool_use","name":"Bash","input":{}},{"type":"tool_use","name":"Agent","input":{"description":"review"}}]}}"#,
                "\n",
                "not json\n"
            ),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let t = claude(&file);
        assert_eq!(t.session.as_deref(), Some("s1"));
        assert_eq!(t.calls.len(), 2);
        assert_eq!(t.spawns.len(), 1);
        assert_eq!(most(&t.cwd).as_deref(), Some("/code/andon-1b"));
    }
}
