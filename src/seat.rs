//! Seat identity: the one join every adapter needs (founding design §5).
//!
//! Adapters record who a source says acted (`actor`, verbatim) and the ids
//! they saw. Resolution then fills `seat`, in order of strength:
//!
//! 1. the actor is a configured alias, or starts with a configured prefix;
//! 2. the event's session belongs to a seat (learned from session spans,
//!    then from other events whose seat is already known);
//! 3. the actor is a workspace whose sessions belong to one seat (a seat's
//!    extra desks, such as `x-native-gates`).
//!
//! With no `[seats]` configured, every actor is its own seat.

use std::collections::{BTreeMap, HashMap};

use crate::config::Config;
use crate::event::{Event, Kind};

/// Labels → seats, from configuration.
#[derive(Debug, Clone, Default)]
pub struct Resolver {
    aliases: HashMap<String, String>,
    /// Longest prefix first, so `x-1a-par` is not taken by `x-1`.
    prefixes: Vec<(String, String)>,
    cwds: Vec<(String, String)>,
    open: bool,
}

impl Resolver {
    #[must_use]
    pub fn new(config: &Config) -> Self {
        let mut aliases = HashMap::new();
        let mut prefixes = Vec::new();
        let mut cwds = Vec::new();
        for (seat, spec) in &config.seats {
            aliases.insert(seat.clone(), seat.clone());
            for alias in &spec.aliases {
                aliases.insert(alias.clone(), seat.clone());
            }
            for prefix in &spec.prefixes {
                prefixes.push((prefix.clone(), seat.clone()));
            }
            for cwd in &spec.cwd {
                let path = config.path(cwd);
                cwds.push((path.to_string_lossy().into_owned(), seat.clone()));
            }
        }
        prefixes.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        Self {
            aliases,
            prefixes,
            cwds,
            open: config.seats.is_empty(),
        }
    }

    /// The seat a label names, by configuration alone.
    #[must_use]
    pub fn label(&self, label: &str) -> Option<String> {
        if self.open {
            return Some(label.to_owned());
        }
        if let Some(seat) = self.aliases.get(label) {
            return Some(seat.clone());
        }
        self.prefixes
            .iter()
            .find(|(prefix, _)| label.starts_with(prefix.as_str()))
            .map(|(_, seat)| seat.clone())
    }

    /// The seat a working directory belongs to.
    #[must_use]
    pub fn cwd(&self, cwd: &str) -> Option<String> {
        self.cwds
            .iter()
            .find(|(dir, _)| cwd == dir || cwd.starts_with(&format!("{dir}/")))
            .map(|(_, seat)| seat.clone())
    }

    /// Fill `seat` on every event that can be resolved, strongest join first.
    pub fn resolve(&self, events: &mut [Event]) {
        self.by_config(events);
        let session_seat = by_session(events);
        let desk_seat = by_desk(events, &session_seat);
        by_shared_ids(events);
        goal_by_change(events);
        self.recipients(events, &desk_seat);
    }

    /// Step 1: A configured cwd, alias or prefix.
    fn by_config(&self, events: &mut [Event]) {
        for event in events.iter_mut().filter(|e| e.seat.is_none()) {
            let by_cwd = event.attr_str("cwd").and_then(|cwd| self.cwd(cwd));
            event.seat = by_cwd.or_else(|| event.actor.as_deref().and_then(|a| self.label(a)));
        }
    }

    /// Step 6: Assignees and message recipients, by label (or by a desk known to
    /// belong to a seat).
    fn recipients(&self, events: &mut [Event], desk_seat: &HashMap<String, String>) {
        for event in events.iter_mut() {
            if matches!(event.kind, Kind::Create | Kind::Claim)
                && let Some(seat) = event.attr_str("assignee").and_then(|a| self.label(a))
            {
                event.set_attr("assignee_seat", seat);
            }
            if event.kind != Kind::Message {
                continue;
            }
            let mut seats: Vec<String> = event
                .attr_strs("to")
                .into_iter()
                .filter_map(|label| self.label(label).or_else(|| desk_seat.get(label).cloned()))
                .collect();
            seats.sort();
            seats.dedup();
            event.set_attr("to_seats", seats);
        }
    }
}

/// For each key, the seat most pairs name.
fn tally(pairs: impl Iterator<Item = (String, String)>) -> HashMap<String, String> {
    let mut counts: HashMap<String, BTreeMap<String, usize>> = HashMap::new();
    for (key, seat) in pairs {
        *counts.entry(key).or_default().entry(seat).or_default() += 1;
    }
    counts
        .into_iter()
        .filter_map(|(key, c)| most(&c).map(|seat| (key, seat)))
        .collect()
}

/// Step 2: Sessions: spans are the authority; a session no span names takes the
/// seat most of its resolved events have.
fn by_session(events: &mut [Event]) -> HashMap<String, String> {
    let pairs = |spans: bool| {
        events
            .iter()
            .filter(move |e| (e.kind == Kind::SessionActive) == spans)
            .filter_map(|e| Some((e.ids.session.clone()?, e.seat.clone()?)))
            .collect::<Vec<_>>()
    };
    let from_spans = tally(pairs(true).into_iter());
    let mut session_seat = tally(pairs(false).into_iter());
    session_seat.retain(|session, _| !from_spans.contains_key(session));
    session_seat.extend(from_spans);
    for event in events.iter_mut() {
        if event.seat.is_none()
            && let Some(seat) = event.ids.session.as_ref().and_then(|s| session_seat.get(s))
        {
            event.seat = Some(seat.clone());
        }
    }
    session_seat
}

