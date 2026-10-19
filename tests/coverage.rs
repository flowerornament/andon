//! Coverage as a rule (`.design/2026-10-14-coverage.md`): for every kind a
//! gauge reads, knocking that kind's coverage out turns exactly the gauges
//! that read it unknown, with a reason, and leaves the others known. A gauge
//! is never 0 where the ledger could not look.

#![allow(
    clippy::disallowed_methods,
    reason = "fixtures build the repositories and files andon reads"
)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use andon::adapters;
use andon::config::Config;
use andon::event::attr;
use andon::event::{ALL_HISTORY, Event, Kind, Outcome, coverage, gap};
use andon::fold::board::board;
use andon::fold::gauge::{Spec, spec_at};
use andon::fold::scorecard::scorecard;
use andon::ledger::Ledger;
use andon::time::{Window, parse_instant};
use jiff::Timestamp;

fn at(s: &str) -> Timestamp {
    parse_instant(s).unwrap_or_else(|e| panic!("{e}"))
}

fn config() -> Config {
    common::config("[seats.x-1a]\n[seats.x-1b]\n", Path::new("/"))
}

fn fact(t: &str, kind: Kind, goal: Option<&str>) -> Event {
    let mut e = Event::new(at(t), kind, "fixture");
    e.actor = Some("x-1a".into());
    e.subject = goal.map(str::to_owned);
    e.ids.goal = e.subject.clone();
    e
}

const KINDS: &[&str] = &[
    "land",
    "commit",
    "run",
    "gate_run",
    "verdict",
    "message",
    "create",
    "claim",
    "close",
    "other:comment",
    "session_active",
];

/// What is wrong with one kind's source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fault {
    /// No source reads it.
    Absent,
    /// Its source reads only from after the window.
    Horizon,
    /// Its source names a path it could not read.
    Gap,
}

/// A day of x-1a's work, every kind of fact, and coverage of every kind
/// except `without`.
fn ledger(without: Option<&str>) -> Ledger {
    faulty(without.map(|k| (k, Fault::Absent)))
}

/// The same day, each kind read by its own source, one of them at fault.
fn faulty(fault: Option<(&str, Fault)>) -> Ledger {
    day(fault, None)
}

