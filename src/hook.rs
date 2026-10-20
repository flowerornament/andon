//! `andon hook`: the seat's row, injected into the agent's turn (founding
//! design §6). The harness pipes its hook JSON on stdin and puts stdout into
//! the agent's context, so stdout carries only the row and its changes.
//!
//! - `SessionStart`: the seat's row.
//! - `UserPromptSubmit`: what changed for the seat since its last turn
//!   (a verdict, a claim or close, a landing, a refused land, budget spent,
//!   a file another seat's workspace changed too), then the row. Silent when nothing
//!   changed.
//!
//! The last turn is read from the transcript the payload names: the time of
//! its last assistant message. No state is kept between calls.
//!
//! A hook must never break a session: with no andon.toml, or on any error,
//! it prints nothing to stdout, explains on stderr, and exits 0.

use std::fmt::Write as _;

use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde_json::Value;

use crate::config::Config;
use crate::event::{Event, Kind, Outcome};
use crate::fold::board::{self, Row};
use crate::fold::projection::Projection;
use crate::ledger::Ledger;
use crate::render;
use crate::seat::Resolver;
use crate::time::parse_instant;

/// What the harness tells us.
#[derive(Debug, Default)]
pub struct Payload {
    pub event: String,
    pub cwd: Option<PathBuf>,
    pub transcript: Option<PathBuf>,
    /// Why the payload could not be read, when it could not.
    pub problem: Option<String>,
}

impl Payload {
    /// Parse a Claude Code or Codex hook payload.
    #[must_use]
    pub fn parse(json: &str) -> Self {
        let parsed = serde_json::from_str::<Value>(json);
        let problem = match &parsed {
            Err(_) if json.trim().is_empty() => Some("no hook payload on stdin".to_owned()),
            Err(_) => Some("the hook payload on stdin is not JSON".to_owned()),
            Ok(v) if !v.is_object() => Some("the hook payload is not a JSON object".to_owned()),
            Ok(_) => None,
        };
        let v = parsed.unwrap_or(Value::Null);
        let text = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
        Self {
            problem,
            event: text("hook_event_name")
                .or_else(|| text("event"))
                .unwrap_or("UserPromptSubmit")
                .to_owned(),
            cwd: text("cwd").map(PathBuf::from),
            transcript: text("transcript_path").map(PathBuf::from),
        }
    }
}

/// The seat a working directory belongs to: a configured cwd, else the
/// nearest directory whose name is a configured seat (a jj workspace is a
/// seat). With no seats configured, the workspace root's name: the nearest
/// directory holding `.jj` or `.git`.
#[must_use]
pub fn seat_of(cwd: &Path, config: &Config) -> Option<String> {
    let resolver = Resolver::new(config);
    if let Some(seat) = resolver.cwd(&cwd.to_string_lossy()) {
        return Some(seat);
    }
    if config.seats.is_empty() {
        let root = cwd
            .ancestors()
            .find(|dir| dir.join(".jj").exists() || dir.join(".git").exists())?;
        return root.file_name().map(|n| n.to_string_lossy().into_owned());
    }
    cwd.ancestors().find_map(|dir| {
        let name = dir.file_name()?.to_string_lossy();
        let seat = resolver.label(&name)?;
        config.seats.contains_key(&seat).then_some(seat)
    })
}

/// The seat's workspace directory: the nearest ancestor of `cwd` named for
/// the seat, else the nearest holding `.jj` or `.git`, else `cwd`.
#[must_use]
pub fn seat_root(cwd: &Path, seat: &str) -> PathBuf {
    cwd.ancestors()
        .find(|d| d.file_name().is_some_and(|n| n == seat))
        .or_else(|| {
            cwd.ancestors()
                .find(|d| d.join(".jj").exists() || d.join(".git").exists())
        })
        .unwrap_or(cwd)
        .to_path_buf()
}

/// The end of the agent's last turn: its last assistant message.
#[must_use]
pub fn last_turn(transcript: &Path) -> Option<Timestamp> {
    let text = std::fs::read_to_string(transcript).ok()?;
    text.lines().rev().find_map(|line| {
        let v: Value = serde_json::from_str(line).ok()?;
        let assistant = v["type"] == "assistant"
            || (v["type"] == "response_item" && v["payload"]["role"] == "assistant");
        if !assistant {
            return None;
        }
        parse_instant(v["timestamp"].as_str()?).ok()
    })
}

