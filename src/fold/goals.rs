//! `andon goals`: the tracker's goals as of an instant, one row each: who
//! holds it, whether it is claimed, and the budget spent of what it allows.
//! Its JSON is its own contract, schema 1 (`tests/schema.rs`).

use jiff::Timestamp;
use serde::Serialize;

use crate::config::Config;
use crate::fold::board::{self, Goal};
use crate::fold::gauge::{Gauge, Scope, Span, Spec, TRACKER};
use crate::fold::projection::Projection;
use crate::ledger::Ledger;

/// The list's schema number.
pub const SCHEMA: u32 = 1;

/// The goals: the tracker's state at the instant.
pub const GOALS: Spec = Spec::new(Scope::Board, "goals", TRACKER, Span::State);

/// The goals as of `asof`.
#[derive(Debug, Clone, Serialize)]
pub struct Goals {
    pub schema: u32,
    pub asof: Timestamp,
    pub goals: Gauge<Vec<Listed>>,
}

/// One goal.
#[derive(Debug, Clone, Serialize)]
pub struct Listed {
    pub id: String,
    pub title: Option<String>,
    /// `open` (nobody has started it), `claimed` or `closed`.
    pub status: &'static str,
    /// Who holds it: a seat, or the identity the tracker recorded.
    pub owner: Option<String>,
    /// Whether the owner is a configured seat.
    pub seated: bool,
    pub item_type: Option<String>,
    pub created_at: Option<Timestamp>,
    pub claimed_at: Option<Timestamp>,
    pub closed_at: Option<Timestamp>,
    /// The budget as written, its amount, the unit `used` counts in (the
    /// budget's, else `[board] budget_unit`), and spent since the claim, as
    /// the board reads it; `used` is `None` for a goal not claimed.
    pub budget: Option<String>,
    pub amount: Option<f64>,
    pub unit: Option<String>,
    pub used: Option<Gauge<f64>>,
}

/// What to list.
#[derive(Debug, Clone, Default)]
pub struct Ask<'a> {
    /// Closed goals too.
    pub all: bool,
    /// Only the goals this seat (or identity) holds.
    pub seat: Option<&'a str>,
}

/// The goals as of `asof`: claimed first, newest claim first; then open,
/// newest first; then (with `all`) closed, newest close first.
#[must_use]
pub fn goals(ledger: &Ledger, config: &Config, asof: Timestamp, ask: &Ask<'_>) -> Goals {
    let ledger = ledger.prefix(asof);
    let p = Projection::new(&ledger, config);
    let g = board::gauger(&p, asof);
    let mut listed: Vec<Listed> =
        p.index
            .items_at(asof)
            .into_iter()
            .filter(|(_, i)| ask.all || !i.closed)
            .filter(|(_, i)| ask.seat.is_none_or(|s| i.owner.as_deref() == Some(s)))
            .map(|(id, item)| {
                let status = if item.closed {
                    "closed"
                } else if item.claimed {
                    "claimed"
                } else {
                    "open"
                };
                let seated = item
                    .owner
                    .as_deref()
                    .is_some_and(|o| config.seats.is_empty() || config.seats.contains_key(o));
                let spent: Option<Goal> =
                    item.owner.as_deref().filter(|_| item.claimed).map(|owner| {
                        board::goal_of(&g, owner, &id, &item, p.events(owner), config)
                    });
                Listed {
                    title: item.title.clone(),
                    status,
                    owner: item.owner.clone(),
                    seated,
                    item_type: item.item_type.clone(),
                    created_at: item.created,
                    claimed_at: item.claimed_at,
                    closed_at: item.closed_at,
                    budget: item.budget,
                    amount: spent.as_ref().and_then(|s| s.amount),
                    unit: spent.as_ref().map(|s| s.used_unit.clone()),
                    used: spent.map(|s| s.used),
                    id,
                }
            })
            .collect();
    listed.sort_by(|a, b| {
        let rank = |l: &Listed| match l.status {
            "claimed" => 0,
            "open" => 1,
            _ => 2,
        };
        let when = |l: &Listed| l.closed_at.or(l.claimed_at).or(l.created_at);
        rank(a)
            .cmp(&rank(b))
            .then_with(|| when(b).cmp(&when(a)))
            .then_with(|| a.id.cmp(&b.id))
    });
    Goals {
        schema: SCHEMA,
        asof,
        goals: g.gauge(&GOALS, None, None, listed),
    }
}
