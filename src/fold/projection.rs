//! The per-seat projection: one fold of a ledger, read by the board, the
//! hook and the scorecard. It holds what each surface used to derive its own
//! way: the shared lookups, the coverage, each seat's events, the goals a
//! seat holds, session hours over a span, and which gate runs are the land
//! verb's refusals.

use std::cell::Cell;
use std::collections::BTreeMap;

use jiff::Timestamp;

use crate::config::Config;
use crate::event::attr;
use crate::event::{Event, Kind, Outcome};
use crate::fold::coverage::Coverage;
use crate::fold::{Index, Item};
use crate::ledger::Ledger;
use crate::time::minutes;

thread_local! {
    static FOLDS: Cell<usize> = const { Cell::new(0) };
}

/// How many projections this thread has folded: a surface that folds the
/// ledger once is tested by it.
#[must_use]
pub fn folds() -> usize {
    FOLDS.with(Cell::get)
}

/// One fold of one ledger under one config.
pub struct Projection<'a> {
    pub ledger: &'a Ledger,
    pub config: &'a Config,
    pub index: Index<'a>,
    pub coverage: Coverage,
    by_seat: BTreeMap<&'a str, Vec<&'a Event>>,
}

impl<'a> Projection<'a> {
    #[must_use]
    pub fn new(ledger: &'a Ledger, config: &'a Config) -> Self {
        FOLDS.with(|n| n.set(n.get() + 1));
        let mut by_seat: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
        for event in ledger.events() {
            if let Some(seat) = event.seat.as_deref() {
                by_seat.entry(seat).or_default().push(event);
            }
        }
        Self {
            ledger,
            config,
            index: Index::new(ledger),
            coverage: Coverage::new(ledger),
            by_seat,
        }
    }

    /// The seat's events, in ledger order.
    #[must_use]
    pub fn events(&self, seat: &str) -> &[&'a Event] {
        self.by_seat.get(seat).map_or(&[], Vec::as_slice)
    }

    /// Every seat with an event, and its events.
    pub fn seats(&self) -> impl Iterator<Item = (&'a str, &[&'a Event])> {
        self.by_seat.iter().map(|(s, e)| (*s, e.as_slice()))
    }

    /// Whether `event` is the land verb refusing: a gate run of the
    /// configured land recipe that failed or was refused.
    #[must_use]
    pub fn refused_land(&self, event: &Event) -> bool {
        event.kind == Kind::GateRun
            && self.is_land_gate(event)
            && matches!(event.outcome, Some(Outcome::Fail | Outcome::Refused))
    }

    /// Whether `event` ran the configured land recipe.
    #[must_use]
    pub fn is_land_gate(&self, event: &Event) -> bool {
        event.attr_str(attr::RECIPE) == Some(self.config.project.land_recipe.as_str())
    }
}

/// The goals `seat` holds in `items`: claimed, open and owned by it.
pub fn held<'i>(
    items: &'i BTreeMap<String, Item>,
    seat: &'i str,
) -> impl Iterator<Item = (&'i String, &'i Item)> {
    items
        .iter()
        .filter(move |(_, i)| i.claimed && !i.closed && i.owner.as_deref() == Some(seat))
}

/// Minutes of a session span inside `from..to`; 0 for any other event.
#[must_use]
pub fn session_minutes(event: &Event, from: Timestamp, to: Timestamp) -> f64 {
    if event.kind != Kind::SessionActive {
        return 0.0;
    }
    let start = event.t.max(from);
    let end = event.t_end.unwrap_or(event.t).min(to);
    if end > start {
        minutes(start, end)
    } else {
        0.0
    }
}
