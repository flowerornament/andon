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

/// What an attribute holds, so a writer cannot change a key's meaning by
/// changing its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A string.
    Text,
    /// A list of strings.
    Texts,
    /// An integer.
    Int,
    /// `true` or `false`.
    Bool,
    /// An object of string keys to lists of strings.
    Map,
}

impl Shape {
    /// The shape's name, as the contract documents it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Texts => "texts",
            Self::Int => "int",
            Self::Bool => "bool",
            Self::Map => "map",
        }
    }

    /// Whether a value has this shape.
    #[must_use]
    pub fn admits(self, value: &Value) -> bool {
        let texts = |v: &Value| v.as_array().is_some_and(|a| a.iter().all(Value::is_string));
        match self {
            Self::Text => value.is_string(),
            Self::Texts => texts(value),
            Self::Int => value.is_i64() || value.is_u64(),
            Self::Bool => value.is_boolean(),
            Self::Map => value.as_object().is_some_and(|o| o.values().all(texts)),
        }
    }
}

/// A declared attribute key: one name, one shape, one meaning. Adapters
/// write and folds read attributes only through these, so the contract
/// between them is this list (`attr::ALL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attr {
    pub key: &'static str,
    pub shape: Shape,
    pub doc: &'static str,
}

macro_rules! declare {
    ($($(#[doc = $doc:literal])+ $name:ident = $key:literal, $shape:ident;)+) => {
        $(
            $(#[doc = $doc])+
            pub const $name: Attr = Attr {
                key: $key,
                shape: Shape::$shape,
                doc: concat!($($doc),+),
            };
        )+
        /// Every declared key, in the order of this list.
        pub const ALL: &[Attr] = &[$($name),+];
    };
}

/// The declared attribute keys. A key absent here cannot be written by an
/// adapter or read by a fold; an undeclared key in a kept ledger or an
/// external adapter's line is carried through and never read.
pub mod attr {
    use super::{Attr, Shape};

    declare! {
        /// The external adapter that emitted the event, by its configured name.
        ADAPTER = "adapter", Text;
        /// The tracker's assignee on a create or claim, verbatim.
        ASSIGNEE = "assignee", Text;
        /// The seat the assignee resolves to (written by the seat resolver).
        ASSIGNEE_SEAT = "assignee_seat", Text;
        /// A commit's author name, as git records it.
        AUTHOR = "author", Text;
        /// The branch a commit or run was on.
        BRANCH = "branch", Text;
        /// A goal's Budget section, verbatim.
        BUDGET = "budget", Text;
        /// Tool calls made in a session's active span.
        CALLS = "calls", Int;
        /// The change ids a verdict names.
        CHANGES = "changes", Texts;
        /// The tracker field that timed a claim whose start was not recorded.
        CLAIM_TIME = "claim_time", Text;
        /// Why a tracker item was closed, as the tracker records it.
        CLOSE_REASON = "close_reason", Text;
        /// A tracker comment's id.
        COMMENT = "comment", Text;
        /// The kinds a coverage event vouches for (`kind` or `state:kind`).
        COVERS = "covers", Texts;
        /// The working directory a session ran in.
        CWD = "cwd", Text;
        /// What the source's own description says it is: a jj operation's
        /// description, a subagent's task.
        DESCRIPTION = "description", Text;
        /// A run's exit code.
        EXIT_CODE = "exit_code", Int;
        /// Where a failed run's output tail is kept (a path).
        FAILURE_TAIL = "failure_tail", Text;
        /// Ferry's own outcome word, verbatim.
        FERRY_OUTCOME = "ferry_outcome", Text;
        /// Why a source could not be read (a coverage gap).
        GAP_REASON = "gap_reason", Text;
        /// Goal ids a message or pull request names in its text.
        GOALS = "goals", Texts;
        /// The harness a session ran under (claude, codex).
        HARNESS = "harness", Text;
        /// A message's first line.
        HEADLINE = "headline", Text;
        /// The tracker item's type (feature, bug, task, epic, ...).
        ITEM_TYPE = "item_type", Text;
        /// How a verdict joins its landing: `change` joins by change id.
        JOIN = "join", Text;
        /// What timed a landing: `op` (the trunk move) or `committer`.
        LAND_TIME = "land_time", Text;
        /// The commit reached the trunk.
        LANDED = "landed", Bool;
        /// Where a failed run's logs are kept (a URL).
        LOGS = "logs", Text;
        /// The machine a run ran on.
        MACHINE = "machine", Text;
        /// When a pull request merged, as GitHub records it.
        MERGED_AT = "merged_at", Text;
        /// A message's kind in its messenger (go, ask, done, fyi, ...).
        MESSAGE_KIND = "message_kind", Text;
        /// Bytes of output a run produced.
        OUTPUT_BYTES = "output_bytes", Int;
        /// A goal's Pool section, verbatim.
        POOL = "pool", Text;
        /// The id of the message this one answers.
        RE = "re", Text;
        /// The recipe a run or gate ran (`land` is the land gate).
        RECIPE = "recipe", Text;
        /// The repository a run ran in (written by external adapters).
        REPO = "repo", Text;
        /// Who gave a verdict.
        REVIEWER = "reviewer", Text;
        /// The runner's own reason for a run's ending, verbatim.
        RUN_REASON = "run_reason", Text;
        /// A commit subject, an item or pull request title.
        TITLE = "title", Text;
        /// A message's recipients, verbatim.
        TO = "to", Texts;
        /// The seats a message's recipients resolve to (seat resolver).
        TO_SEATS = "to_seats", Texts;
        /// Trailers a project keeps on landings (`adapters.jj.trailers.keep`),
        /// by lowercased name.
        TRAILERS = "trailers", Map;
        /// The jj workspace a landing was made from.
        WORKSPACE = "workspace", Text;
    }
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
        attr::COVERS,
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
        attr::COVERS,
        covers.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
    );
    e.set_attr(attr::GAP_REASON, reason.into());
    e
}

/// A gap at one instant: a fact the store held at `at`, unreadable. It
/// stands only over spans that hold `at` (a span of one instant, as a
/// current-state read is); a gap with no known time stands everywhere.
#[must_use]
pub fn gap_at(
    at: Timestamp,
    covers: &[&str],
    source: impl Into<String>,
    seat: Option<&str>,
    reason: impl Into<String>,
) -> Event {
    let mut e = gap(at, covers, source, seat, reason);
    e.t_end = Some(at);
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
    /// Source-specific facts, by declared key (`attr`). Private, so every
    /// write and read goes through a declared key. Sorted, so serialization
    /// is stable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    attrs: BTreeMap<String, Value>,
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
    pub fn attr_str(&self, attr: Attr) -> Option<&str> {
        self.attrs.get(attr.key).and_then(Value::as_str)
    }

    /// A string-list attribute; empty when absent.
    #[must_use]
    pub fn attr_strs(&self, attr: Attr) -> Vec<&str> {
        self.attrs
            .get(attr.key)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    /// Whether the attribute is present.
    #[must_use]
    pub fn has_attr(&self, attr: Attr) -> bool {
        self.attrs.contains_key(attr.key)
    }

    /// Remove an attribute.
    pub fn remove_attr(&mut self, attr: Attr) {
        self.attrs.remove(attr.key);
    }

    /// Set an attribute when the value is meaningful (not null, not empty).
    /// A value of the wrong shape is a bug in the writer: it fails a debug
    /// build (every test) and is dropped in a release.
    pub fn set_attr(&mut self, attr: Attr, value: impl Into<Value>) {
        let value = value.into();
        let empty = match &value {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            Value::Bool(_) | Value::Number(_) => false,
        };
        if empty {
            return;
        }
        debug_assert!(
            attr.shape.admits(&value),
            "attribute {} is {}, not {value}",
            attr.key,
            attr.shape.as_str()
        );
        if attr.shape.admits(&value) {
            self.attrs.insert(attr.key.to_owned(), value);
        }
    }

    /// The first declared key whose value has the wrong shape: a writer
    /// outside this crate (an external adapter, a hand-edited ledger) broke
    /// the contract.
    #[must_use]
    pub fn misshapen(&self) -> Option<Attr> {
        attr::ALL
            .iter()
            .find(|a| self.attrs.get(a.key).is_some_and(|v| !a.shape.admits(v)))
            .copied()
    }

    /// Keys this event carries that are not declared: an external adapter's
    /// or an older andon's facts, carried through and never read.
    #[must_use]
    pub fn undeclared(&self) -> Vec<&str> {
        self.attrs
            .keys()
            .map(String::as_str)
            .filter(|k| attr::ALL.iter().all(|a| a.key != *k))
            .collect()
    }

    /// Bring an event read from a kept ledger or an external adapter up to
    /// the declared keys. Before 0.3.0 one key, `reason`, carried four
    /// meanings; it is renamed by the kind of event that carried it.
    pub fn upgrade(&mut self) {
        let Some(reason) = self.attrs.get("reason").cloned() else {
            return;
        };
        let to = match self.kind {
            Kind::Coverage => attr::GAP_REASON,
            Kind::Close => attr::CLOSE_REASON,
            Kind::Run | Kind::GateRun if self.attr_str(attr::MACHINE) == Some("github-actions") => {
                attr::LOGS
            }
            Kind::Run | Kind::GateRun => attr::RUN_REASON,
            _ => return,
        };
        if !self.has_attr(to) && to.shape.admits(&reason) {
            self.attrs.remove("reason");
            self.attrs.insert(to.key.to_owned(), reason);
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
        event.set_attr(attr::RUN_REASON, Value::Null);
        event.set_attr(attr::FAILURE_TAIL, "");
        event.set_attr(attr::RECIPE, "check");
        assert_eq!(event.attrs.len(), 1);
        assert_eq!(event.attr_str(attr::RECIPE), Some("check"));
    }

    #[test]
    fn every_key_is_declared_once_with_a_meaning() {
        let mut keys: Vec<&str> = attr::ALL.iter().map(|a| a.key).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "a key is declared twice");
        for a in attr::ALL {
            assert!(!a.doc.trim().is_empty(), "{} has no meaning", a.key);
            assert_eq!(a.key, a.key.to_lowercase(), "{} is not snake case", a.key);
        }
    }

    #[test]
    #[should_panic(expected = "attribute recipe is text")]
    fn a_value_of_the_wrong_shape_is_refused() {
        let mut event = Event::new(Timestamp::UNIX_EPOCH, Kind::Run, "test");
        event.set_attr(attr::RECIPE, 3);
    }

    #[test]
    fn an_undeclared_key_is_carried_and_named() {
        let line = r#"{"t":"2026-10-12T00:00:00Z","kind":"run","source":"x","attrs":{"note":"n","recipe":"land"}}"#;
        let event: Event = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(event.undeclared(), vec!["note"]);
        assert_eq!(event.attr_str(attr::RECIPE), Some("land"));
    }

    #[test]
    fn the_old_reason_is_renamed_by_its_meaning() {
        let read = |kind: &str, extra: &str| {
            let line = format!(
                r#"{{"t":"2026-10-12T00:00:00Z","kind":"{kind}","source":"x","attrs":{{"reason":"why"{extra}}}}}"#
            );
            let mut event: Event = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}"));
            event.upgrade();
            event
        };
        assert_eq!(read("coverage", "").attr_str(attr::GAP_REASON), Some("why"));
        assert_eq!(read("close", "").attr_str(attr::CLOSE_REASON), Some("why"));
        assert_eq!(read("run", "").attr_str(attr::RUN_REASON), Some("why"));
        let gh = read("run", r#","machine":"github-actions""#);
        assert_eq!(gh.attr_str(attr::LOGS), Some("why"));
        assert_eq!(gh.undeclared(), Vec::<&str>::new());
        assert_eq!(read("message", "").undeclared(), vec!["reason"]);
    }
}
