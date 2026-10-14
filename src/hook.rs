//! `andon hook`: the seat's row, injected into the agent's turn (founding
//! design §6). The harness pipes its hook JSON on stdin and puts stdout into
//! the agent's context, so stdout carries only the row and its changes.
//!
//! - `SessionStart`: the seat's row.
//! - `UserPromptSubmit`: what changed for the seat since its last turn
//!   (a verdict, a claim or close, a landing, a refused land, appetite spent),
//!   then the row. Silent when nothing changed.
//!
//! The last turn is read from the transcript the payload names: the time of
//! its last assistant message. No state is kept between calls.
//!
//! A hook must never break a session: with no andon.toml, or on any error,
//! it prints nothing to stdout, explains on stderr, and exits 0.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde_json::Value;

use crate::config::Config;
use crate::event::{Event, Kind, Outcome};
use crate::fold::board::{self, Row};
use crate::fold::coverage::Coverage;
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

/// The hook's output for a seat at `now`. Pure: everything it reads is an
/// argument.
#[must_use]
pub fn render(
    ledger: &Ledger,
    config: &Config,
    seat: &str,
    event: &str,
    since: Option<Timestamp>,
    now: Timestamp,
) -> String {
    let board = board::board(ledger, config, now);
    let Some(row) = board.seats.iter().find(|r| r.seat == seat) else {
        return String::new();
    };
    if event == "SessionStart" {
        return format!("andon: {}\n", line(row, now));
    }
    let Some(since) = since else {
        return String::new();
    };
    // claims and closes from a current-state source (a tracker export) are
    // not changes since the last turn: a claim released meanwhile is not in
    // it. They are listed only where some source reads them as history
    let coverage = Coverage::new(ledger);
    let mut changes = changes(ledger, seat, since, now, &|k| coverage.reads(k));
    let before = board::board(ledger, config, since);
    let held = |r: &Row| r.goals.get().cloned().unwrap_or_default();
    if let Some(prev) = before.seats.iter().find(|r| r.seat == seat) {
        let was = held(prev);
        let over =
            |g: &board::Goal| matches!((g.used.get(), g.budget), (Some(u), Some(b)) if *u > b);
        for goal in held(row) {
            if over(&goal) && !was.iter().find(|g| g.id == goal.id).is_some_and(over) {
                changes.push(format!("appetite spent on {}", goal.id));
            }
        }
    }
    let unread = unread(ledger, seat, since, now);
    if changes.is_empty() && unread.is_empty() {
        return String::new();
    }
    let mut out = format!("andon: {}\n", line(row, now));
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
fn unread(ledger: &Ledger, seat: &str, since: Timestamp, now: Timestamp) -> Vec<&'static str> {
    let coverage = Coverage::new(ledger);
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
        .filter(|(k, _)| k.iter().all(|k| coverage.reads(k)))
        .filter(|(k, _)| since < now && coverage.check(k, Some(seat), since, now).is_some())
        .map(|(_, noun)| noun)
        .collect()
}

/// The row on one line, for injection.
#[must_use]
pub fn line(row: &Row, now: Timestamp) -> String {
    let cells = render::board_row(row, now);
    let header = render::board_header();
    let mut parts = vec![row.seat.clone()];
    for (name, cell) in header.iter().zip(cells.iter()).skip(1) {
        if cell.is_empty() || cell == "—" {
            continue;
        }
        if name == "flags" {
            parts.push(format!("⚑ {cell}"));
        } else if name == "goal" {
            parts.push(cell.clone());
        } else {
            parts.push(format!("{name} {cell}"));
        }
    }
    parts.join("  ")
}

/// Facts about the seat recorded after its last turn.
fn changes(
    ledger: &Ledger,
    seat: &str,
    since: Timestamp,
    now: Timestamp,
    history: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    use std::collections::BTreeMap;
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
            Kind::GateRun
                if e.attr_str("recipe") == Some("land")
                    && matches!(e.outcome, Some(Outcome::Fail | Outcome::Refused)) =>
            {
                refused += 1;
            }
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
        Config::parse("[seats.andon-1b]\n[seats.andon-1a]\n", Path::new("/"))
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
        create.set_attr("appetite", "about 8 agent-hours");
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
}
