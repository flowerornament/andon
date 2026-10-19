//! The folds: every surface is a pure function of the ledger, the config and
//! an instant supplied by the caller. Nothing here reads the clock.

pub mod board;
pub mod coverage;
pub mod gauge;
pub mod scorecard;

use std::collections::{BTreeMap, HashMap, HashSet};

use jiff::Timestamp;

use crate::event::attr;
use crate::event::{Event, Kind};
use crate::ledger::Ledger;

/// Lookups the folds share, built once per ledger.
pub struct Index<'a> {
    pub lands: Vec<&'a Event>,
    pub gate_runs: Vec<&'a Event>,
    /// Verdict id → verdict.
    pub verdicts: HashMap<&'a str, &'a Event>,
    /// Change id → verdicts on it (reviews that join by change, not trailer).
    pub verdicts_by_change: HashMap<&'a str, Vec<&'a Event>>,
    /// Sessions the sessions source saw.
    pub sessions_seen: HashSet<&'a str>,
    /// Commit id → earliest change start (author time).
    starts: HashMap<&'a str, Timestamp>,
    items: Vec<&'a Event>,
}

impl<'a> Index<'a> {
    #[must_use]
    pub fn new(ledger: &'a Ledger) -> Self {
        let mut index = Self {
            lands: Vec::new(),
            gate_runs: Vec::new(),
            verdicts: HashMap::new(),
            verdicts_by_change: HashMap::new(),
            sessions_seen: HashSet::new(),
            starts: HashMap::new(),
            items: Vec::new(),
        };
        for event in ledger.events() {
            match event.kind {
                Kind::Land => index.lands.push(event),
                Kind::GateRun => index.gate_runs.push(event),
                Kind::Verdict => {
                    if let Some(id) = event.ids.verdict.as_deref() {
                        index.verdicts.entry(id).or_insert(event);
                    }
                    // a verdict joins its landing by change only when its
                    // source says so (a pull request's review); elsewhere the
                    // landing's trailers cite it
                    if event.attr_str(attr::JOIN) == Some("change")
                        && let Some(change) = event.ids.change.as_deref()
                    {
                        index
                            .verdicts_by_change
                            .entry(change)
                            .or_default()
                            .push(event);
                    }
                }
                Kind::SessionActive => {
                    if let Some(s) = event.ids.session.as_deref() {
                        index.sessions_seen.insert(s);
                    }
                }
                Kind::Commit => {
                    if let Some(c) = event.ids.commit.as_deref() {
                        let entry = index.starts.entry(c).or_insert(event.t);
                        if event.t < *entry {
                            *entry = event.t;
                        }
                    }
                }
                Kind::Create | Kind::Claim | Kind::Close => index.items.push(event),
                _ => {}
            }
        }
        index
    }

    /// The verdicts a landing has: those its trailers cite, and those on its
    /// change (a pull request's reviews).
    #[must_use]
    pub fn verdicts_of(&self, land: &Event) -> Vec<&'a Event> {
        let mut out: Vec<&Event> = land
            .ids
            .cites
            .iter()
            .filter_map(|c| self.verdicts.get(c.as_str()).copied())
            .collect();
        if let Some(change) = land.ids.change.as_deref() {
            for v in self.verdicts_by_change.get(change).into_iter().flatten() {
                if v.t <= land.t && !out.iter().any(|o| std::ptr::eq(*o, *v)) {
                    out.push(v);
                }
            }
        }
        out
    }

    /// When the change behind a commit started.
    #[must_use]
    pub fn change_start(&self, commit: &str) -> Option<Timestamp> {
        self.starts.get(commit).copied()
    }

    /// Every tracker item's state at `t` (events at or before `t`).
    #[must_use]
    pub fn items_at(&self, t: Timestamp) -> BTreeMap<String, Item> {
        let mut items: BTreeMap<String, Item> = BTreeMap::new();
        for event in self.items.iter().filter(|e| e.t <= t) {
            let Some(id) = event.ids.goal.as_ref().or(event.subject.as_ref()) else {
                continue;
            };
            let item = items.entry(id.clone()).or_default();
            match event.kind {
                Kind::Create => {
                    item.created = Some(event.t);
                    if item.owner.is_none() {
                        item.owner = event
                            .attr_str(attr::ASSIGNEE_SEAT)
                            .or_else(|| event.attr_str(attr::ASSIGNEE))
                            .map(str::to_owned);
                    }
                    if item.title.is_none() {
                        item.title = event.attr_str(attr::TITLE).map(str::to_owned);
                    }
                    item.budget = event.attr_str(attr::BUDGET).map(str::to_owned);
                    item.item_type = event.attr_str(attr::ITEM_TYPE).map(str::to_owned);
                }
                Kind::Claim => {
                    item.owner = event.seat.clone().or_else(|| event.actor.clone());
                    item.claimed = true;
                    item.claimed_at = Some(event.t);
                    item.closed = false;
                }
                Kind::Close => {
                    item.closed = true;
                    item.closed_at = Some(event.t);
                }
                _ => {}
            }
        }
        items
    }
}

/// One tracker item, folded.
#[derive(Debug, Clone, Default)]
pub struct Item {
    pub created: Option<Timestamp>,
    pub owner: Option<String>,
    pub claimed: bool,
    pub claimed_at: Option<Timestamp>,
    pub closed: bool,
    pub closed_at: Option<Timestamp>,
    pub title: Option<String>,
    pub budget: Option<String>,
    /// The tracker's type for it (feature, bug, task, epic…), when it says.
    pub item_type: Option<String>,
}

impl Item {
    /// Whether it counts toward a seat's WIP: claimed, open, and of a type
    /// `[goal] types` names (an item of no stated type counts).
    #[must_use]
    pub fn in_progress(&self, config: &crate::config::Config) -> bool {
        self.claimed
            && !self.closed
            && self
                .item_type
                .as_deref()
                .is_none_or(|t| t.is_empty() || config.goal.types.iter().any(|x| x == t))
    }
}

/// The median, rounded to two decimals; `None` for no values.
#[must_use]
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    let m = if sorted.len().is_multiple_of(2) {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    };
    Some(round2(m))
}

/// `a / b`, rounded; `None` when `b` is zero.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn ratio(a: usize, b: usize) -> Option<f64> {
    (b > 0).then(|| round2(a as f64 / b as f64))
}

/// Round to two decimals, so JSON output is stable across platforms.
#[must_use]
pub fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_even_and_odd_counts() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn ratio_refuses_division_by_zero() {
        assert_eq!(ratio(600, 387), Some(1.55));
        assert_eq!(ratio(1, 0), None);
    }
}