/// Where the hook reads its config and ledger from: `--config` and
/// `--ledger`, else the nearest andon.toml and the sources.
pub struct Sources<'a> {
    pub config: Option<&'a Path>,
    pub ledger: Option<&'a Path>,
}

/// `andon hook --check`: what a payload resolves to, line by line, and
/// whether the hook can work with it.
pub fn check(sources: &Sources<'_>, input: &str, now: Timestamp) -> (String, bool) {
    let mut report = String::new();
    let mut line = |ok: bool, what: &str, detail: String| {
        let _ = writeln!(
            report,
            "{} {what}: {detail}",
            if ok { "ok     " } else { "refused" }
        );
        ok
    };
    let payload = Payload::parse(input);
    if let Some(problem) = &payload.problem {
        line(
            false,
            "payload",
            format!("{problem}; pipe the harness's hook JSON to andon hook --check"),
        );
        return (report, false);
    }
    line(true, "payload", format!("{} event", payload.event));
    let cwd = payload
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let path = sources
        .config
        .map(Path::to_path_buf)
        .or_else(|| Config::find(&cwd));
    let Some(path) = path else {
        line(
            false,
            "config",
            format!(
                "no andon.toml in {} or above; add one, or pass --config",
                cwd.display()
            ),
        );
        return (report, false);
    };
    let config = match Config::load(&path) {
        Ok(mut c) => {
            c.capture = Some(now);
            line(true, "config", path.display().to_string());
            c
        }
        Err(e) => {
            line(false, "config", format!("{e:#}"));
            return (report, false);
        }
    };
    let Some(seat) = seat_of(&cwd, &config) else {
        line(
            false,
            "seat",
            format!(
                "{} belongs to no seat; add it under [seats] (cwd or prefixes)",
                cwd.display()
            ),
        );
        return (report, false);
    };
    let root = seat_root(&cwd, &seat);
    line(
        true,
        "seat",
        format!("{seat} (workspace {})", root.display()),
    );
    let ok = check_transcript_and_row(sources, &payload, &config, &seat, &root, now, &mut line);
    (report, ok)
}

/// The rest of `hook --check`: the transcript, then the row itself.
fn check_transcript_and_row(
    sources: &Sources<'_>,
    payload: &Payload,
    config: &Config,
    seat: &str,
    root: &Path,
    now: Timestamp,
    line: &mut dyn FnMut(bool, &str, String) -> bool,
) -> bool {
    let mut ok = true;
    match (
        &payload.transcript,
        payload.transcript.as_deref().and_then(last_turn),
    ) {
        (Some(p), Some(t)) => {
            line(
                true,
                "transcript",
                format!("{} (last turn {})", p.display(), render::short_time(t)),
            );
        }
        (Some(p), None) => {
            ok &= line(
                false,
                "transcript",
                format!(
                    "cannot read a last turn from {}; the hook cannot tell what changed",
                    p.display()
                ),
            );
        }
        (None, _) if payload.event == "SessionStart" => {
            line(
                true,
                "transcript",
                "none needed at session start".to_owned(),
            );
        }
        (None, _) => {
            ok &= line(
                false,
                "transcript",
                "the payload names no transcript_path; the hook cannot tell what changed"
                    .to_owned(),
            );
        }
    }
    let ledger = match sources.ledger {
        Some(p) => Ledger::open(p),
        None => crate::adapters::collect_for_seat(config, seat, root, now),
    };
    match ledger {
        Ok(ledger) => {
            let board = board::board(&ledger, config, now);
            let row = board.seats.iter().find(|r| r.seat == seat);
            match row {
                Some(r) => {
                    let columns = render::board_columns(config.goal_schema(), &board.seats);
                    line(true, "row", self::line(r, now, &columns));
                }
                None => {
                    ok &= line(
                        false,
                        "row",
                        format!("{seat} has no row; add it under [seats]"),
                    );
                }
            }
        }
        Err(e) => {
            ok &= line(false, "sources", format!("{e:#}"));
        }
    }
    ok
}

