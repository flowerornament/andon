//! Coverage: where the ledger was able to look (`.design/2026-10-14-coverage.md`).
//!
//! Every gauge names the kinds it reads; it is known for a seat over a span
//! only if each kind is covered for that seat over the whole span, with no
//! gap standing. [`Coverage::check`] answers that, and when the answer is no,
//! says why, so a gauge renders unknown with its reason and never 0.

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::event::{Kind, Outcome};
use crate::ledger::Ledger;

#[derive(Debug, Clone)]
struct Span {
    covers: Vec<String>,
    from: Timestamp,
    to: Option<Timestamp>,
    seat: Option<String>,
    source: String,
}

#[derive(Debug, Clone)]
struct Gap {
    covers: Vec<String>,
    seat: Option<String>,
    reason: String,
}

/// What the ledger covers.
#[derive(Debug, Clone, Default)]
pub struct Coverage {
    spans: Vec<Span>,
    gaps: Vec<Gap>,
}

/// Which seat a coverage event is scoped to: none for every seat, else its
/// resolved seat (or its label, which then matches no seat).
fn scope(e: &crate::event::Event) -> Option<String> {
    e.actor
        .as_ref()
        .map(|a| e.seat.clone().unwrap_or_else(|| format!("unresolved:{a}")))
}

impl Coverage {
    #[must_use]
    pub fn new(ledger: &Ledger) -> Self {
        let mut c = Self::default();
        for e in ledger.events().iter().filter(|e| e.kind == Kind::Coverage) {
            let covers: Vec<String> = e
                .attr_strs("covers")
                .into_iter()
                .map(str::to_owned)
                .collect();
            match e.outcome {
                Some(Outcome::Fail) => c.gaps.push(Gap {
                    covers,
                    seat: scope(e),
                    reason: e.attr_str("reason").unwrap_or("unreadable").to_owned(),
                }),
                _ => c.spans.push(Span {
                    covers,
                    from: e.t,
                    to: e.t_end,
                    seat: scope(e),
                    source: e.source.clone(),
                }),
            }
        }
        c
    }

    /// Where `kind` starts being covered for `seat`: the earliest span that
    /// applies to it (or to every seat).
    #[must_use]
    pub fn covered_from(&self, kind: &str, seat: Option<&str>) -> Option<Timestamp> {
        self.spans
            .iter()
            .filter(|s| s.covers.iter().any(|c| c == kind))
            .filter(|s| match (&s.seat, seat) {
                (None, _) => true,
                (Some(scoped), Some(seat)) => scoped == seat,
                (Some(_), None) => false,
            })
            .map(|s| s.from)
            .min()
    }

    /// Whether any source reads `kind` (a span or a gap names it): a kind no
    /// source reads is a fact of the configuration, not of a span.
    #[must_use]
    pub fn reads(&self, kind: &str) -> bool {
        self.spans
            .iter()
            .any(|s| s.covers.iter().any(|c| c == kind))
            || self.gaps.iter().any(|g| g.covers.iter().any(|c| c == kind))
    }

    /// `None` when every kind is covered for `seat` (or, with `None`, for
    /// every seat) from `from` to `to`; else why not.
    #[must_use]
    pub fn check(
        &self,
        kinds: &[&str],
        seat: Option<&str>,
        from: Timestamp,
        to: Timestamp,
    ) -> Option<String> {
        for kind in kinds {
            if let Some(reason) = self.check_kind(kind, seat, from, to) {
                return Some(reason);
            }
        }
        None
    }

    /// `None` when the state of `kinds` at the instant `at` is known for
    /// `seat`: each kind read over all history to `at`, or a current-state
    /// source (`state:<kind>`, such as a tracker's export) read at `at`. A
    /// current state says nothing about an earlier instant: what was claimed
    /// then and released since is not in it.
    #[must_use]
    pub fn check_state(&self, kinds: &[&str], seat: Option<&str>, at: Timestamp) -> Option<String> {
        for kind in kinds {
            let Some(history) = self.check_kind(kind, seat, crate::event::ALL_HISTORY, at) else {
                continue;
            };
            let state = format!("state:{kind}");
            if !self.reads(&state) {
                return Some(history);
            }
            if let Some(why) = self.check_kind(&state, seat, at, at) {
                return Some(why);
            }
        }
        None
    }

    fn check_kind(
        &self,
        kind: &str,
        seat: Option<&str>,
        from: Timestamp,
        to: Timestamp,
    ) -> Option<String> {
        let applies = |s: &Option<String>| match (s, seat) {
            (None, _) => true,
            (Some(scoped), Some(seat)) => scoped == seat,
            (Some(_), None) => false,
        };
        let names = noun(kind);
        let gaps: Vec<&Gap> = self
            .gaps
            .iter()
            .filter(|g| {
                g.covers.iter().any(|c| c == kind)
                    && match (&g.seat, seat) {
                        (None, _) | (_, None) => true,
                        (Some(scoped), Some(seat)) => scoped == seat,
                    }
            })
            .collect();
        if let Some(first) = gaps.first() {
            let n = gaps.len();
            let sources = if n == 1 { "source" } else { "sources" };
            return Some(format!(
                "{n} {names} {sources} could not be read: {}",
                first.reason
            ));
        }
        let mut spans: Vec<&Span> = self
            .spans
            .iter()
            .filter(|s| s.covers.iter().any(|c| c == kind) && applies(&s.seat))
            .collect();
        if spans.is_empty() {
            // a current-state source reads the kind, but not as history
            let state = format!("state:{kind}");
            if let Some(s) = self.spans.iter().find(|s| s.covers.contains(&state)) {
                return Some(format!(
                    "{names} are read only as current state, not history ({})",
                    s.source
                ));
            }
            return Some(format!("no source reads {names}"));
        }
        spans.sort_by_key(|s| s.from);
        // walk the union of the spans from `from`; any hole before `to` is
        // the reason. A span must contain `from` itself: an instant (`from ==
        // to`) is covered only by a span that holds it, never by default
        let mut reached = from;
        let mut held = false;
        for s in &spans {
            if s.from > reached {
                break;
            }
            match s.to {
                None => return None,
                Some(end) if end >= reached => {
                    held = true;
                    reached = end;
                }
                Some(_) => continue,
            }
            if reached >= to {
                return None;
            }
        }
        if held && reached >= to {
            return None;
        }
        let earliest = spans[0];
        if earliest.from > from {
            Some(format!(
                "{names} are read from {} ({})",
                day(earliest.from),
                earliest.source
            ))
        } else {
            // name the latest instant a span reached (the capture), not the
            // instant asked for: a hole that starts before `from` ends where
            // the read ended
            let until = if held {
                reached
            } else {
                spans.iter().filter_map(|s| s.to).max().unwrap_or(reached)
            };
            Some(format!(
                "{names} are read only until {} ({})",
                day(until),
                earliest.source
            ))
        }
    }
}

