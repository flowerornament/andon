//! The one type every adapter emits (founding design §4).
//!
//! An event records a fact an artifact holds: a landing, a verdict, a run, a
//! message. Joins between events are by id, never by position or by name, so
//! the fold can combine sources it has never seen together.

use std::collections::BTreeMap;
use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// What happened. `Other` keeps facts the fold does not interpret yet, so an
/// adapter can record them without a code change here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// A change reached the trunk.
    Land,
    /// A change was frozen for review.
    Freeze,
    /// A commit exists; `t` is when its change started (author time).
    Commit,
    /// A reviewer's verdict on a change.
    Verdict,
    /// A local gate run (the land gate, a check).
    GateRun,
    /// A runner's run (Ferry, CI).
    Run,
    /// A measurement recorded against a commit.
    Measurement,
    /// A message between seats.
    Message,
    /// A tracker item was created.
    Create,
    /// A seat claimed a tracker item.
    Claim,
    /// A tracker item was closed.
    Close,
    /// A span in which a session made tool calls.
    SessionActive,
    /// A session started a subagent.
    Spawn,
    /// What a source let andon see: a span it covers, or a gap it could not
    /// read (see `.design/2026-10-14-coverage.md`).
    Coverage,
    /// Anything else, as `other:<name>`.
    Other(String),
}

impl Kind {
    /// The wire name, as it appears in the ledger.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Land => "land",
            Self::Freeze => "freeze",
            Self::Commit => "commit",
            Self::Verdict => "verdict",
            Self::GateRun => "gate_run",
            Self::Run => "run",
            Self::Measurement => "measurement",
            Self::Message => "message",
            Self::Create => "create",
            Self::Claim => "claim",
            Self::Close => "close",
            Self::SessionActive => "session_active",
            Self::Spawn => "spawn",
            Self::Coverage => "coverage",
            Self::Other(name) => name,
        }
    }

    /// Parse a wire name. Unknown names become `Other`, prefixed if needed.
    #[must_use]
    pub fn parse(name: &str) -> Self {
        match name {
            "land" => Self::Land,
            "freeze" => Self::Freeze,
            "commit" => Self::Commit,
            "verdict" => Self::Verdict,
            "gate_run" => Self::GateRun,
            "run" => Self::Run,
            "measurement" => Self::Measurement,
            "message" => Self::Message,
            "create" => Self::Create,
            "claim" => Self::Claim,
            "close" => Self::Close,
            "session_active" => Self::SessionActive,
            "spawn" => Self::Spawn,
            "coverage" => Self::Coverage,
            other if other.starts_with("other:") => Self::Other(other.to_owned()),
            other => Self::Other(format!("other:{other}")),
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Kind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::parse(&name))
    }
}

/// How it ended, when the artifact records an ending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
    Go,
    Return,
    Refused,
    Stopped,
    Unknown,
}

impl Outcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Go => "go",
            Self::Return => "return",
            Self::Refused => "refused",
            Self::Stopped => "stopped",
            Self::Unknown => "unknown",
        }
    }
}

/// The ids an event carries. Every join in the fold goes through one of these.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Ids {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critic_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Verdict ids a landing cites (a `Critic:` trailer per verdict).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cites: Vec<String>,
}

/// The start of time for coverage: a source that read all its history.
pub const ALL_HISTORY: Timestamp = Timestamp::UNIX_EPOCH;

/// A span a source covers: for `covers`, every fact it holds between `from`
/// and `to` (open when `None`) is in the ledger. `seat` scopes it to one
/// seat's label; `None` covers every seat.
#[must_use]
pub fn coverage(
    from: Timestamp,
    to: Option<Timestamp>,
    covers: &[&str],
    source: impl Into<String>,
    seat: Option<&str>,
) -> Event {
    let mut e = Event::new(from, Kind::Coverage, source);
    e.t_end = to;
    e.actor = seat.map(str::to_owned);
    e.outcome = Some(Outcome::Pass);
    e.set_attr(
        "covers",
        covers.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
    );
    e
}

/// A gap: a store held facts of `covers` that could not be read, and why.
#[must_use]
pub fn gap(
    at: Timestamp,
    covers: &[&str],
    source: impl Into<String>,
    seat: Option<&str>,
    reason: impl Into<String>,
) -> Event {
    let mut e = Event::new(at, Kind::Coverage, source);
    e.actor = seat.map(str::to_owned);
    e.outcome = Some(Outcome::Fail);
    e.set_attr(
        "covers",
        covers.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
    );
    e.set_attr("reason", reason.into());
    e
}

/// One fact from one artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub t: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_end: Option<Timestamp>,
    /// The seat, once resolved. Adapters leave it empty; the seat resolver
    /// fills it from `actor` and the ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    /// Who the source says did it, verbatim: a trailer value, a tracker actor,
    /// a workspace name, a message label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub kind: Kind,
    /// The goal the event is about, when the source names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default)]
    pub ids: Ids,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    /// The path or command that produced it.
    pub source: String,
    /// Source-specific facts the fold reads by name (recipe, repo, reason,
    /// message kind and recipients). Sorted, so serialization is stable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, Value>,
}

impl Event {
    /// A bare event; fill the rest with struct update or the builders below.
    #[must_use]
    pub fn new(t: Timestamp, kind: Kind, source: impl Into<String>) -> Self {
        Self {
            t,
            t_end: None,
            seat: None,
            actor: None,
            kind,
            subject: None,
            ids: Ids::default(),
            outcome: None,
            source: source.into(),
            attrs: BTreeMap::new(),
        }
    }

    /// A string attribute, if present.
    #[must_use]
    pub fn attr_str(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).and_then(Value::as_str)
    }

    /// A string-list attribute; empty when absent.
    #[must_use]
    pub fn attr_strs(&self, key: &str) -> Vec<&str> {
        self.attrs
            .get(key)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    /// Set an attribute when the value is meaningful (not null, not empty).
    pub fn set_attr(&mut self, key: &str, value: impl Into<Value>) {
        let value = value.into();
        let empty = match &value {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            Value::Bool(_) | Value::Number(_) => false,
        };
        if !empty {
            self.attrs.insert(key.to_owned(), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_round_trips_through_its_wire_name() {
        for kind in [
            Kind::Land,
            Kind::GateRun,
            Kind::SessionActive,
            Kind::Other("other:desk_create".to_owned()),
        ] {
            assert_eq!(Kind::parse(kind.as_str()), kind);
        }
        assert_eq!(
            Kind::parse("compaction"),
            Kind::Other("other:compaction".to_owned())
        );
    }

    #[test]
    fn set_attr_drops_empty_values() {
        let mut event = Event::new(Timestamp::UNIX_EPOCH, Kind::Run, "test");
        event.set_attr("reason", Value::Null);
        event.set_attr("tail", "");
        event.set_attr("recipe", "check");
        assert_eq!(event.attrs.len(), 1);
        assert_eq!(event.attr_str("recipe"), Some("check"));
    }
}
