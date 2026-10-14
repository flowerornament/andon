//! The ledger: every event from every adapter, in one deterministic order.
//!
//! andon persists nothing. `andon ledger` prints the ledger as JSONL so that a
//! project can keep it (sources lose history: receipts go with deleted
//! workspaces, messages are swept), and `--ledger FILE` reads a kept one back.

use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use jiff::Timestamp;

use crate::event::{Event, Kind};

/// Order among events at one instant.
const fn rank(kind: &Kind) -> u8 {
    match kind {
        Kind::Create => 0,
        Kind::Claim => 1,
        Kind::Close => 3,
        _ => 2,
    }
}

/// Events in canonical order.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    events: Vec<Event>,
}

impl Ledger {
    /// Build a ledger. Sorts into canonical order and drops exact duplicates,
    /// so the result does not depend on the order adapters ran in.
    #[must_use]
    pub fn new(events: Vec<Event>) -> Self {
        let mut keyed: Vec<(String, Event)> = events
            .into_iter()
            .map(|e| (serde_json::to_string(&e).unwrap_or_default(), e))
            .collect();
        // at one instant, an item is created before it is claimed and claimed
        // before it is closed, whatever its JSON looks like
        keyed.sort_by(|(ka, a), (kb, b)| {
            a.t.cmp(&b.t)
                .then_with(|| rank(&a.kind).cmp(&rank(&b.kind)))
                .then_with(|| ka.cmp(kb))
        });
        keyed.dedup_by(|(ka, _), (kb, _)| ka == kb);
        Self {
            events: keyed.into_iter().map(|(_, e)| e).collect(),
        }
    }

    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// The ledger as it could have been known at `t`: events that began by
    /// then, and for any still running then, no outcome and no end yet (a
    /// gate that failed a minute later had not failed). `asof` folds this, so
    /// a past board needs no stored state and shows nothing early.
    ///
    /// Coverage is kept whatever its instant, its spans clipped to `t`: it
    /// describes where the ledger can look, not something that happened, so
    /// an instant before a source's horizon is known to be uncovered.
    #[must_use]
    pub fn prefix(&self, t: Timestamp) -> Self {
        let events = self
            .events
            .iter()
            .filter(|e| e.t <= t || e.kind == Kind::Coverage)
            .map(|e| {
                if e.kind == Kind::Coverage {
                    let mut c = e.clone();
                    if c.t <= t && c.t_end.is_none_or(|end| end > t) {
                        c.t_end = Some(t);
                    }
                    c
                } else if e.t_end.is_some_and(|end| end > t) {
                    let mut running = e.clone();
                    running.outcome = None;
                    running.t_end = (running.kind == Kind::SessionActive).then_some(t);
                    running
                } else {
                    e.clone()
                }
            })
            .collect();
        Self { events }
    }

    /// Read a kept ledger.
    ///
    /// # Errors
    /// When a line is not an event; the error names the line.
    pub fn read(reader: impl BufRead) -> Result<Self> {
        let mut events = Vec::new();
        for (index, line) in reader.lines().enumerate() {
            let line = line.context("reading the ledger")?;
            if line.trim().is_empty() {
                continue;
            }
            let event: Event = serde_json::from_str(&line)
                .with_context(|| format!("ledger line {} is not an event", index + 1))?;
            events.push(event);
        }
        Ok(Self::new(events))
    }

    /// Write as JSONL, one event per line.
    ///
    /// # Errors
    /// When the writer fails.
    pub fn write(&self, mut writer: impl Write) -> Result<()> {
        for event in &self.events {
            serde_json::to_writer(&mut writer, event)?;
            writer.write_all(b"\n")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::parse_instant;

    fn at(s: &str, kind: Kind) -> Event {
        Event::new(
            parse_instant(s).unwrap_or_else(|e| panic!("{e}")),
            kind,
            "test",
        )
    }

    #[test]
    fn order_does_not_depend_on_input_order() {
        let a = at("2026-10-11T01:00:00Z", Kind::Land);
        let b = at("2026-10-11T00:00:00Z", Kind::Run);
        let c = at("2026-10-11T00:00:00Z", Kind::Message);
        let one = Ledger::new(vec![a.clone(), b.clone(), c.clone()]);
        let two = Ledger::new(vec![c, a, b.clone(), b]);
        assert_eq!(one.events(), two.events());
        assert_eq!(one.len(), 3);
    }

    #[test]
    fn written_ledger_reads_back_identically() {
        let mut land = at("2026-10-11T01:00:00Z", Kind::Land);
        land.ids.commit = Some("abc".to_owned());
        land.set_attr("recipe", "land");
        let ledger = Ledger::new(vec![land, at("2026-10-11T00:00:00Z", Kind::Run)]);
        let mut bytes = Vec::new();
        ledger.write(&mut bytes).unwrap_or_else(|e| panic!("{e}"));
        let back = Ledger::read(bytes.as_slice()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back.events(), ledger.events());
        let mut again = Vec::new();
        back.write(&mut again).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(bytes, again);
    }

    #[test]
    fn prefix_keeps_what_began_by_then() {
        let ledger = Ledger::new(vec![
            at("2026-10-11T00:00:00Z", Kind::Run),
            at("2026-10-12T00:00:00Z", Kind::Land),
        ]);
        let t = parse_instant("2026-10-11T12:00:00Z").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ledger.prefix(t).len(), 1);
    }

    #[test]
    fn a_run_still_going_at_the_cut_has_no_outcome_yet() {
        let mut gate = at("2026-10-11T15:58:56Z", Kind::GateRun);
        gate.t_end = Some(parse_instant("2026-10-11T16:00:19Z").unwrap_or_else(|e| panic!("{e}")));
        gate.outcome = Some(crate::event::Outcome::Fail);
        let mut span = at("2026-10-11T15:00:00Z", Kind::SessionActive);
        span.t_end = gate.t_end;
        let ledger = Ledger::new(vec![gate, span]);
        let cut = parse_instant("2026-10-11T15:59:00Z").unwrap_or_else(|e| panic!("{e}"));
        let then = ledger.prefix(cut);
        let gate = then.events().iter().find(|e| e.kind == Kind::GateRun);
        assert_eq!(gate.and_then(|g| g.outcome), None);
        assert_eq!(gate.and_then(|g| g.t_end), None);
        let span = then.events().iter().find(|e| e.kind == Kind::SessionActive);
        assert_eq!(span.and_then(|s| s.t_end), Some(cut));
        // after it ended, the outcome is there
        let later = parse_instant("2026-10-11T17:00:00Z").unwrap_or_else(|e| panic!("{e}"));
        assert!(
            ledger
                .prefix(later)
                .events()
                .iter()
                .any(|e| e.outcome.is_some())
        );
    }

    #[test]
    fn a_close_sorts_after_a_claim_at_the_same_instant() {
        let mut claim = at("2026-10-11T10:00:00Z", Kind::Claim);
        claim.seat = Some("zz-last-alphabetically".into());
        let close = at("2026-10-11T10:00:00Z", Kind::Close);
        let ledger = Ledger::new(vec![close, claim]);
        let kinds: Vec<&Kind> = ledger.events().iter().map(|e| &e.kind).collect();
        assert_eq!(kinds, vec![&Kind::Claim, &Kind::Close]);
    }

    #[test]
    fn a_bad_line_is_named() {
        let err = Ledger::read(&b"{\"t\":1}\n"[..])
            .err()
            .map(|e| e.to_string());
        assert_eq!(err.as_deref(), Some("ledger line 1 is not an event"));
    }
}
