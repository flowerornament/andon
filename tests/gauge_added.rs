//! Adding a gauge is two declarations: a [`Spec`] beside the fold that
//! computes it (its output path, the kinds it reads, its span) and a
//! [`Column`] beside the others (its header, its `--help` line, its cell).
//! Everything else follows from those two: whether it is known, the `?` and
//! its reason, the help line, and the coverage property, which reads every
//! spec from `gauge::all()`. This test adds a gauge of its own, nowhere but
//! here, and shows each of those following.

use andon::event::{ALL_HISTORY, Event, Kind, coverage};
use andon::fold::coverage::Coverage;
use andon::fold::gauge::{Gauge, Gauger, Scope, Span, Spec};
use andon::ledger::Ledger;
use andon::render::{Column, grid, help, row_unknowns};
use andon::time::parse_instant;
use serde::Serialize;

/// The test's own gauge: subagents a seat started in the window.
const SPAWNS: Spec = Spec::new(Scope::Row, "spawns", &["spawn"], Span::Window);

#[derive(Serialize)]
struct Seat {
    #[serde(skip)]
    seat: String,
    spawns: Gauge<usize>,
}

const COLUMNS: &[Column<Seat>] = &[
    Column {
        name: "seat",
        help: "the seat.",
        cell: |s, _| s.seat.clone(),
    },
    Column {
        name: "spawns",
        help: "subagents the seat started in the window.",
        cell: |s, _| s.spawns.cell(ToString::to_string),
    },
];

fn seat(ledger: &Ledger) -> Seat {
    let at = |s: &str| parse_instant(s).unwrap_or_else(|e| panic!("{e}"));
    let coverage = Coverage::new(ledger);
    let g = Gauger {
        coverage: &coverage,
        from: at("2026-10-13T00:00:00Z"),
        to: at("2026-10-14T00:00:00Z"),
    };
    let n = ledger
        .events()
        .iter()
        .filter(|e| e.kind == Kind::Spawn)
        .count();
    Seat {
        seat: "x-1a".to_owned(),
        spawns: g.gauge(&SPAWNS, Some("x-1a"), None, n),
    }
}

#[test]
fn a_gauge_is_a_spec_and_a_column() {
    let t = parse_instant("2026-10-13T10:00:00Z").unwrap_or_else(|e| panic!("{e}"));
    let spawn = Event::new(t, Kind::Spawn, "transcript");
    let now = parse_instant("2026-10-14T00:00:00Z").unwrap_or_else(|e| panic!("{e}"));

    // covered: the value
    let read = Ledger::new(vec![
        spawn.clone(),
        coverage(ALL_HISTORY, None, &["spawn"], "transcripts", None),
    ]);
    let rows = [seat(&read)];
    assert_eq!(grid(COLUMNS, &rows, now), "seat  spawns\nx-1a  1\n");
    assert_eq!(row_unknowns(&rows, |s| s.seat.as_str()), Vec::new());

    // not covered: `?`, never 0, and the reason, with no code but the two
    // declarations above
    let unread = Ledger::new(vec![spawn]);
    let rows = [seat(&unread)];
    assert_eq!(grid(COLUMNS, &rows, now), "seat  spawns\nx-1a  ?\n");
    assert_eq!(
        row_unknowns(&rows, |s| s.seat.as_str()),
        vec![("x-1a spawns".to_owned(), "no source reads spawn".to_owned())]
    );

    // the help line, and the output path the spec declares
    assert_eq!(
        help(COLUMNS),
        "  spawns: subagents the seat started in the window.\n"
    );
    assert!(SPAWNS.at("seats.0.spawns"));
}