fn day(t: Timestamp) -> String {
    t.to_zoned(TimeZone::UTC)
        .strftime("%Y-%m-%d %H:%M")
        .to_string()
}

/// A kind, as a plural noun for a reason.
#[must_use]
pub fn noun(kind: &str) -> &str {
    match kind {
        "land" => "landings",
        "commit" => "change starts",
        "run" => "runs",
        "gate_run" => "gate runs",
        "verdict" => "verdicts",
        "message" => "messages",
        "create" => "tracker items",
        "claim" => "claims",
        "close" => "closes",
        "session_active" => "sessions",
        "other:comment" => "findings",
        "state:create" | "state:claim" | "state:close" => "goal states",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{coverage, gap};
    use crate::time::parse_instant;

    fn at(s: &str) -> Timestamp {
        parse_instant(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn cov(events: Vec<crate::event::Event>) -> Coverage {
        Coverage::new(&Ledger::new(events))
    }

    #[test]
    fn a_span_covers_inside_and_names_its_horizon_outside() {
        let c = cov(vec![coverage(
            at("2026-10-09T00:00:00Z"),
            Some(at("2026-10-14T00:00:00Z")),
            &["land"],
            "jj",
            None,
        )]);
        assert_eq!(
            c.check(
                &["land"],
                Some("x-1a"),
                at("2026-10-10T00:00:00Z"),
                at("2026-10-12T00:00:00Z")
            ),
            None
        );
        assert_eq!(
            c.check(
                &["land"],
                None,
                at("2026-10-01T00:00:00Z"),
                at("2026-10-06T00:00:00Z")
            )
            .as_deref(),
            Some("landings are read from 2026-10-09 00:00 (jj)")
        );
        assert_eq!(
            c.check(
                &["land"],
                None,
                at("2026-10-10T00:00:00Z"),
                at("2026-10-15T00:00:00Z")
            )
            .as_deref(),
            Some("landings are read only until 2026-10-14 00:00 (jj)")
        );
        assert_eq!(
            c.check(
                &["run"],
                None,
                at("2026-10-10T00:00:00Z"),
                at("2026-10-11T00:00:00Z")
            )
            .as_deref(),
            Some("no source reads runs")
        );
    }

    #[test]
    fn an_instant_is_covered_only_by_a_span_that_holds_it() {
        // a current-state source read at 04:00 says nothing of 00:30
        let at4 = at("2026-10-14T04:00:00Z");
        let c = cov(vec![coverage(at4, Some(at4), &["state:claim"], "bd", None)]);
        let early = at("2026-10-13T00:30:00Z");
        assert_eq!(c.check(&["state:claim"], None, at4, at4), None);
        assert!(c.check(&["state:claim"], None, early, early).is_some());
        let late = at("2026-10-14T05:00:00Z");
        assert!(c.check(&["state:claim"], None, late, late).is_some());
        assert!(c.check_state(&["claim"], None, early).is_some());
        // after the capture, the reason names the capture, not the instant
        assert_eq!(
            c.check(&["state:claim"], None, late, late).as_deref(),
            Some("goal states are read only until 2026-10-14 04:00 (bd)")
        );
        assert_eq!(c.check_state(&["claim"], None, at4), None);
    }

    #[test]
    fn adjacent_spans_join_and_a_gap_overrules_them() {
        let c = cov(vec![
            coverage(
                at("2026-10-01T00:00:00Z"),
                Some(at("2026-10-05T00:00:00Z")),
                &["run"],
                "a",
                None,
            ),
            coverage(at("2026-10-05T00:00:00Z"), None, &["run"], "b", None),
        ]);
        assert_eq!(
            c.check(
                &["run"],
                None,
                at("2026-10-02T00:00:00Z"),
                at("2026-10-20T00:00:00Z")
            ),
            None
        );
        let mut events = vec![coverage(
            at("2026-10-01T00:00:00Z"),
            None,
            &["session_active"],
            "s",
            None,
        )];
        events.push(gap(
            at("2026-10-13T00:00:00Z"),
            &["session_active"],
            "s",
            None,
            "x.jsonl: permission denied",
        ));
        let c = cov(events);
        assert_eq!(
            c.check(
                &["session_active"],
                Some("x-1a"),
                at("2026-10-02T00:00:00Z"),
                at("2026-10-03T00:00:00Z")
            )
            .as_deref(),
            Some("1 sessions source could not be read: x.jsonl: permission denied")
        );
    }
}