/// `andon hook`: the seat's row and what changed, for the harness to put
/// into the agent's turn.
///
/// # Errors
/// When the payload, the config or a source cannot be read; the caller
/// says so in one line and never fails the session.
pub fn run(sources: &Sources<'_>, input: &str, now: Timestamp) -> Result<String> {
    let payload = Payload::parse(input);
    if let Some(problem) = &payload.problem {
        return Err(anyhow!(
            "{problem}; the harness hook should pipe its JSON payload to `andon hook`"
        ));
    }
    let cwd = match &payload.cwd {
        Some(cwd) => cwd.clone(),
        None => std::env::current_dir()?,
    };
    let path = match sources.config {
        Some(p) => p.to_path_buf(),
        None => match Config::find(&cwd) {
            Some(p) => p,
            None => return Ok(String::new()),
        },
    };
    let mut config = Config::load(&path)?;
    config.capture = Some(now);
    let Some(seat) = seat_of(&cwd, &config) else {
        return Ok(String::new());
    };
    // a kept ledger when given; else only what this seat's row needs
    let ledger = if let Some(path) = sources.ledger {
        Ledger::open(path)?
    } else {
        let root = seat_root(&cwd, &seat);
        crate::adapters::collect_for_seat(&config, &seat, &root, now)?
    };
    let since = payload.transcript.as_deref().and_then(last_turn);
    let mut text = render(&ledger, &config, &seat, &payload.event, since, now);
    // on a prompt, a transcript andon cannot read means it cannot tell what
    // changed: say so rather than look like "nothing changed"
    if payload.event != "SessionStart" && since.is_none() {
        let why = match &payload.transcript {
            None => "the payload names no transcript_path".to_owned(),
            Some(p) => format!("cannot read a last turn from {}", p.display()),
        };
        let _ = writeln!(
            text,
            "andon: hook cannot tell what changed since the last turn: {why}; check the wiring: andon hook --check"
        );
    }
    Ok(text)
}

/// The hook's output for a seat at `now`. Pure: everything it reads is an
/// argument. It folds the ledger once: the row now and the goals held at the
/// last turn are both read off one projection.
#[must_use]
pub fn render(
    ledger: &Ledger,
    config: &Config,
    seat: &str,
    event: &str,
    since: Option<Timestamp>,
    now: Timestamp,
) -> String {
    let ledger = ledger.prefix(now);
    let p = Projection::new(&ledger, config);
    let now_board = board::board_of(&p, now);
    let Some(row) = now_board.seats.into_iter().find(|r| r.seat == seat) else {
        return String::new();
    };
    let columns = render::board_columns(config.goal_schema(), std::slice::from_ref(&row));
    if event == "SessionStart" {
        return format!("andon: {}\n", line(&row, now, &columns));
    }
    let Some(since) = since else {
        return String::new();
    };
    // claims and closes from a current-state source (a tracker export) are
    // not changes since the last turn: a claim released meanwhile is not in
    // it. They are listed only where some source reads them as history
    let mut changes = changes(&p, seat, since, now);
    let before = board::goals_at(&p, &board::gauger(&p, since), seat);
    let was = before.get().map_or(&[][..], Vec::as_slice);
    for goal in row.goals.get().map_or(&[][..], Vec::as_slice) {
        if config.goal_schema()
            && goal.over_budget()
            && !was.iter().any(|g| g.id == goal.id && g.over_budget())
        {
            changes.push(board::over_budget(&goal.id));
        }
    }
    // the workspaces are read only now, so an overlap is news when one of
    // its files changed on either side since the last turn: before then both
    // sides already held it
    let mine = board::unlanded(p.events(seat), now);
    for o in row.overlaps.get().map_or(&[][..], Vec::as_slice) {
        let theirs = board::unlanded(p.events(&o.seat), now);
        let touched = |f: &String| {
            [&mine, &theirs]
                .iter()
                .any(|side| side.get(f).is_some_and(|(t, _)| *t > since))
        };
        if o.files.iter().any(touched) {
            changes.push(board::overlap(o));
        }
    }
    let unread = unread(&p, seat, since, now);
    if changes.is_empty() && unread.is_empty() {
        return String::new();
    }
    let mut out = format!("andon: {}\n", line(&row, now, &columns));
    let mut said = capped(&changes);
    if !unread.is_empty() {
        // a count over a span the ledger did not fully read is not the
        // whole count: say which kinds, never a silent undercount (C3)
        let sep = if said.is_empty() { "" } else { "; " };
        let _ = write!(
            said,
            "{sep}not fully read since then: {}",
            unread.join(", ")
        );
    }
    let _ = writeln!(out, "  since your last turn: {said}");
    out
}