/// The same day, with one source at fault and one kind's facts left out
/// (its source still covering it).
fn day(fault: Option<(&str, Fault)>, drop: Option<&str>) -> Ledger {
    let mut land = fact("2026-10-13T12:00:00Z", Kind::Land, Some("x-goal"));
    land.ids.commit = Some("c1".into());
    // it cites its review and names its session, so the flow and the joins
    // have something to join
    land.ids.cites = vec!["v1".into()];
    land.ids.session = Some("s1".into());
    let mut start = fact("2026-10-13T11:00:00Z", Kind::Commit, Some("x-goal"));
    start.ids.commit = Some("c1".into());
    let mut run = fact("2026-10-13T11:30:00Z", Kind::Run, None);
    run.outcome = Some(Outcome::Fail);
    run.set_attr(attr::REPO, "x");
    let mut gate = fact("2026-10-13T11:50:00Z", Kind::GateRun, None);
    gate.t_end = Some(at("2026-10-13T11:55:00Z"));
    gate.outcome = Some(Outcome::Pass);
    gate.set_attr(attr::RECIPE, "land");
    let mut verdict = fact("2026-10-13T11:40:00Z", Kind::Verdict, Some("x-goal"));
    verdict.outcome = Some(Outcome::Go);
    verdict.ids.verdict = Some("v1".into());
    let mut message = fact("2026-10-13T10:00:00Z", Kind::Message, Some("x-goal"));
    message.set_attr(attr::MESSAGE_KIND, "ask");
    message.set_attr(attr::TO, vec!["x-1b".to_owned()]);
    let mut create = fact("2026-10-13T08:00:00Z", Kind::Create, Some("x-goal"));
    create.set_attr(attr::BUDGET, "4 agent-hours");
    let claim = fact("2026-10-13T09:00:00Z", Kind::Claim, Some("x-goal"));
    let comment = fact(
        "2026-10-13T09:30:00Z",
        Kind::Other("other:comment".into()),
        Some("x-goal"),
    );
    // x-1b landed and was reviewed too, so a fully covered day leaves no
    // seat's last landing or verdict unknown
    let mut land_b = fact("2026-10-13T13:00:00Z", Kind::Land, None);
    land_b.actor = Some("x-1b".into());
    land_b.ids.commit = Some("c2".into());
    let mut verdict_b = fact("2026-10-13T12:30:00Z", Kind::Verdict, None);
    verdict_b.actor = Some("x-1b".into());
    verdict_b.outcome = Some(Outcome::Go);
    let mut span = fact("2026-10-13T10:00:00Z", Kind::SessionActive, None);
    span.t_end = Some(at("2026-10-13T11:00:00Z"));
    span.ids.session = Some("s1".into());
    let mut sources = Vec::new();
    for kind in KINDS.iter().copied() {
        let tracker = ["create", "claim", "close"].contains(&kind);
        let from = if tracker {
            ALL_HISTORY
        } else {
            at("2026-10-01T00:00:00Z")
        };
        match fault.filter(|(k, _)| *k == kind).map(|(_, f)| f) {
            None => sources.push(coverage(from, None, &[kind], kind, None)),
            Some(Fault::Absent) => {}
            Some(Fault::Horizon) => {
                sources.push(coverage(
                    at("2026-10-14T00:00:00Z"),
                    None,
                    &[kind],
                    kind,
                    None,
                ));
            }
            Some(Fault::Gap) => {
                sources.push(coverage(from, None, &[kind], kind, None));
                sources.push(gap(at(NOON), &[kind], kind, None, "unreadable"));
            }
        }
    }
    adapters::resolve(
        &config(),
        vec![
            land, start, run, gate, verdict, message, create, claim, comment, span, land_b,
            verdict_b,
        ]
        .into_iter()
        .filter(|e| drop.is_none_or(|k| e.kind.as_str() != k))
        .chain(sources)
        .collect(),
    )
}

const DAY: &str = "2026-10-13";
const NOON: &str = "2026-10-13T23:00:00Z";

fn card(l: &Ledger) -> andon::fold::scorecard::Scorecard {
    let window = Window::parse(DAY, at(NOON)).unwrap_or_else(|e| panic!("{e}"));
    scorecard(l, &config(), window)
}

/// The scorecard gauges each kind's knockout must turn unknown (seat card
/// for x-1a, or scorecard-wide).
fn expect(kind: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match kind {
        "land" => (
            &["landings", "runs_per_landing"],
            &[
                "flow.lead_median_min",
                "flow.review_median_min",
                "joins.landings",
                "joins.cites",
            ],
        ),
        "commit" => (
            &["verdicts_go", "verdicts_return"],
            &["flow.lead_median_min"],
        ),
        "run" => (
            &["runs", "runs_per_landing"],
            &["joins.runs", "runs_by_repo"],
        ),
        "gate_run" => (&["gate_runs"], &["flow.gate_median_min"]),
        "verdict" => (
            &["verdicts_go", "verdicts_return"],
            &["flow.review_median_min", "joins.cites_resolved"],
        ),
        "message" => (
            &[
                "messages_sent",
                "messages_received",
                "messages_naming_goal",
                "decisions_without_goal",
            ],
            &["hub"],
        ),
        "create" => (
            &["items_created", "wip_at_end"],
            &["tracker.created", "tracker.open_at_end"],
        ),
        "claim" => (
            &["wip_at_end"],
            &["tracker.open_at_end", "tracker.unowned_at_end"],
        ),
        "close" => (&["wip_at_end"], &["tracker.closed", "tracker.branching"]),
        "session_active" => (&["active_hours"], &["joins.landing_session_seen"]),
        _ => (&[], &[]),
    }
}

