//! sessions: harness transcripts (Claude Code JSONL, Codex rollouts).
//!
//! A transcript is an artifact the harness writes as the agent works; andon
//! reads the facts in it (when tools were called, from which directory, which
//! subagents started) and nothing the agent asserted. Tool-call times become
//! activity spans: calls closer than `gap_minutes` belong to one span.
//!
//! Emits `session_active` spans (the actor is the working directory's name,
//! so a workspace is a seat), `spawn` for each subagent started, and `edit`
//! for each file an edit tool call wrote (Claude Code's `Edit`, `Write`,
//! `MultiEdit` and `NotebookEdit`, Codex's `apply_patch`). A subagent's
//! events carry its root session in `ids.session` and its own id in
//! `ids.critic_session`, so its effort counts for the seat that started it.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Result;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adapters::{Reads, cache};
use crate::config::{Config, SessionsConfig, wildcard};
use crate::event::attr;
use crate::event::{Event, Kind};
use crate::time::parse_instant;

/// What one transcript says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub harness: String,
    pub session: Option<String>,
    /// The root session, for a subagent.
    pub root: Option<String>,
    pub subagent: bool,
    /// Each tool call: when, and the directory it ran in.
    pub calls: Vec<(Timestamp, String)>,
    /// Each subagent started: when, what for, and from which directory.
    pub spawns: Vec<(Timestamp, String, String)>,
    /// Files edit tool calls wrote: when, the tool, the path as written,
    /// and the directory the call ran in.
    pub edits: Vec<(Timestamp, String, String, String)>,
    pub source: String,
}

