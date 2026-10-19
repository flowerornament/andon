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

/// Where in an output a gauge sits: each scope is one or more roots in the
/// JSON, and a gauge's output paths are its roots joined to its field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The board, beyond its rows.
    Board,
    /// A board row: `seats.<n>`.
    Row,
    /// The scorecard, beyond its cards.
    Scorecard,
    /// A scorecard card: a seat's (`seats.<seat>`), `total` and
    /// `unattributed`.
    Card,
}

impl Scope {
    /// The output roots, `*` standing for any one key or index.
    #[must_use]
    pub const fn roots(self) -> &'static [&'static str] {
        match self {
            Self::Board | Self::Scorecard => &[""],
            Self::Row => &["seats.*"],
            Self::Card => &["seats.*", "total", "unattributed"],
        }
    }
}

/// A gauge: where it is in the output and the kinds it reads. A gauge is
/// declared as a `Spec` constant beside the fold that computes it, and built
/// only through it, so its output path, its kinds and its value cannot
/// drift apart.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub scope: Scope,
    /// The field under each of the scope's roots, `*` for any key or index:
    /// `landings`, `goals.*.used`, `flow.lead_n`.
    pub field: &'static str,
    pub kinds: &'static [&'static str],
    /// Kinds read only for a single seat (a verdict reaches its seat through
    /// the reviewed commit, so a seat's verdicts need change starts too).
    pub seat_kinds: &'static [&'static str],
    pub span: Span,
    /// Gauges this one is unknown with: it is computed over their values
    /// (findings over the goals held), so it reads what they read.
    pub via: &'static [&'static Self],
    /// Gauges at one path that read different kinds by what they count (a
    /// budget spent in hours, runs or wall clock) differ by variant; the
    /// first declared is the path's.
    pub variant: &'static str,
}

impl Spec {
    /// A gauge over `kinds` at `field` of `scope`.
    #[must_use]
    pub const fn new(
        scope: Scope,
        field: &'static str,
        kinds: &'static [&'static str],
        span: Span,
    ) -> Self {
        Self {
            scope,
            field,
            kinds,
            seat_kinds: &[],
            span,
            via: &[],
            variant: "",
        }
    }

    /// Also reading `kinds` for a single seat.
    #[must_use]
    pub const fn for_seat(mut self, kinds: &'static [&'static str]) -> Self {
        self.seat_kinds = kinds;
        self
    }

    /// Computed over `parents`' values.
    #[must_use]
    pub const fn over(mut self, parents: &'static [&'static Self]) -> Self {
        self.via = parents;
        self
    }

    /// A variant of the gauge at the same path.
    #[must_use]
    pub const fn variant(mut self, name: &'static str) -> Self {
        self.variant = name;
        self
    }

    /// Whether this gauge reads `kind`, itself or through a gauge it is
    /// computed over; `seat` for a single seat's instance.
    #[must_use]
    pub fn reads(&self, kind: &str, seat: bool) -> bool {
        self.kinds.contains(&kind)
            || (seat && self.seat_kinds.contains(&kind))
            || self.via.iter().any(|s| s.reads(kind, seat))
    }

    /// Whether `path`, a path in the output, is an instance of this gauge.
    #[must_use]
    pub fn at(&self, path: &str) -> bool {
        self.scope.roots().iter().any(|root| {
            let pattern = if root.is_empty() {
                self.field.to_owned()
            } else {
                format!("{root}.{}", self.field)
            };
            matches(&pattern, path)
        })
    }

    /// The gauge's name in a reason: its field's first word (`goals.*.used`
    /// is the budget).
    #[must_use]
    pub fn name(&self) -> &'static str {
        if self.field.rsplit('.').next() == Some("used") {
            "budget"
        } else {
            self.field.split('.').next().unwrap_or(self.field)
        }
    }
}

/// A dotted path against a pattern whose `*` segments match any one segment.
fn matches(pattern: &str, path: &str) -> bool {
    let (mut p, mut q) = (pattern.split('.'), path.split('.'));
    loop {
        match (p.next(), q.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) if a == "*" || a == b => {}
            _ => return false,
        }
    }
}

/// The tracker's kinds: what a goal's state is read from.
pub const TRACKER: &[&str] = &["create", "claim", "close"];

/// Every gauge the board and the scorecard can print.
#[must_use]
pub fn all() -> Vec<&'static Spec> {
    crate::fold::board::SPECS
        .iter()
        .chain(crate::fold::scorecard::SPECS)
        .copied()
        .collect()
}

/// The gauge an output path is an instance of, in the board's output or
/// the scorecard's.
#[must_use]
pub fn spec_at(board: bool, path: &str) -> Option<&'static Spec> {
    all()
        .into_iter()
        .filter(|s| matches!(s.scope, Scope::Board | Scope::Row) == board)
        .find(|s| s.at(path))
}

/// Builds gauges against one ledger's coverage, over one window and instant.
pub struct Gauger<'a> {
    pub coverage: &'a Coverage,
    pub from: Timestamp,
    pub to: Timestamp,
}

impl Gauger<'_> {
    /// The gauge `spec` for `seat` (`None`: every seat), its value `v` when
    /// every kind it reads is covered; `origin` is where a [`Span::Origin`]
    /// gauge reads from (`None`: it cannot be known).
    pub fn gauge<T>(
        &self,
        spec: &Spec,
        seat: Option<&str>,
        origin: Option<Timestamp>,
        v: T,
    ) -> Gauge<T> {
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
                                spec.field
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

    /// A gauge computed over `parent`'s value (findings over the goals
    /// held): unknown with the parent's reason when the parent is, else as
    /// [`Self::gauge`]. Its spec must name the parent in `via`.
    pub fn over<U, T>(
        &self,
        parent: &Gauge<U>,
        spec: &Spec,
        seat: Option<&str>,
        origin: Option<Timestamp>,
        v: T,
    ) -> Gauge<T> {
        if spec.via.is_empty() {
            return Gauge::unknown(format!(
                "{} is not declared as computed over a gauge",
                spec.field
            ));
        }
        match parent.reason() {
            Some(why) => Gauge::unknown(why.to_owned()),
            None => self.gauge(spec, seat, origin, v),
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
    const KINDLESS: &[(&str, &str)] = &[("goals.*.used", "wall-clock")];

    #[test]
    fn a_gauge_that_reads_no_kind_is_declared() {
        for spec in all() {
            if spec.kinds.is_empty() {
                assert!(
                    KINDLESS.contains(&(spec.field, spec.variant)),
                    "{} reads no kind",
                    spec.field
                );
            }
        }
    }

    #[test]
    fn every_gauge_has_its_own_path() {
        let mut paths: Vec<(bool, &str, &str)> = all()
            .iter()
            .map(|s| (matches!(s.scope, Scope::Card), s.field, s.variant))
            .collect();
        let n = paths.len();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), n);
    }

    #[test]
    fn a_path_matches_its_gauge_at_every_root() {
        const CARD: Spec = Spec::new(Scope::Card, "landings", &["land"], Span::Window);
        const USED: Spec = Spec::new(Scope::Row, "goals.*.used", &[], Span::Origin);
        assert!(CARD.at("seats.x-1a.landings"));
        assert!(CARD.at("total.landings"));
        assert!(CARD.at("unattributed.landings"));
        assert!(!CARD.at("flow.landings"));
        assert!(USED.at("seats.3.goals.0.used"));
        assert_eq!(USED.name(), "budget");
    }
}