/// Every unknown gauge in an output, by its JSON path.
fn unknown<T: serde::Serialize>(out: &T) -> BTreeMap<String, String> {
    let v = serde_json::to_value(out).unwrap_or_else(|e| panic!("{e}"));
    andon::fold::gauge::unknowns(&v).into_iter().collect()
}

#[test]
fn every_gauge_is_known_where_everything_is_covered() {
    let c = card(&ledger(None));
    assert!(unknown(&c).is_empty(), "{:?}", unknown(&c));
    assert_eq!(c.seats["x-1a"].landings, 1);
}

#[test]
fn knocking_out_one_kind_turns_exactly_its_gauges_unknown() {
    for kind in KINDS.iter().copied().filter(|k| *k != "other:comment") {
        let c = card(&ledger(Some(kind)));
        let (on_card, wide) = expect(kind);
        let gone = unknown(&c);
        for gauge in on_card {
            let why = gone
                .get(&format!("seats.x-1a.{gauge}"))
                .unwrap_or_else(|| panic!("{kind}: {gauge} stayed known"));
            assert!(why.starts_with("no source reads"), "{kind}: {gauge}: {why}");
        }
        for gauge in wide {
            assert!(
                gone.contains_key(*gauge),
                "{kind}: {gauge} stayed known: {gone:?}"
            );
        }
        // and only those: every other gauge of x-1a's card stays known
        for path in gone.keys() {
            if let Some(gauge) = path.strip_prefix("seats.x-1a.") {
                assert!(on_card.contains(&gauge), "{kind}: {gauge} went unknown too");
            }
        }
        // the text cell is `?`, never the uncovered value
        let text = andon::render::scorecard(&c);
        let row = text
            .lines()
            .find(|l| l.starts_with("x-1a "))
            .unwrap_or_else(|| panic!("{text}"));
        // (gate runs have no column: the flow line's gate median shows them)
        if on_card.iter().any(|g| *g != "gate_runs") {
            assert!(row.split_whitespace().any(|c| c == "?"), "{kind}: {row}");
        }
    }
}

#[test]
fn a_window_before_coverage_is_unknown_with_the_horizon_named() {
    // the ledger covers from 2026-10-01: a window in September cannot say 0
    let window =
        Window::parse("2026-09-20..2026-09-25", at(NOON)).unwrap_or_else(|e| panic!("{e}"));
    let c = scorecard(&ledger(None), &config(), window);
    assert_eq!(
        c.seats["x-1a"].landings.reason(),
        Some("landings are read from 2026-10-01 00:00 (land)")
    );
    // the cell itself, not only the reasons line (C9: CARD_CELL)
    let text = andon::render::scorecard(&c);
    let row: Vec<&str> = text
        .lines()
        .find(|l| l.starts_with("x-1a "))
        .unwrap_or_else(|| panic!("{text}"))
        .split_whitespace()
        .collect();
    assert_eq!(row[1], "?", "lands: {row:?}");
    assert!(text.contains("? landings"), "{text}");
}

#[test]
fn asof_before_coverage_claims_no_absence() {
    // R3-1: before the horizon, no "no landing" and no 0 landings
    let b = board(&ledger(None), &config(), at("2026-09-25T00:00:00Z"));
    let row = b
        .seats
        .iter()
        .find(|r| r.seat == "x-1a")
        .unwrap_or_else(|| panic!("row"));
    assert!(
        row.flags.iter().all(|f| !f.contains("no landing")),
        "{:?}",
        row.flags
    );
    assert!(!row.landings.is_known() && !row.runs.is_known());
    // and the text cell prints `?`, not the uncounted 0 (round five: F4)
    let text = andon::render::board(&b);
    let cells: Vec<&str> = text
        .lines()
        .find(|l| l.starts_with("x-1a "))
        .unwrap_or_else(|| panic!("{text}"))
        .split_whitespace()
        .collect();
    let header: Vec<String> = andon::render::board_header();
    let lands = header
        .iter()
        .position(|h| h == "lands")
        .unwrap_or_else(|| panic!("lands"));
    assert_eq!(cells[lands], "?", "{cells:?}");
}

