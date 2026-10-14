//! The one type every gauge is: known, or unknown with the reason
//! (`.design/2026-10-14-coverage.md`).
//!
//! A gauge reaches output only as a [`Gauge`], built by [`Gauger::gauge`]
//! from a registered [`Spec`] that names the kinds it reads. Coverage of
//! those kinds decides which it is. Renderers can only print what a gauge
//! holds: text prints `?` for an unknown one, JSON `{"unknown": reason}`, and
//! flags and change lines read gauges, not raw counts. So a 0 where the ledger
//! could not look cannot be represented, let alone printed.

use jiff::Timestamp;
use serde::{Serialize, Serializer};

use crate::fold::coverage::Coverage;

/// A gauge's value, or why the ledger cannot give one. Sealed: outside this
/// module a gauge is read, never made, so the only way to a known value is
/// [`Gauger::gauge`], through the registry and coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gauge<T>(State<T>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum State<T> {
    Known(T),
    Unknown(String),
}

impl<T> Gauge<T> {
    const fn known(v: T) -> Self {
        Self(State::Known(v))
    }

    const fn unknown(why: String) -> Self {
        Self(State::Unknown(why))
    }

    /// The value, when known.
    #[must_use]
    pub const fn get(&self) -> Option<&T> {
        match &self.0 {
            State::Known(v) => Some(v),
            State::Unknown(_) => None,
        }
    }

    /// Why it is unknown, when it is.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match &self.0 {
            State::Known(_) => None,
            State::Unknown(r) => Some(r),
        }
    }

    #[must_use]
    pub const fn is_known(&self) -> bool {
        matches!(self.0, State::Known(_))
    }

    /// A text cell: the value through `show`, or `?`.
    pub fn cell(&self, show: impl FnOnce(&T) -> String) -> String {
        match &self.0 {
            State::Known(v) => show(v),
            State::Unknown(_) => "?".to_owned(),
        }
    }

    /// The same reason over a derived value: what is unknown stays unknown.
    pub fn map<U>(&self, f: impl FnOnce(&T) -> U) -> Gauge<U> {
        match &self.0 {
            State::Known(v) => Gauge::known(f(v)),
            State::Unknown(r) => Gauge::unknown(r.clone()),
        }
    }
}

/// A known gauge equals its value; an unknown one equals no value, not even
/// 0, which is the point.
impl<T: PartialEq> PartialEq<T> for Gauge<T> {
    fn eq(&self, other: &T) -> bool {
        self.get() == Some(other)
    }
}

impl<T: Serialize> Serialize for Gauge<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            State::Known(v) => v.serialize(s),
            State::Unknown(r) => serde_json::json!({ "unknown": r }).serialize(s),
        }
    }
}

/// Over what span a gauge reads its kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// The window (or the board's trailing hours).
    Window,
    /// The state at the instant, such as goals held: its kinds read over all
    /// history, or a current-state source read at the instant.
    State,
    /// From an origin the fold supplies (a claim, a landing) to the instant.
    Origin,
}

/// A registered gauge: its path in the output and the kinds it reads.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub path: &'static str,
    pub kinds: &'static [&'static str],
    /// Kinds read only for a single seat (a verdict reaches its seat through
    /// the reviewed commit, so a seat's verdicts need change starts too).
    pub seat_kinds: &'static [&'static str],
    pub span: Span,
    /// Gauges this one is unknown with: it is computed over their values
    /// (findings over the goals held), so it reads what they read.
    pub via: &'static [&'static str],
}

impl Spec {
    /// Whether this gauge reads `kind`, itself or through a gauge it is
    /// computed over; `seat` for a single seat's instance.
    #[must_use]
    pub fn reads(&self, kind: &str, seat: bool) -> bool {
        self.kinds.contains(&kind)
            || (seat && self.seat_kinds.contains(&kind))
            || self
                .via
                .iter()
                .filter_map(|p| registered(p))
                .any(|s| s.reads(kind, seat))
    }
}

const fn spec(path: &'static str, kinds: &'static [&'static str], span: Span) -> Spec {
    Spec {
        path,
        kinds,
        seat_kinds: &[],
        span,
        via: &[],
    }
}

const fn via(
    path: &'static str,
    kinds: &'static [&'static str],
    via: &'static [&'static str],
) -> Spec {
    Spec {
        path,
        kinds,
        seat_kinds: &[],
        span: Span::Origin,
        via,
    }
}