/// Step 3: A workspace whose sessions belong to one seat (a seat's task-named
/// extra desks).
fn by_desk(
    events: &mut [Event],
    session_seat: &HashMap<String, String>,
) -> HashMap<String, String> {
    let desk_seat = tally(events.iter().filter_map(|e| {
        let seat = session_seat.get(e.ids.session.as_ref()?)?;
        Some((e.actor.clone()?, seat.clone()))
    }));
    for event in events.iter_mut() {
        if event.seat.is_none()
            && let Some(seat) = event.actor.as_ref().and_then(|a| desk_seat.get(a))
        {
            event.seat = Some(seat.clone());
        }
    }
    desk_seat
}

/// Step 4: A change or a commit carries its seat to events that share it: a
/// landing attributed only to an author's name takes its pull request's
/// seat; a run whose source names no one takes its commit's. A run whose
/// source names a workspace that is no seat (a shared desk) stays
/// unattributed: who ran it is a fact, whose commit it measured is not the
/// same fact.
fn by_shared_ids(events: &mut [Event]) {
    for key in [IdKey::Change, IdKey::Commit] {
        let known = tally(
            events
                .iter()
                .filter_map(|e| Some((key.of(e)?.to_owned(), e.seat.clone()?))),
        );
        for event in events.iter_mut() {
            let anonymous = event.actor.is_none()
                || (matches!(key, IdKey::Change)
                    && matches!(event.kind, Kind::Land | Kind::Commit));
            if event.seat.is_none()
                && anonymous
                && let Some(seat) = key.of(event).and_then(|id| known.get(id))
            {
                event.seat = Some(seat.clone());
            }
        }
    }
}

/// Step 5: A landing that names no goal takes the goal its change closes.
fn goal_by_change(events: &mut [Event]) {
    let mut change_goal: HashMap<String, String> = HashMap::new();
    for event in events.iter() {
        if let (Some(change), Some(goal)) = (&event.ids.change, &event.subject) {
            change_goal
                .entry(change.clone())
                .or_insert_with(|| goal.clone());
        }
    }
    for event in events.iter_mut() {
        if event.subject.is_none()
            && matches!(event.kind, Kind::Land | Kind::Commit | Kind::Verdict)
            && let Some(goal) = event.ids.change.as_ref().and_then(|c| change_goal.get(c))
        {
            event.subject = Some(goal.clone());
            event.ids.goal = Some(goal.clone());
        }
    }
}

#[derive(Clone, Copy)]
enum IdKey {
    Change,
    Commit,
}

impl IdKey {
    fn of(self, event: &Event) -> Option<&str> {
        match self {
            Self::Change => event.ids.change.as_deref(),
            Self::Commit => event.ids.commit.as_deref(),
        }
    }
}

/// The most frequent key; ties go to the first in order, so it is stable.
fn most(counts: &BTreeMap<String, usize>) -> Option<String> {
    let mut best: Option<(&String, usize)> = None;
    for (key, &n) in counts {
        if best.is_none_or(|(_, m)| n > m) {
            best = Some((key, n));
        }
    }
    best.map(|(k, _)| k.clone())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::time::parse_instant;

    fn config() -> Config {
        Config::parse(
            r#"
[seats.x-1a]
prefixes = ["x-1a-"]
[seats.x-1b]
aliases = ["1b-bot"]
"#,
            Path::new("/"),
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn event(kind: Kind, actor: &str, session: Option<&str>) -> Event {
        let mut e = Event::new(
            parse_instant("2026-10-11T00:00:00Z").unwrap_or_else(|e| panic!("{e}")),
            kind,
            "test",
        );
        e.actor = Some(actor.to_owned());
        e.ids.session = session.map(str::to_owned);
        e
    }

    #[test]
    fn labels_resolve_by_alias_then_longest_prefix() {
        let r = Resolver::new(&config());
        assert_eq!(r.label("x-1a").as_deref(), Some("x-1a"));
        assert_eq!(r.label("x-1a-par").as_deref(), Some("x-1a"));
        assert_eq!(r.label("1b-bot").as_deref(), Some("x-1b"));
        assert_eq!(r.label("Flower Ornament"), None);
    }

    #[test]
    fn sessions_and_desks_carry_identity_to_unlabelled_events() {
        let r = Resolver::new(&config());
        let mut events = vec![
            event(Kind::SessionActive, "x-1a", Some("s1")),
            // a shared tracker account, but the session is known
            event(Kind::Verdict, "Flower Ornament", Some("s1")),
            // a desk named for its task, landing from the seat's session
            event(Kind::Land, "x-native-gates", Some("s1")),
            // the same desk later, with no session at all
            event(Kind::GateRun, "x-native-gates", None),
            event(Kind::Message, "y-1a", None),
        ];
        r.resolve(&mut events);
        let seats: Vec<Option<&str>> = events.iter().map(|e| e.seat.as_deref()).collect();
        assert_eq!(
            seats,
            vec![Some("x-1a"), Some("x-1a"), Some("x-1a"), Some("x-1a"), None]
        );
    }

    #[test]
    fn with_no_seats_configured_each_actor_is_a_seat() {
        let r = Resolver::new(&Config::default());
        assert_eq!(r.label("andon-1b").as_deref(), Some("andon-1b"));
    }
}