#[test]
fn a_gap_makes_its_kind_unknown_for_every_seat() {
    let mut events: Vec<Event> = ledger(None).events().to_vec();
    events.push(gap(
        at(NOON),
        &["session_active"],
        "t.jsonl",
        None,
        "t.jsonl: permission denied",
    ));
    let l = adapters::resolve(&config(), events);
    let c = card(&l);
    for seat in ["x-1a", "x-1b"] {
        assert_eq!(
            c.seats[seat].active_hours.reason(),
            Some("1 sessions source could not be read: t.jsonl: permission denied")
        );
    }
    // the board's budget spent reads sessions too, in JSON as in text (C8)
    let b = board(&l, &config(), at(NOON));
    let gone = unknown(&b);
    let i = b
        .seats
        .iter()
        .position(|r| r.seat == "x-1a")
        .unwrap_or_else(|| panic!("row"));
    assert!(
        gone.contains_key(&format!("seats.{i}.goals.0.used")),
        "{gone:?}"
    );
}

#[test]
fn an_unreadable_transcript_is_a_gap_not_zero_hours() {
    // R3-2, through the sessions adapter: a transcript that is not JSON
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let projects = dir.path().join("projects/-x-1a");
    std::fs::create_dir_all(&projects).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(projects.join("ok.jsonl"), concat!(
        r#"{"type":"assistant","timestamp":"2026-10-13T10:00:00Z","sessionId":"s","cwd":"/x-1a","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
        "\n"
    ))
    .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(projects.join("bad.jsonl"), b"\x00\x01 not a transcript\n")
        .unwrap_or_else(|e| panic!("{e}"));
    let config = common::config(
        &format!(
            "[seats.x-1a]\ncwd = [\"/x-1a\"]\n[adapters.sessions]\nclaude = [\"{}/projects/*\"]\n",
            dir.path().display()
        ),
        dir.path(),
    );
    let l = adapters::collect(&config).unwrap_or_else(|e| panic!("{e:#}"));
    let window = Window::parse(DAY, at(NOON)).unwrap_or_else(|e| panic!("{e}"));
    let c = scorecard(&l, &config, window);
    let why = c.seats["x-1a"]
        .active_hours
        .reason()
        .unwrap_or_else(|| panic!("active hours known"));
    assert!(
        why.contains("bad.jsonl: its first line is not JSON"),
        "{why}"
    );
}

/// Finding F (andon-3eu): a transcript's turns all precede its last write,
/// so one unreadable transcript stands over spans up to then, and a window
/// after it is still known.
#[test]
fn an_unreadable_transcript_stands_only_until_its_last_write() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let projects = dir.path().join("projects/-x-1a");
    std::fs::create_dir_all(&projects).unwrap_or_else(|e| panic!("{e}"));
    let bad = projects.join("bad.jsonl");
    std::fs::write(&bad, b"\x00\x01 not a transcript\n").unwrap_or_else(|e| panic!("{e}"));
    // last written two days before the window
    let touched = std::process::Command::new("touch")
        .args(["-t", "202610110000"])
        .arg(&bad)
        .status()
        .is_ok_and(|s| s.success());
    assert!(touched, "touch -t failed");
    let config = common::config(
        &format!(
            "[seats.x-1a]\ncwd = [\"/x-1a\"]\n[adapters.sessions]\nclaude = [\"{}/projects/*\"]\n",
            dir.path().display()
        ),
        dir.path(),
    );
    let l = adapters::collect(&config).unwrap_or_else(|e| panic!("{e:#}"));
    let hours = |window: &str| {
        let w = Window::parse(window, at(NOON)).unwrap_or_else(|e| panic!("{e}"));
        scorecard(&l, &config, w).seats["x-1a"].active_hours.clone()
    };
    assert_eq!(hours(DAY), 0.0, "{:?}", hours(DAY).reason());
    assert!(
        hours("2026-10-10")
            .reason()
            .is_some_and(|w| w.contains("bad.jsonl"))
    );
}