const TRACKER: &[&str] = &["create", "claim", "close"];

/// Every gauge the scorecard and board can print. A gauge not here cannot be
/// built ([`Gauger::gauge`] makes it unknown), and the property test checks
/// that every gauge in the output is here.
pub const REGISTRY: &[Spec] = &[
    // a seat card (and the total and unattributed cards)
    spec("card.landings", &["land"], Span::Window),
    spec("card.runs", &["run"], Span::Window),
    spec("card.runs_per_landing", &["run", "land"], Span::Window),
    spec("card.gate_runs", &["gate_run"], Span::Window),
    Spec {
        path: "card.verdicts_go",
        kinds: &["verdict"],
        seat_kinds: &["commit"],
        span: Span::Window,
        via: &[],
    },
    Spec {
        path: "card.verdicts_return",
        kinds: &["verdict"],
        seat_kinds: &["commit"],
        span: Span::Window,
        via: &[],
    },
    spec("card.messages_sent", &["message"], Span::Window),
    spec("card.messages_received", &["message"], Span::Window),
    spec("card.messages_naming_goal", &["message"], Span::Window),
    spec("card.decisions_without_goal", &["message"], Span::Window),
    spec("card.items_created", &["create"], Span::Window),
    spec("card.wip_at_end", TRACKER, Span::State),
    spec("card.active_hours", &["session_active"], Span::Window),
    // scorecard-wide
    spec("flow.lead_median_min", &["land", "commit"], Span::Window),
    spec("flow.lead_n", &["land", "commit"], Span::Window),
    spec(
        "flow.review_median_min",
        &["land", "verdict", "commit"],
        Span::Window,
    ),
    spec(
        "flow.review_n",
        &["land", "verdict", "commit"],
        Span::Window,
    ),
    spec("flow.gate_median_min", &["gate_run"], Span::Window),
    spec("flow.gate_n", &["gate_run"], Span::Window),
    spec("tracker.created", &["create"], Span::Window),
    spec("tracker.closed", &["close"], Span::Window),
    spec("tracker.branching", &["create", "close"], Span::Window),
    spec("tracker.open_at_end", TRACKER, Span::State),
    spec("tracker.unowned_at_end", TRACKER, Span::State),
    spec("hub", &["message"], Span::Window),
    spec("joins.landings", &["land"], Span::Window),
    spec("joins.landing_session", &["land"], Span::Window),
    spec(
        "joins.landing_session_seen",
        &["land", "session_active"],
        Span::Window,
    ),
    spec(
        "joins.landings_citing_verdict",
        &["land", "verdict"],
        Span::Window,
    ),
    spec("joins.cites", &["land"], Span::Window),
    spec("joins.cites_resolved", &["land", "verdict"], Span::Window),
    spec("joins.runs", &["run"], Span::Window),
    spec("joins.runs_attributed", &["run"], Span::Window),
    spec("runs_by_repo", &["run"], Span::Window),
    // a board row
    spec("row.goals", TRACKER, Span::State),
    spec("row.landings", &["land"], Span::Window),
    spec("row.last_land", &["land"], Span::Origin),
    spec("row.runs", &["run"], Span::Window),
    spec("row.runs_pass", &["run"], Span::Window),
    spec("row.runs_since_land", &["run", "land"], Span::Origin),
    Spec {
        path: "row.last_verdict",
        kinds: &["verdict"],
        seat_kinds: &["commit"],
        span: Span::Origin,
        via: &[],
    },
    via("row.findings", &["other:comment"], &["row.goals"]),
    via("row.goals.used", &["session_active"], &["row.goals"]),
    via("row.goals.used.runs", &["run"], &["row.goals"]),
    via("row.goals.used.wall", &[], &["row.goals"]),
];

/// The spec registered at `path`.
#[must_use]
pub fn registered(path: &str) -> Option<&'static Spec> {
    REGISTRY.iter().find(|s| s.path == path)
}

/// Builds gauges against one ledger's coverage, over one window and instant.
pub struct Gauger<'a> {
    pub coverage: &'a Coverage,
    pub from: Timestamp,
    pub to: Timestamp,
}

