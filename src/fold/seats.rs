//! `andon seats`: the roster. Each configured seat with the names that join
//! to it, where it works, whether a session runs there, and what joined to
//! it in the window; then every identity the sources recorded that joins
//! to no seat, with the fix.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::Serialize;

use crate::config::Config;
use crate::event::{Kind, attr};
use crate::fold::board::{self, Presence};
use crate::fold::projection::Projection;
use crate::ledger::Ledger;

/// The roster's schema number.
pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct Roster {
    pub schema: u32,
    pub asof: Timestamp,
    /// `joined` counts the facts of the last this many hours.
    pub window_hours: f64,
    pub seats: Vec<Seat>,
    /// Identities the sources recorded that join to no seat.
    pub unseated: Vec<Identity>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Seat {
    pub seat: String,
    pub aliases: Vec<String>,
    pub prefixes: Vec<String>,
    /// The directory its sessions last worked in, else the jj workspace its
    /// changes were last made in.
    pub workspace: Option<String>,
    /// A session running there now, when the sources read sessions.
    pub session: Option<Presence>,
    /// Facts in the window that joined to it, by kind.
    pub joined: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Identity {
    pub identity: String,
    /// Facts recorded under it, by kind.
    pub facts: BTreeMap<String, usize>,
    /// How to make it join a seat.
    pub fix: String,
}

/// The roster as of `asof`.
#[must_use]
pub fn roster(ledger: &Ledger, config: &Config, asof: Timestamp) -> Roster {
    let ledger = ledger.prefix(asof);
    let p = Projection::new(&ledger, config);
    let g = board::gauger(&p, asof);
    let mut names: Vec<String> = config.seats.keys().cloned().collect();
    if names.is_empty() {
        names = p.seats().map(|(s, _)| s.to_owned()).collect();
    }
    let seats = names
        .into_iter()
        .map(|seat| {
            let spec = config.seats.get(&seat);
            let events = p.events(&seat);
            let mut joined: BTreeMap<String, usize> = BTreeMap::new();
            for e in events
                .iter()
                .filter(|e| e.t >= g.from && e.kind != Kind::Coverage)
            {
                *joined.entry(e.kind.to_string()).or_default() += 1;
            }
            let worked = events
                .iter()
                .rev()
                .find_map(|e| e.attr_str(attr::CWD).map(str::to_owned))
                .or_else(|| {
                    events.iter().rev().find_map(|e| {
                        e.attr_str(attr::WORKSPACE)
                            .filter(|w| !w.is_empty())
                            .map(str::to_owned)
                    })
                });
            Seat {
                aliases: spec.map(|s| s.aliases.clone()).unwrap_or_default(),
                prefixes: spec.map(|s| s.prefixes.clone()).unwrap_or_default(),
                workspace: worked,
                session: board::presence(events, asof),
                joined,
                seat,
            }
        })
        .collect();
    Roster {
        schema: SCHEMA,
        asof,
        window_hours: config.board.window_hours,
        seats,
        unseated: unseated(&ledger, config),
    }
}

/// Every name the sources recorded as an actor or an assignee that joins
/// to no seat, with how many facts carry it; none when no seats are
/// configured (every name is then its own seat).
fn unseated(ledger: &Ledger, config: &Config) -> Vec<Identity> {
    if config.seats.is_empty() {
        return Vec::new();
    }
    let mut seen: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for e in ledger.events().iter().filter(|e| e.kind != Kind::Coverage) {
        if e.seat.is_none()
            && let Some(actor) = e.actor.as_deref().filter(|a| !a.is_empty())
        {
            *seen
                .entry(actor.to_owned())
                .or_default()
                .entry(e.kind.to_string())
                .or_default() += 1;
        }
        if let Some(assignee) = e.attr_str(attr::ASSIGNEE).filter(|a| !a.is_empty())
            && !e.has_attr(attr::ASSIGNEE_SEAT)
        {
            *seen
                .entry(assignee.to_owned())
                .or_default()
                .entry("assigned".to_owned())
                .or_default() += 1;
        }
    }
    seen.into_iter()
        .map(|(identity, facts)| Identity {
            fix: format!(
                "add \"{identity}\" to the aliases of the seat it is ([seats.<seat>] aliases), or make its tool record a seat's name"
            ),
            identity,
            facts,
        })
        .collect()
}