/// The gauge a JSON path is an instance of (its declared path is the output
/// path), and whether it is a single seat's.
fn spec_of(board: bool, path: &str) -> Option<(&'static Spec, bool)> {
    let seat = path.starts_with("seats.") || path.starts_with("unattributed.");
    spec_at(board, path).map(|s| (s, seat))
}

/// Whether the gauge at `path`, or any gauge holding it, is unknown.
fn unknown_at(v: &serde_json::Value, path: &str) -> bool {
    let mut node = v;
    for key in path.split('.') {
        if node
            .get("unknown")
            .is_some_and(serde_json::Value::is_string)
        {
            return true;
        }
        node = match node {
            serde_json::Value::Array(items) => {
                match key.parse::<usize>().ok().and_then(|i| items.get(i)) {
                    Some(n) => n,
                    None => return false,
                }
            }
            _ => match node.get(key) {
                Some(n) => n,
                None => return false,
            },
        };
    }
    node.get("unknown")
        .is_some_and(serde_json::Value::is_string)
}

/// The property (round four): for every source a gauge depends on, moving
/// that source's horizon past the window, or making it unreadable, turns
/// every gauge that reads it unknown, in the scorecard and on the board; and
/// every gauge either can print is registered. A new gauge is covered by
/// construction: it is unknown wherever it is not registered, and here
/// wherever its registered kinds are faulted.
#[test]
fn every_gauge_turns_unknown_with_any_source_it_reads() {
    let outputs = |l: &Ledger| {
        (
            serde_json::to_value(card(l)).unwrap_or_default(),
            serde_json::to_value(board(l, &config(), at(NOON))).unwrap_or_default(),
        )
    };
    let mut cases = vec![(None, faulty(None))];
    for kind in KINDS {
        for f in [Fault::Absent, Fault::Horizon, Fault::Gap] {
            cases.push((Some((*kind, f)), faulty(Some((*kind, f)))));
        }
    }
    let outs: Vec<_> = cases.iter().map(|(f, l)| (*f, outputs(l))).collect();
    // every cell either output prints, from the registry (a census of the
    // fully covered outputs), not from the gauges seen going unknown
    let instances = census_of(&outs[0].1);
    assert!(instances.len() > 40, "{}", instances.len());
    for (board, path) in &instances {
        assert!(
            spec_of(*board, path).is_some(),
            "{path} is not a registered gauge"
        );
    }
    for (fault, (c, b)) in &outs {
        let Some((kind, f)) = fault else {
            assert_eq!(andon::fold::gauge::unknowns(c), vec![]);
            assert!(
                andon::fold::gauge::unknowns(b).is_empty(),
                "{:?}",
                andon::fold::gauge::unknowns(b)
            );
            continue;
        };
        for (board, path) in &instances {
            let Some((spec, seat)) = spec_of(*board, path) else {
                continue;
            };
            let reads = spec.reads(kind, seat);
            if reads {
                let out = if *board { b } else { c };
                assert!(unknown_at(out, path), "{kind} {f:?}: {path} stayed known");
            }
        }
    }
}

/// The value at a JSON path, if the output has it.
fn value_at<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    path.split('.').try_fold(v, |node, key| match node {
        serde_json::Value::Array(items) => items.get(key.parse::<usize>().ok()?),
        _ => node.get(key),
    })
}