impl Gauger<'_> {
    /// The registered gauge at `path` for `seat` (`None`: every seat), its
    /// value `v` when every kind it reads is covered; `origin` is where a
    /// [`Span::Origin`] gauge reads from (`None`: it cannot be known).
    pub fn gauge<T>(
        &self,
        path: &str,
        seat: Option<&str>,
        origin: Option<Timestamp>,
        v: T,
    ) -> Gauge<T> {
        let Some(spec) = registered(path) else {
            return Gauge::unknown(format!("{path} is not a registered gauge"));
        };
        let mut kinds: Vec<&str> = spec.kinds.to_vec();
        if seat.is_some() {
            kinds.extend(spec.seat_kinds);
        }
        let from = match spec.span {
            Span::Window => self.from,
            Span::State => {
                return match self.coverage.check_state(&kinds, seat, self.to) {
                    None => Gauge::known(v),
                    Some(why) => Gauge::unknown(why),
                };
            }
            Span::Origin => match origin {
                Some(o) if o <= self.to => o,
                // an origin after the instant, or none: the count has no
                // start the ledger read; say which source and horizon
                _ => {
                    let why = self
                        .coverage
                        .check(&kinds, seat, crate::event::ALL_HISTORY, self.to)
                        .unwrap_or_else(|| {
                            format!(
                                "{} has no start before the instant to count from",
                                spec.path
                            )
                        });
                    return Gauge::unknown(why);
                }
            },
        };
        match self.coverage.check(&kinds, seat, from, self.to) {
            None => Gauge::known(v),
            Some(why) => Gauge::unknown(why),
        }
    }
}

impl Gauger<'_> {
    /// A gauge computed over `parent`'s value (findings over the goals
    /// held): unknown with the parent's reason when the parent is, else as
    /// [`Self::gauge`]. Its spec must name the parent in `via`.
    pub fn over<U, T>(
        &self,
        parent: &Gauge<U>,
        path: &str,
        seat: Option<&str>,
        origin: Option<Timestamp>,
        v: T,
    ) -> Gauge<T> {
        if registered(path).is_none_or(|s| s.via.is_empty()) {
            return Gauge::unknown(format!("{path} is not registered as computed over a gauge"));
        }
        match parent.reason() {
            Some(why) => Gauge::unknown(why.to_owned()),
            None => self.gauge(path, seat, origin, v),
        }
    }
}

/// Every unknown gauge in a serialized output, as (path, reason).
#[must_use]
pub fn unknowns(v: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(v, String::new(), &mut out);
    out
}

fn walk(v: &serde_json::Value, path: String, out: &mut Vec<(String, String)>) {
    match v {
        serde_json::Value::Object(map) => {
            if let (1, Some(serde_json::Value::String(why))) = (map.len(), map.get("unknown")) {
                out.push((path, why.clone()));
                return;
            }
            for (k, x) in map {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(x, p, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (i, x) in items.iter().enumerate() {
                walk(x, format!("{path}.{i}"), out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gauge_serializes_as_its_value_or_its_reason() {
        let known: Gauge<usize> = Gauge::known(3);
        let unknown: Gauge<usize> = Gauge::unknown("no source reads runs".into());
        assert_eq!(serde_json::to_string(&known).unwrap_or_default(), "3");
        assert_eq!(
            serde_json::to_string(&unknown).unwrap_or_default(),
            r#"{"unknown":"no source reads runs"}"#
        );
        assert_eq!(unknown.cell(ToString::to_string), "?");
        assert!(unknown != 0 && known == 3);
        assert_eq!(
            unknown.map(|v| v + 1).reason(),
            Some("no source reads runs")
        );
    }

    /// A gauge that reads no kind is never unknown for want of coverage, so
    /// it must be declared: today only wall-clock budget, which is the
    /// claim's age and is unknown with the goals held.
    const KINDLESS: &[&str] = &["row.goals.used.wall"];

    #[test]
    fn a_gauge_that_reads_no_kind_is_declared() {
        for spec in REGISTRY {
            if spec.kinds.is_empty() {
                assert!(KINDLESS.contains(&spec.path), "{} reads no kind", spec.path);
            }
        }
    }

    #[test]
    fn every_registered_path_is_unique() {
        let mut paths: Vec<&str> = REGISTRY.iter().map(|s| s.path).collect();
        paths.sort_unstable();
        let n = paths.len();
        paths.dedup();
        assert_eq!(paths.len(), n);
    }
}