impl Transcript {
    /// Every directory a call ran in.
    #[must_use]
    pub fn cwds(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.calls.iter().map(|(_, c)| c.as_str()).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The calls, spawns and edits made in a directory `keep` accepts, each
    /// directory taken to its workspace: the shortest kept directory of the
    /// transcript's that contains it (a call in `ws/src` is `ws`'s, when the
    /// session also worked in `ws`).
    #[must_use]
    pub fn kept(mut self, keep: &dyn Fn(&str) -> bool) -> Self {
        let dirs: Vec<String> = self
            .cwds()
            .into_iter()
            .filter(|c| keep(c))
            .map(str::to_owned)
            .collect();
        let root = |cwd: &str| -> Option<String> {
            dirs.iter()
                .filter(|d| Path::new(cwd).starts_with(d.as_str()))
                .min_by_key(|d| d.len())
                .cloned()
        };
        self.calls = std::mem::take(&mut self.calls)
            .into_iter()
            .filter_map(|(t, c)| Some((t, root(&c)?)))
            .collect();
        self.spawns = std::mem::take(&mut self.spawns)
            .into_iter()
            .filter_map(|(t, w, c)| Some((t, w, root(&c)?)))
            .collect();
        self.edits = std::mem::take(&mut self.edits)
            .into_iter()
            .filter_map(|(t, tool, p, c)| Some((t, tool, p, root(&c)?)))
            .collect();
        self
    }
}

/// The kinds a transcript yields.
const KINDS: &[&str] = &["session_active", "spawn", "edit"];

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
    let in_root = |cwd: &str| {
        scope
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
    // transcripts are append-only: a read resumes where the last one stopped
    let kept = cache::Transcripts::open();
    let cache = kept.as_ref();
    let read: Vec<(PathBuf, Result<Option<Transcript>, String>)> = std::thread::scope(|scope_| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                let configured = &configured;
                scope_.spawn(move || {
                    part.iter()
                        .map(|(file, is_codex)| {
                            let got = if *is_codex {
                                codex_cached(file, configured, root, cache)
                            } else {
                                read(file, cache, new_claude, claude_from).map(Some)
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
    let mut events = reads.gaps_since(0, now, KINDS, None);
    for (file, got) in read {
        let t = match got {
            Ok(Some(t)) => t,
            Ok(None) => continue,
            // an unreadable transcript is a gap: whose sessions it held is
            // unknown, so every seat's effort is unknown while it stands
            Err(why) => {
                events.push(unreadable(&file, &why, now));
                continue;
            }
        };
        // each call counts for the directory it ran in, never for where
        // the session mostly worked: a session that moves between
        // workspaces is each seat's for its own part
        let t = t.kept(&|cwd| configured(cwd) && in_root(cwd));
        events.extend(spans(&t, gap, since));
    }
    // sessions are covered from `since` (or all history) to now; a scoped
    // read covers only its seat
    events.push(crate::event::coverage(
        since.unwrap_or(crate::event::ALL_HISTORY),
        Some(now),
        KINDS,
        "transcripts",
        scope.seat.as_deref(),
    ));
    Ok(events)
}

/// The gap an unreadable transcript leaves. Its turns were all written
/// before its last write, so the gap stands over spans up to then, not over
/// every window after.
fn unreadable(file: &Path, why: &str, now: Timestamp) -> Event {
    let reason = format!("{}: {why}", file.display());
    let source = file.display().to_string();
    match last_write(file) {
        Some(t) => {
            crate::event::gap_over(crate::event::ALL_HISTORY, t, KINDS, source, None, reason)
        }
        None => crate::event::gap(now, KINDS, source, None, reason),
    }
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

/// Where a read of a transcript stopped, and what the lines after it
/// carry on from: transcripts are append-only, so a read can resume.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resume {
    /// The byte after the last line read.
    pub offset: u64,
    /// The directory the last line ran in.
    pub cwd: String,
    /// A Codex rollout's first directory (its `session_meta`).
    pub started_in: Option<String>,
}

/// Read a transcript's JSON lines from `from`, calling `each` with every
/// line that holds one of `needles` (and the first line, at the start: it
/// names the session). Returns where the read stopped: after the last
/// complete line, or before a malformed last line (it is being written).
/// With `partial`, a last line with no newline yet is read too when it is
/// JSON, though the read still stops before it.
fn scan(
    file: &Path,
    from: u64,
    needles: &[&str],
    partial: bool,
    each: &mut dyn FnMut(Value) -> Result<(), String>,
) -> Result<u64, String> {
    let mut f = File::open(file).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(from)).map_err(|e| e.to_string())?;
    let finders: Vec<memchr::memmem::Finder<'_>> = needles
        .iter()
        .map(|n| memchr::memmem::Finder::new(n.as_bytes()))
        .collect();
    let mut reader = BufReader::with_capacity(1 << 20, f);
    let mut buf = Vec::new();
    let mut offset = from;
    if from == 0 {
        reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| e.to_string())?;
        if buf.trim_ascii().is_empty() {
            return Ok(if buf.ends_with(b"\n") {
                buf.len() as u64
            } else {
                0
            });
        }
        let complete = buf.ends_with(b"\n");
        if !complete && !partial {
            return Ok(0);
        }
        // a transcript is JSON lines; a first line that is not is no transcript
        let head: Value =
            serde_json::from_slice(&buf).map_err(|_| "its first line is not JSON".to_owned())?;
        each(head)?;
        if !complete {
            return Ok(0);
        }
        offset = buf.len() as u64;
    }
    let mut malformed: Option<u64> = None;
    loop {
        buf.clear();
        // a read error loses the rest of the transcript: the reader says so
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        // a line still being written is read next time, and now only when
        // asked and already whole JSON
        if !buf.ends_with(b"\n") {
            if partial
                && malformed.is_none()
                && finders.iter().any(|f| f.find(&buf).is_some())
                && let Ok(v) = serde_json::from_slice::<Value>(&buf)
            {
                each(v)?;
            }
            break;
        }
        // only the last line may be malformed: it is being written
        if let Some(line) = malformed {
            return Err(format!("a line {line} bytes in is not JSON"));
        }
        let at = offset;
        if finders.iter().any(|f| f.find(&buf).is_some()) {
            if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
                each(v)?;
            } else {
                malformed = Some(at);
                continue;
            }
        }
        offset += buf.len() as u64;
    }
    Ok(offset)
}

/// A Claude Code transcript. Subagent transcripts live under `subagents/`.
#[must_use]
pub fn claude(file: &Path) -> Transcript {
    let mut t = new_claude(file);
    let mut resume = Resume::default();
    match claude_from(file, &mut t, &mut resume, true) {
        Ok(()) => t,
        Err(_) => Transcript::default(),
    }
}

fn new_claude(file: &Path) -> Transcript {
    Transcript {
        harness: "claude".into(),
        subagent: file.components().any(|c| c.as_os_str() == "subagents"),
        source: file.display().to_string(),
        ..Transcript::default()
    }
}

/// Read a Claude Code transcript on from where `resume` stopped.
fn claude_from(
    file: &Path,
    t: &mut Transcript,
    resume: &mut Resume,
    partial: bool,
) -> Result<(), String> {
    // a line names the directory it ran in; one that does not ran where
    // the last one did
    let mut cwd = std::mem::take(&mut resume.cwd);
    let offset = scan(file, resume.offset, &["\"tool_use\""], partial, &mut |o| {
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
            return Ok(());
        };
        let ts = parse_instant(ts).map_err(|e| format!("a turn's time {ts:?}: {e}"))?;
        if let Some(dir) = o["cwd"].as_str() {
            dir.clone_into(&mut cwd);
        }
        if o["type"] != "assistant" {
            return Ok(());
        }
        for c in o["message"]["content"].as_array().into_iter().flatten() {
            if c["type"] != "tool_use" {
                continue;
            }
            t.calls.push((ts, cwd.clone()));
            let name = c["name"].as_str().unwrap_or_default();
            if matches!(name, "Agent" | "Task") {
                let what = c["input"]["description"].as_str().unwrap_or_default();
                t.spawns.push((ts, what.to_owned(), cwd.clone()));
            }
            let path = match name {
                "Edit" | "Write" | "MultiEdit" => c["input"]["file_path"].as_str(),
                "NotebookEdit" => c["input"]["notebook_path"].as_str(),
                _ => None,
            };
            if let Some(path) = path.filter(|p| !p.is_empty()) {
                t.edits
                    .push((ts, name.to_owned(), path.to_owned(), cwd.clone()));
            }
        }
        Ok(())
    })?;
    resume.offset = offset;
    resume.cwd = cwd;
    Ok(())
}

/// A Codex rollout's first directory, from its first line (`None` when it
/// names none): read before the rest, since rollouts are many and large.
fn codex_started_in(file: &Path) -> Result<Option<String>, String> {
    let mut first = String::new();
    BufReader::new(File::open(file).map_err(|e| e.to_string())?)
        .read_line(&mut first)
        .map_err(|e| e.to_string())?;
    if first.trim().is_empty() {
        return Ok(None);
    }
    let meta: Value =
        serde_json::from_str(&first).map_err(|_| "its first line is not JSON".to_owned())?;
    Ok(Some(
        (meta["type"] == "session_meta")
            .then(|| meta["payload"]["cwd"].as_str())
            .flatten()
            .unwrap_or_default()
            .to_owned(),
    ))
}

fn new_codex(file: &Path) -> Transcript {
    Transcript {
        harness: "codex".into(),
        source: file.display().to_string(),
        ..Transcript::default()
    }
}

/// Read a Codex rollout on from where `resume` stopped.
fn codex_from(
    file: &Path,
    t: &mut Transcript,
    resume: &mut Resume,
    partial: bool,
) -> Result<(), String> {
    let mut cwd = std::mem::take(&mut resume.cwd);
    let mut started_in = resume.started_in.take();
    let at_start = resume.offset == 0;
    let needles = [
        "\"turn_context\"",
        "\"function_call\"",
        "\"custom_tool_call\"",
    ];
    let mut first = at_start;
    let offset = scan(file, resume.offset, &needles, partial, &mut |o| {
        let payload = &o["payload"];
        // the first line says whose session this is
        if std::mem::take(&mut first) {
            if o["type"] == "session_meta" {
                payload["cwd"]
                    .as_str()
                    .unwrap_or_default()
                    .clone_into(&mut cwd);
                t.session = payload["id"].as_str().map(str::to_owned);
                t.subagent = payload["thread_source"] == "subagent";
                if t.subagent {
                    t.root = payload["session_id"].as_str().map(str::to_owned);
                }
            }
            started_in = Some(cwd.clone());
            return Ok(());
        }
        // a turn says where the calls after it run
        if o["type"] == "turn_context" {
            if let Some(dir) = payload["cwd"].as_str() {
                dir.clone_into(&mut cwd);
            }
            return Ok(());
        }
        // a line with no time is not a turn (a summary, a header); a time
        // that does not parse is a transcript andon cannot read
        let Some(ts) = o["timestamp"].as_str() else {
            return Ok(());
        };
        let ts = parse_instant(ts).map_err(|e| format!("a turn's time {ts:?}: {e}"))?;
        let kind = payload["type"].as_str().unwrap_or_default();
        if matches!(kind, "function_call" | "custom_tool_call") {
            t.calls.push((ts, cwd.clone()));
            let name = payload["name"].as_str().unwrap_or_default();
            if name.contains("spawn") {
                t.spawns.push((ts, name.to_owned(), cwd.clone()));
            }
            if name == "apply_patch" {
                for path in patch_paths(payload) {
                    t.edits.push((ts, name.to_owned(), path, cwd.clone()));
                }
            }
        }
        Ok(())
    })?;
    resume.offset = offset;
    resume.cwd = cwd;
    resume.started_in = started_in;
    Ok(())
}

/// A Codex rollout; `Ok(None)` when its first line names a directory not
/// kept (rollouts are many and large: one started outside every kept
/// directory is never read), or (with `root`) when it never worked under
/// `root`.
///
/// # Errors
/// When the file cannot be read or is not a rollout.
pub fn codex(
    file: &Path,
    keep_cwd: &dyn Fn(&str) -> bool,
    root: Option<&str>,
) -> Result<Option<Transcript>, String> {
    codex_cached(file, keep_cwd, root, None)
}

fn codex_cached(
    file: &Path,
    keep_cwd: &dyn Fn(&str) -> bool,
    root: Option<&str>,
    cache: Option<&cache::Transcripts>,
) -> Result<Option<Transcript>, String> {
    let Some(started_in) = codex_started_in(file)? else {
        return Ok(None);
    };
    if !keep_cwd(&started_in) {
        return Ok(None);
    }
    let t = read(file, cache, new_codex, codex_from)?;
    let under_root = |dir: &str| root.is_none_or(|r| dir == r || dir.starts_with(&format!("{r}/")));
    let worked = under_root(&started_in) || t.calls.iter().any(|(_, c)| under_root(c));
    Ok(worked.then_some(t))
}

/// A transcript, read whole or resumed from the cache.
fn read(
    file: &Path,
    cache: Option<&cache::Transcripts>,
    new: fn(&Path) -> Transcript,
    from: fn(&Path, &mut Transcript, &mut Resume, bool) -> Result<(), String>,
) -> Result<Transcript, String> {
    let kept = cache.and_then(|c| c.get(file));
    let (mut t, mut resume) = kept.unwrap_or_else(|| (new(file), Resume::default()));
    let before = resume.clone();
    from(file, &mut t, &mut resume, false)?;
    if let Some(c) = cache
        && resume != before
    {
        c.put(file, &t, &resume);
    }
    // a last line with no newline yet counts now, and is read again next
    // time from where the kept parse stopped
    if std::fs::metadata(file).is_ok_and(|m| m.len() > resume.offset) {
        from(file, &mut t, &mut resume.clone(), true)?;
    }
    Ok(t)
}

/// The files an `apply_patch` call names: its patch is the custom tool
/// call's `input`, or a string in the function call's JSON `arguments`.
fn patch_paths(payload: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    if let Some(input) = payload["input"].as_str() {
        texts.push(input.to_owned());
    }
    if let Some(args) = payload["arguments"].as_str()
        && let Ok(v) = serde_json::from_str::<Value>(args)
    {
        strings(&v, &mut texts);
    }
    let mut paths: Vec<String> = Vec::new();
    for text in texts {
        for line in text.lines() {
            let path = [
                "*** Update File: ",
                "*** Add File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ]
            .iter()
            .find_map(|p| line.strip_prefix(p));
            if let Some(path) = path.map(str::trim).filter(|p| !p.is_empty())
                && !paths.iter().any(|p| p == path)
            {
                paths.push(path.to_owned());
            }
        }
    }
    paths
}

fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|x| strings(x, out)),
        Value::Object(map) => map.values().for_each(|x| strings(x, out)),
        _ => {}
    }
}

/// An edited path as the board matches it: relative to the workspace root
/// `cwd` when under it (so one file in two workspaces is one path), as
/// written when relative (to the turn's directory), else absolute.
#[must_use]
pub fn workspace_path(path: &str, cwd: &str) -> String {
    let p = Path::new(path);
    match p.strip_prefix(cwd) {
        Ok(rel) if p.is_absolute() && !rel.as_os_str().is_empty() => {
            rel.to_string_lossy().into_owned()
        }
        _ => path.to_owned(),
    }
}

/// Activity spans, spawns and edits for one transcript, each counted for
/// the directory its call ran in (the actor is that directory's name, so a
/// workspace is a seat). Calls closer than `gap` form one span; a span
/// whose calls move to another directory ends where the move is, and the
/// next begins there.
#[must_use]
pub fn spans(t: &Transcript, gap: SignedDuration, since: Option<Timestamp>) -> Vec<Event> {
    let mut calls = t.calls.clone();
    calls.sort();
    let (session, critic) = if t.subagent {
        (
            t.root.clone().or_else(|| t.session.clone()),
            t.session.clone(),
        )
    } else {
        (t.session.clone(), None)
    };
    let make = |start: Timestamp, kind: Kind, cwd: &str| {
        let mut e = Event::new(start, kind, t.source.clone());
        e.actor = Path::new(cwd)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        e.ids.session.clone_from(&session);
        e.ids.critic_session.clone_from(&critic);
        e.set_attr(attr::CWD, cwd.to_owned());
        e.set_attr(attr::HARNESS, t.harness.clone());
        e
    };
    let mut out: Vec<(Timestamp, Timestamp, usize, &str)> = Vec::new();
    let mut span: Option<(Timestamp, Timestamp, usize, &str)> = None;
    for (call, cwd) in &calls {
        let call = *call;
        span = match span {
            Some((start, end, n, dir)) if call.duration_since(end) <= gap && dir == cwd => {
                Some((start, call, n + 1, dir))
            }
            // the session moved on within the gap: the time until the move
            // is the earlier directory's
            Some((start, end, n, dir)) if call.duration_since(end) <= gap => {
                out.push((start, call, n, dir));
                Some((call, call, 1, cwd))
            }
            Some(done) => {
                out.push(done);
                Some((call, call, 1, cwd))
            }
            None => Some((call, call, 1, cwd)),
        };
    }
    out.extend(span);
    let mut events: Vec<Event> = out
        .into_iter()
        .filter(|(_, end, _, _)| since.is_none_or(|s| *end >= s))
        .map(|(start, end, n, cwd)| {
            let mut e = make(start, Kind::SessionActive, cwd);
            e.t_end = Some(end);
            #[allow(clippy::cast_possible_wrap)]
            e.set_attr(attr::CALLS, n as i64);
            e
        })
        .collect();
    for (ts, what, cwd) in &t.spawns {
        if since.is_none_or(|s| *ts >= s) {
            let mut e = make(*ts, Kind::Spawn, cwd);
            e.set_attr(attr::DESCRIPTION, what.clone());
            events.push(e);
        }
    }
    for (ts, tool, path, cwd) in &t.edits {
        if since.is_none_or(|s| *ts >= s) {
            let mut e = make(*ts, Kind::Edit, cwd);
            e.set_attr(attr::TOOL, tool.clone());
            e.set_attr(attr::PATH, workspace_path(path, cwd));
            events.push(e);
        }
    }
    events
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
            harness: "claude".into(),
            session: Some("s1".into()),
            calls: vec![
                (ts("2026-10-13T10:00:00Z"), "/code/andon-1b".into()),
                (ts("2026-10-13T10:05:00Z"), "/code/andon-1b".into()),
                (ts("2026-10-13T10:30:00Z"), "/code/andon-1b".into()),
            ],
            spawns: vec![(
                ts("2026-10-13T10:05:00Z"),
                "critic".into(),
                "/code/andon-1b".into(),
            )],
            source: "t.jsonl".into(),
            ..Transcript::default()
        };
        let events = spans(&t, SignedDuration::from_mins(10), None);
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
        assert_eq!(t.cwds(), vec!["/code/andon-1b"]);
    }

    #[test]
    fn edit_tool_calls_become_edits_relative_to_the_workspace() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let file = dir.path().join("s.jsonl");
        let call = |name: &str, input: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-10-13T10:00:05Z","sessionId":"s1","cwd":"/code/x-1b","message":{{"content":[{{"type":"tool_use","name":"{name}","input":{input}}}]}}}}"#
            )
        };
        let lines = [
            call("Edit", r#"{"file_path":"/code/x-1b/src/a.rs"}"#),
            call("Write", r#"{"file_path":"/code/x-1b/b.md"}"#),
            call("MultiEdit", r#"{"file_path":"/code/x-1/src/c.rs"}"#),
            call("NotebookEdit", r#"{"notebook_path":"/code/x-1b/n.ipynb"}"#),
            call("Read", r#"{"file_path":"/code/x-1b/src/r.rs"}"#),
        ];
        std::fs::write(&file, lines.join("\n") + "\n").unwrap_or_else(|e| panic!("{e}"));
        let t = claude(&file);
        let events = spans(&t.kept(&|_| true), SignedDuration::from_mins(10), None);
        let edits: Vec<(&str, &str)> = events
            .iter()
            .filter(|e| e.kind == Kind::Edit)
            .map(|e| {
                (
                    e.attr_str(attr::TOOL).unwrap_or_default(),
                    e.attr_str(attr::PATH).unwrap_or_default(),
                )
            })
            .collect();
        // x-1 is a sibling, not the workspace: its path stays absolute
        assert_eq!(
            edits,
            vec![
                ("Edit", "src/a.rs"),
                ("Write", "b.md"),
                ("MultiEdit", "/code/x-1/src/c.rs"),
                ("NotebookEdit", "n.ipynb"),
            ]
        );
        assert!(
            events
                .iter()
                .filter(|e| e.kind == Kind::Edit)
                .all(|e| e.actor.as_deref() == Some("x-1b"))
        );
    }

    #[test]
    fn an_apply_patch_names_each_file_it_touches() {
        let patch = "*** Begin Patch\n*** Update File: /code/x-1a/src/a.rs\n@@\n-x\n+y\n*** Add File: docs/new.md\n+hi\n*** Delete File: /code/x-1a/old.rs\n*** Update File: /code/x-1a/src/b.rs\n*** Move to: /code/x-1a/src/c.rs\n*** End Patch\n";
        let custom =
            serde_json::json!({"type": "custom_tool_call", "name": "apply_patch", "input": patch});
        let args = serde_json::json!({"input": patch}).to_string();
        let function =
            serde_json::json!({"type": "function_call", "name": "apply_patch", "arguments": args});
        let want = vec![
            "/code/x-1a/src/a.rs",
            "docs/new.md",
            "/code/x-1a/old.rs",
            "/code/x-1a/src/b.rs",
            "/code/x-1a/src/c.rs",
        ];
        assert_eq!(patch_paths(&custom), want);
        assert_eq!(patch_paths(&function), want);
        assert_eq!(
            workspace_path("/code/x-1a/src/a.rs", "/code/x-1a"),
            "src/a.rs"
        );
        assert_eq!(workspace_path("docs/new.md", "/code/x-1a"), "docs/new.md");
        assert_eq!(workspace_path("/code/x-1a", "/code/x-1a"), "/code/x-1a");
    }

    #[test]
    fn a_session_that_moves_credits_each_workspace_with_its_own_part() {
        // one transcript: 10:00-10:20 in andon (and its src), then 10:30-11:00
        // in andon-1a, editing there
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let file = dir.path().join("s.jsonl");
        let call = |t: &str, cwd: &str, name: &str, input: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{t}","sessionId":"s1","cwd":"{cwd}","message":{{"content":[{{"type":"tool_use","name":"{name}","input":{input}}}]}}}}"#
            )
        };
        let lines = [
            call("2026-10-13T10:00:00Z", "/code/andon", "Bash", "{}"),
            call("2026-10-13T10:10:00Z", "/code/andon/src", "Bash", "{}"),
            call("2026-10-13T10:20:00Z", "/code/andon", "Bash", "{}"),
            call(
                "2026-10-13T10:30:00Z",
                "/code/andon-1a",
                "Edit",
                r#"{"file_path":"/code/andon-1a/src/x.rs"}"#,
            ),
            call("2026-10-13T10:45:00Z", "/code/andon-1a", "Bash", "{}"),
            call("2026-10-13T11:00:00Z", "/code/andon-1a", "Bash", "{}"),
        ];
        std::fs::write(&file, lines.join("\n") + "\n").unwrap_or_else(|e| panic!("{e}"));
        let t = claude(&file).kept(&|_| true);
        let events = spans(&t, SignedDuration::from_mins(15), None);
        let got: Vec<(&str, &str, Option<Timestamp>, &str)> = events
            .iter()
            .map(|e| {
                (
                    e.kind.as_str(),
                    e.actor.as_deref().unwrap_or_default(),
                    e.t_end,
                    e.attr_str(attr::PATH).unwrap_or_default(),
                )
            })
            .collect();
        let ts = |s: &str| Some(parse_instant(s).unwrap_or_else(|e| panic!("{e}")));
        // the move at 10:30 ends andon's span there; a call in andon/src is
        // andon's
        assert_eq!(
            got,
            vec![
                ("session_active", "andon", ts("2026-10-13T10:30:00Z"), ""),
                ("session_active", "andon-1a", ts("2026-10-13T11:00:00Z"), ""),
                ("edit", "andon-1a", None, "src/x.rs"),
            ]
        );
        // a read kept to one workspace keeps only its part
        let t = claude(&file).kept(&|c| c.starts_with("/code/andon-1a"));
        let events = spans(&t, SignedDuration::from_mins(15), None);
        assert!(
            events
                .iter()
                .all(|e| e.actor.as_deref() == Some("andon-1a"))
        );
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn a_codex_rollout_credits_each_call_to_its_turns_directory() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let file = dir.path().join("r.jsonl");
        let lines = [
            r#"{"timestamp":"2026-10-13T10:00:00Z","type":"session_meta","payload":{"id":"r1","cwd":"/code/x-dev"}}"#,
            r#"{"timestamp":"2026-10-13T10:00:01Z","type":"response_item","payload":{"type":"function_call","name":"exec"}}"#,
            r#"{"timestamp":"2026-10-13T10:05:00Z","type":"turn_context","payload":{"cwd":"/code/x-1b"}}"#,
            r#"{"timestamp":"2026-10-13T10:05:01Z","type":"response_item","payload":{"type":"function_call","name":"exec"}}"#,
        ];
        std::fs::write(&file, lines.join("\n") + "\n").unwrap_or_else(|e| panic!("{e}"));
        let keep = |c: &str| c.starts_with("/code/x-");
        let t = codex(&file, &keep, None)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("the rollout worked in a kept directory"));
        let events = spans(&t.kept(&keep), SignedDuration::from_mins(10), None);
        let actors: Vec<&str> = events.iter().filter_map(|e| e.actor.as_deref()).collect();
        assert_eq!(actors, vec!["x-dev", "x-1b"]);
        // scoped to another workspace, the rollout is not read at all
        assert!(matches!(codex(&file, &keep, Some("/code/x-1a")), Ok(None)));
        // nor is one that started outside every kept directory
        let only_1b = |c: &str| c.starts_with("/code/x-1b");
        assert!(matches!(codex(&file, &only_1b, None), Ok(None)));
    }

    #[test]
    fn a_resumed_read_is_the_whole_read() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let file = dir.path().join("s.jsonl");
        let line = |t: &str, cwd: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{t}","sessionId":"s1","cwd":"{cwd}","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{}}}}]}}}}"#
            )
        };
        let cache = cache::Transcripts::at(&dir.path().join("cache"));
        let fresh = |file: &Path| read(file, None, new_claude, claude_from);
        let cached = |file: &Path| read(file, Some(&cache), new_claude, claude_from);
        let write = |text: String| std::fs::write(&file, text).unwrap_or_else(|e| panic!("{e}"));
        let mut text = format!(
            "{}\n{}\n",
            line("2026-10-13T10:00:00Z", "/x-1a"),
            line("2026-10-13T10:05:00Z", "/x-1a")
        );
        write(text.clone());
        assert_eq!(cached(&file), fresh(&file));
        // appended, with a last line still being written: resumed, it is
        // what a whole read gives, and the half line counts once
        text.push_str(&line("2026-10-13T10:10:00Z", "/x-1b"));
        text.push('\n');
        text.push_str(&line("2026-10-13T10:15:00Z", "/x-1b"));
        write(text.clone());
        let got = cached(&file);
        assert_eq!(got, fresh(&file));
        assert_eq!(got.as_ref().map(|t| t.calls.len()).ok(), Some(4));
        text.push('\n');
        write(text);
        assert_eq!(cached(&file).map(|t| t.calls.len()).ok(), Some(4));
        // rewritten from the start: the kept parse no longer describes it
        write(format!("{}\n", line("2026-10-14T09:00:00Z", "/x-1c")));
        let got = cached(&file);
        assert_eq!(got, fresh(&file));
        assert_eq!(got.map(|t| t.calls.len()).ok(), Some(1));
    }
}