/// The other half, from the folds rather than the registry: leave one kind's
/// facts out (its source still covering it) and every gauge whose value moves
/// depends on that kind, so its registration must name it. A gauge that
/// under-declares what it reads is caught here.
#[test]
fn every_gauge_registers_each_kind_its_value_depends_on() {
    let outputs = |l: &Ledger| {
        (
            serde_json::to_value(card(l)).unwrap_or_default(),
            serde_json::to_value(board(l, &config(), at(NOON))).unwrap_or_default(),
        )
    };
    let instances = census_of(&outputs(&day(None, None)));
    let (c0, b0) = outputs(&day(None, None));
    for kind in KINDS {
        let (c1, b1) = outputs(&day(None, Some(kind)));
        for (board, path) in &instances {
            let (before, after) = if *board { (&b0, &b1) } else { (&c0, &c1) };
            let (Some(x), Some(y)) = (value_at(before, path), value_at(after, path)) else {
                continue;
            };
            // a gauge holding gauges (goals, each with its budget spent) is
            // compared without them: each nested gauge is checked as its own
            let (mut x, mut y) = (x.clone(), y.clone());
            for (_, inner) in instances.iter().filter(|(b, _)| b == board) {
                if let Some(rest) = inner.strip_prefix(&format!("{path}.")) {
                    blank(&mut x, rest);
                    blank(&mut y, rest);
                }
            }
            if x == y {
                continue;
            }
            let (spec, seat) = spec_of(*board, path).unwrap_or_else(|| panic!("{path}"));
            let reads = spec.reads(kind, seat);
            assert!(
                reads,
                "{path} moves without {kind} facts ({x} -> {y}), but {} does not register {kind}",
                spec.field
            );
        }
    }
}

/// Set the value at a relative JSON path to null, where it exists.
fn blank(v: &mut serde_json::Value, path: &str) {
    let mut node = v;
    for key in path.split('.') {
        let next = match node {
            serde_json::Value::Array(items) => {
                key.parse::<usize>().ok().and_then(|i| items.get_mut(i))
            }
            serde_json::Value::Object(map) => map.get_mut(key),
            _ => None,
        };
        let Some(n) = next else { return };
        node = n;
    }
    *node = serde_json::Value::Null;
}

/// Output leaves that are not gauges: they read no kind, and say so here.
/// Anything else an output prints must lie inside a registered gauge.
const METADATA: &[(bool, &str)] = &[
    (false, "schema"),
    (false, "from"),
    (false, "to"),
    (false, "asked_to"),
    (false, "days"),
    (true, "schema"),
    (true, "asof"),
    (true, "window_hours"),
    (true, "seats.*.seat"),
    // flags read only known gauges (`board::flags`)
    (true, "seats.*.flags"),
];

fn metadata(board: bool, path: &str) -> bool {
    METADATA.iter().any(|(b, pattern)| {
        *b == board
            && pattern.split('.').count() == path.split('.').count()
            && pattern
                .split('.')
                .zip(path.split('.'))
                .all(|(p, k)| p == "*" || p == k)
    })
}

/// Every gauge instance in an output, and every leaf that is neither inside
/// one nor declared metadata.
fn census(board: bool, v: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    fn walk(
        board: bool,
        v: &serde_json::Value,
        path: &str,
        inside: bool,
        out: &mut (Vec<String>, Vec<String>),
    ) {
        if !path.is_empty() && metadata(board, path) {
            return;
        }
        let inside = if !path.is_empty() && spec_of(board, path).is_some() {
            out.0.push(path.to_owned());
            true
        } else {
            inside
        };
        let join = |k: &str| {
            if path.is_empty() {
                k.to_owned()
            } else {
                format!("{path}.{k}")
            }
        };
        match v {
            serde_json::Value::Object(map) => {
                for (k, x) in map {
                    walk(board, x, &join(k), inside, out);
                }
            }
            serde_json::Value::Array(items) => {
                for (i, x) in items.iter().enumerate() {
                    walk(board, x, &join(&i.to_string()), inside, out);
                }
            }
            _ if !inside => out.1.push(path.to_owned()),
            _ => {}
        }
    }
    let mut out = (Vec::new(), Vec::new());
    walk(board, v, "", false, &mut out);
    out
}