/// The kinds the change line counts that the ledger does not cover for the
/// seat over `since..now`.
fn unread(p: &Projection<'_>, seat: &str, since: Timestamp, now: Timestamp) -> Vec<&'static str> {
    let kinds: [(&[&str], &str); 5] = [
        (&["verdict", "commit"], "verdicts"),
        (&["gate_run"], "land runs"),
        (&["land"], "landings"),
        (&["close"], "closes"),
        (&["claim"], "claims"),
    ];
    kinds
        .into_iter()
        // a kind no source reads is the board's `?`, not news each turn
        .filter(|(k, _)| k.iter().all(|k| p.coverage.reads(k)))
        .filter(|(k, _)| since < now && p.coverage.check(k, Some(seat), since, now).is_some())
        .map(|(_, noun)| noun)
        .collect()
}

/// The row on one line, for injection: each column's cell after its name.
#[must_use]
pub fn line(row: &Row, now: Timestamp, columns: &[render::Column<Row>]) -> String {
    let mut parts = vec![row.seat.clone()];
    for column in columns.iter().skip(1) {
        let cell = (column.cell)(row, now);
        if cell.is_empty() || cell == "—" {
            continue;
        }
        match column.name {
            "flags" => parts.push(format!("⚑ {cell}")),
            "goal" => parts.push(cell),
            name => parts.push(format!("{name} {cell}")),
        }
    }
    parts.join("  ")
}

/// Facts about the seat recorded after its last turn.
fn changes(p: &Projection<'_>, seat: &str, since: Timestamp, now: Timestamp) -> Vec<String> {
    use std::collections::BTreeMap;
    let ledger = p.ledger;
    let history = |k: &str| p.coverage.reads(k);
    let mine = |e: &&Event| e.seat.as_deref() == Some(seat) && e.t > since && e.t <= now;
    let held: Vec<&str> = ledger
        .events()
        .iter()
        .filter(|e| e.kind == Kind::Claim && e.seat.as_deref() == Some(seat))
        .filter_map(|e| e.subject.as_deref())
        .collect();
    // grouped by what happened, then by goal, so a long absence is one line
    let mut verdicts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut lands: BTreeMap<String, usize> = BTreeMap::new();
    let mut claimed: Vec<String> = Vec::new();
    let mut closed: Vec<String> = Vec::new();
    let mut refused = 0;
    let goal = |e: &Event| e.subject.clone().unwrap_or_else(|| "no goal".into());
    for e in ledger.events().iter().filter(mine) {
        match e.kind {
            Kind::Verdict => {
                let said = e.outcome.map_or("verdict", Outcome::as_str).to_uppercase();
                *verdicts
                    .entry(said)
                    .or_default()
                    .entry(goal(e))
                    .or_default() += 1;
            }
            Kind::Claim => claimed.push(goal(e)),
            Kind::Land => *lands.entry(goal(e)).or_default() += 1,
            Kind::GateRun if p.refused_land(e) => refused += 1,
            _ => {}
        }
    }
    // closes carry no seat; they are the seat's when the goal is
    for e in ledger.events() {
        if e.kind == Kind::Close
            && e.t > since
            && e.t <= now
            && e.subject.as_deref().is_some_and(|g| held.contains(&g))
        {
            closed.push(goal(e));
        }
    }
    // what most needs acting on first: a return, a refused land; then what
    // moved; approvals last
    let mut out = Vec::new();
    let verdict = |said: &str| {
        verdicts
            .get(said)
            .map(|by_goal| format!("{said} on {}", counted(by_goal)))
    };
    out.extend(verdict("RETURN"));
    if refused > 0 {
        out.push(format!("land refused ×{refused}"));
    }
    if !lands.is_empty() {
        let n: usize = lands.values().sum();
        out.push(format!("landed {n} ({})", counted(&lands)));
    }
    if !closed.is_empty() && history("close") {
        out.push(format!("closed {}", closed.join(", ")));
    }
    if !claimed.is_empty() && history("claim") {
        out.push(format!("claimed {}", claimed.join(", ")));
    }
    for (said, by_goal) in &verdicts {
        if said != "RETURN" {
            out.push(format!("{said} on {}", counted(by_goal)));
        }
    }
    out
}

/// `{a: 2, b: 1}` → `a ×2, b`.
fn counted(by_goal: &std::collections::BTreeMap<String, usize>) -> String {
    by_goal
        .iter()
        .map(|(g, n)| {
            if *n > 1 {
                format!("{g} ×{n}")
            } else {
                g.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The most the change line injects into a turn.
pub const CHANGES_MAX: usize = 280;

/// Join changes into one line of at most [`CHANGES_MAX`] bytes, ending with
/// how many were left out.
#[must_use]
pub fn capped(changes: &[String]) -> String {
    let mut line = String::new();
    for (i, c) in changes.iter().enumerate() {
        let sep = if line.is_empty() { "" } else { "; " };
        let more = changes.len() - i;
        let tail = format!("; … {more} more");
        if line.len() + sep.len() + c.len() + if more > 1 { tail.len() } else { 0 } > CHANGES_MAX {
            if line.is_empty() {
                // one change alone too long: cut it at a character boundary
                let mut cut = CHANGES_MAX.saturating_sub(tail.len());
                while !c.is_char_boundary(cut) {
                    cut -= 1;
                }
                line.push_str(&c[..cut]);
                line.push('…');
                if more > 1 {
                    let _ = write!(line, "; … {} more", more - 1);
                }
            } else {
                line.push_str(&tail);
            }
            return line;
        }
        line.push_str(sep);
        line.push_str(c);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters;
    use crate::event::attr;

    #[test]
    fn a_long_absence_is_one_bounded_line() {
        let many: Vec<String> = (0..60).map(|i| format!("landed 1 (goal-{i:03})")).collect();
        let line = capped(&many);
        assert!(line.len() <= CHANGES_MAX + 4, "{} bytes", line.len());
        assert!(line.ends_with(" more"), "{line}");
        assert_eq!(capped(&["a".into(), "b".into()]), "a; b");
    }

    #[test]
    fn changes_group_by_kind_and_goal() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("x-afw48".to_owned(), 17);
        m.insert("x-55npf".to_owned(), 1);
        assert_eq!(counted(&m), "x-55npf, x-afw48 ×17");
    }

    fn config() -> Config {
        Config::parse(
            "[goal]\n[seats.andon-1b]\n[seats.andon-1a]\n",
            Path::new("/"),
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn at(s: &str) -> Timestamp {
        parse_instant(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn ledger() -> Ledger {
        let mut claim = Event::new(at("2026-10-13T10:00:00Z"), Kind::Claim, "bd");
        claim.actor = Some("andon-1b".into());
        claim.subject = Some("andon-xa4".into());
        claim.ids.goal = claim.subject.clone();
        let mut create = Event::new(at("2026-10-13T09:00:00Z"), Kind::Create, "bd");
        create.subject = Some("andon-xa4".into());
        create.ids.goal = create.subject.clone();
        create.set_attr(attr::BUDGET, "about 8 agent-hours");
        let mut verdict = Event::new(at("2026-10-13T12:00:00Z"), Kind::Verdict, "bd");
        verdict.actor = Some("andon-1b".into());
        verdict.subject = Some("andon-xa4".into());
        verdict.outcome = Some(Outcome::Return);
        let all = crate::event::coverage(
            crate::event::ALL_HISTORY,
            None,
            &[
                "land",
                "run",
                "verdict",
                "create",
                "claim",
                "close",
                "other:comment",
                "session_active",
            ],
            "test",
            None,
        );
        adapters::resolve(&config(), vec![claim, create, verdict, all])
    }

    #[test]
    fn payloads_name_their_event_cwd_and_transcript() {
        let p = Payload::parse(
            r#"{"hook_event_name":"SessionStart","cwd":"/code/andon-1b","transcript_path":"/t.jsonl"}"#,
        );
        assert_eq!(p.event, "SessionStart");
        assert_eq!(p.cwd.as_deref(), Some(Path::new("/code/andon-1b")));
        assert_eq!(
            Payload::parse("not json").problem.as_deref(),
            Some("the hook payload on stdin is not JSON")
        );
        assert_eq!(
            Payload::parse("").problem.as_deref(),
            Some("no hook payload on stdin")
        );
        assert_eq!(Payload::parse("{}").problem, None);
    }

    #[test]
    fn the_seat_is_the_nearest_workspace_directory() {
        assert_eq!(
            seat_of(Path::new("/code/andon-1b/src/fold"), &config()).as_deref(),
            Some("andon-1b")
        );
        assert_eq!(seat_of(Path::new("/tmp/elsewhere"), &config()), None);
    }

    #[test]
    fn with_no_seats_the_workspace_root_is_the_seat() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let root = dir.path().join("x-1b");
        std::fs::create_dir_all(root.join(".jj")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(root.join("src/fold")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            seat_of(&root.join("src/fold"), &Config::default()).as_deref(),
            Some("x-1b")
        );
    }

    #[test]
    fn session_start_prints_the_row() {
        let out = render(
            &ledger(),
            &config(),
            "andon-1b",
            "SessionStart",
            None,
            at("2026-10-13T13:00:00Z"),
        );
        assert!(out.starts_with("andon: andon-1b  andon-xa4"), "{out}");
    }

    #[test]
    fn a_prompt_after_a_return_names_it_and_is_silent_otherwise() {
        let l = ledger();
        let c = config();
        let now = at("2026-10-13T13:00:00Z");
        let out = render(
            &l,
            &c,
            "andon-1b",
            "UserPromptSubmit",
            Some(at("2026-10-13T11:00:00Z")),
            now,
        );
        assert!(
            out.contains("since your last turn: RETURN on andon-xa4"),
            "{out}"
        );
        let quiet = render(
            &l,
            &c,
            "andon-1b",
            "UserPromptSubmit",
            Some(at("2026-10-13T12:30:00Z")),
            now,
        );
        assert_eq!(quiet, "");
    }

    #[test]
    fn a_prompt_folds_the_ledger_once() {
        // the row now and the goals held at the last turn come off one
        // projection: the hook used to fold the whole board twice per call
        let (l, c) = (ledger(), config());
        let before = crate::fold::projection::folds();
        let out = render(
            &l,
            &c,
            "andon-1b",
            "UserPromptSubmit",
            Some(at("2026-10-13T11:00:00Z")),
            at("2026-10-13T13:00:00Z"),
        );
        assert!(out.contains("since your last turn"), "{out}");
        assert_eq!(crate::fold::projection::folds() - before, 1);
    }

    #[test]
    fn an_overlap_is_news_only_when_a_shared_file_changed_since_the_last_turn() {
        let now = at("2026-10-13T12:00:00Z");
        let unlanded = |t: &str, seat: &str, path: &str| {
            let mut e = Event::new(at(t), Kind::Unlanded, "jj");
            e.t_end = Some(now);
            e.actor = Some(seat.into());
            e.set_attr(attr::PATH, path);
            e
        };
        let all = crate::event::coverage(
            crate::event::ALL_HISTORY,
            None,
            &["land", "verdict", "commit", "gate_run", "claim", "close"],
            "test",
            None,
        );
        let read = crate::event::coverage(now, Some(now), &["state:unlanded"], "jj", None);
        let ledger = |events: Vec<Event>| {
            adapters::resolve(
                &config(),
                events
                    .into_iter()
                    .chain([all.clone(), read.clone()])
                    .collect(),
            )
        };
        let last_turn = Some(at("2026-10-13T11:00:00Z"));
        let said =
            |l: &Ledger| render(l, &config(), "andon-1b", "UserPromptSubmit", last_turn, now);
        // andon-1a changed render.rs after andon-1b's last turn: news
        let l = ledger(vec![
            unlanded("2026-10-13T10:00:00Z", "andon-1b", "src/render.rs"),
            unlanded("2026-10-13T11:30:00Z", "andon-1a", "src/render.rs"),
        ]);
        let out = said(&l);
        assert!(
            out.contains("since your last turn: overlaps andon-1a on 1 file (src/render.rs)"),
            "{out}"
        );
        // both sides held it unchanged since before the last turn: not news
        let l = ledger(vec![
            unlanded("2026-10-13T10:00:00Z", "andon-1b", "src/render.rs"),
            unlanded("2026-10-13T10:30:00Z", "andon-1a", "src/render.rs"),
        ]);
        assert_eq!(said(&l), "");
    }
}