/// The census of a (scorecard, board) pair: every instance; panics on a
/// leaf that is neither a gauge nor declared metadata.
fn census_of((c, b): &(serde_json::Value, serde_json::Value)) -> Vec<(bool, String)> {
    let mut instances = Vec::new();
    for (board, v) in [(false, c), (true, b)] {
        let (found, stray) = census(board, v);
        assert!(
            stray.is_empty(),
            "printed but neither a registered gauge nor metadata: {stray:?}"
        );
        instances.extend(found.into_iter().map(|p| (board, p)));
    }
    assert!(instances.len() > 40, "{}", instances.len());
    instances
}

/// A bare field (D3: `spawns: usize`) or an unregistered gauge is a stray.
#[test]
fn every_printed_cell_is_a_registered_gauge_or_declared_metadata() {
    let card = serde_json::json!({"schema": 2, "seats": {"x-1a": {"landings": 1, "spawns": 0}}});
    let (found, stray) = census(false, &card);
    assert_eq!(found, vec!["seats.x-1a.landings"]);
    assert_eq!(stray, vec!["seats.x-1a.spawns"]);
}

/// The text census (round six: E1). With no coverage at all, every data cell
/// the scorecard and board print as text is `?`. A column computed outside
/// the gauges (a field kept out of JSON and printed only in text) prints a
/// value here and fails.
#[test]
fn with_nothing_covered_every_text_cell_is_unknown() {
    let bare: Vec<Event> = ledger(None)
        .events()
        .iter()
        .filter(|e| e.kind != Kind::Coverage)
        .cloned()
        .collect();
    let l = adapters::resolve(&config(), bare);
    let text = andon::render::scorecard(&card(&l));
    let rows: Vec<&str> = text
        .lines()
        .filter(|l| {
            ["x-1a ", "x-1b ", "(unattributed) ", "total "]
                .iter()
                .any(|p| l.starts_with(p))
        })
        .collect();
    assert_eq!(rows.len(), 4, "{text}");
    for row in rows {
        let cells: Vec<&str> = row.split_whitespace().skip(1).collect();
        assert!(cells.iter().all(|c| *c == "?"), "{row}");
    }
    for line in text.lines().filter(|l| {
        ["flow ", "tracker ", "joins "]
            .iter()
            .any(|p| l.starts_with(p))
    }) {
        assert!(!line.chars().any(|c| c.is_ascii_digit()), "{line}");
    }
    let b = board(&l, &config(), at(NOON));
    for row in &b.seats {
        let cells = andon::render::board_row(row, b.asof);
        let (seat, rest) = cells.split_first().unwrap_or_else(|| panic!("cells"));
        let (flags, data) = rest.split_last().unwrap_or_else(|| panic!("cells"));
        assert!(data.iter().all(|c| c == "?"), "{seat}: {data:?}");
        assert!(flags.is_empty(), "{seat}: {flags}");
    }
}

/// Item 4 (andon-e10): a window that ends after the capture instant counts
/// up to it and says so, rather than turning every gauge unknown.
#[test]
fn a_window_ending_in_the_future_counts_to_the_capture() {
    let window =
        Window::parse("2026-10-13..2026-10-14", at(NOON)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(window.to, at(NOON));
    // sources read to the capture instant, as real adapters do
    let bounded: Vec<Event> = ledger(None)
        .events()
        .iter()
        .cloned()
        .map(|mut e| {
            if e.kind == Kind::Coverage {
                e.t_end = Some(at(NOON));
            }
            e
        })
        .collect();
    let c = scorecard(&adapters::resolve(&config(), bounded), &config(), window);
    assert_eq!(c.seats["x-1a"].landings, 1);
    let text = andon::render::scorecard(&c);
    assert!(
        text.contains("counted to 2026-10-13 23:00; the window ends in the future"),
        "{text}"
    );
}
