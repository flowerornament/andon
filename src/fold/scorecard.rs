//! `andon scorecard`: the four laws of flow, per seat, over a window
//! (founding design §2). Every number is a count or a median over ledger
//! events; nothing reads the clock.
//!
//! | law           | gauge here                                            |
//! |---------------|-------------------------------------------------------|
//! | Little        | goals in progress per seat at the window's end; flow   |
//! |               | times (change start → landing, → first verdict, gate) |
//! | Branching     | items created per item closed                         |
//! | Rework        | runs per landing; failed runs that kept no reason      |
//! | Single server | messages into the hub; decisions that name no goal     |

use std::collections::{BTreeMap, BTreeSet, HashMap};

use jiff::Timestamp;
use serde::Serialize;

use crate::config::Config;
use crate::event::{Event, Kind, Outcome};
use crate::fold::coverage::Coverage;
use crate::fold::gauge::{Gauge, Gauger};
use crate::fold::{Index, median, ratio, round2};
use crate::ledger::Ledger;
use crate::time::{Window, minutes};

/// A scorecard over one window. Every gauge is a [`Gauge`]: known, or
/// unknown with the reason, never a 0 where the ledger could not look.
#[derive(Debug, Clone, Serialize)]
pub struct Scorecard {
    pub schema: u32,
    pub from: Timestamp,
    pub to: Timestamp,
    /// The end asked for, when it lay after the capture instant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asked_to: Option<Timestamp>,
    pub days: f64,
    pub seats: BTreeMap<String, SeatCard>,
    /// Every event in the window, attributed or not.
    pub total: SeatCard,
    /// Events no seat could be resolved for.
    pub unattributed: SeatCard,
    pub flow: Flow,
    pub tracker: Tracker,
    pub hub: Gauge<Option<Hub>>,
    pub joins: Joins,
    /// Runs per repository named in the receipts.
    pub runs_by_repo: Gauge<BTreeMap<String, RunCounts>>,
}

/// The gauges for one seat, or a total.
#[derive(Debug, Clone, Serialize)]
pub struct SeatCard {
    /// A seat from `[seats]`, shown even when idle; others only when active.
    #[serde(skip)]
    pub configured: bool,
    #[serde(skip)]
    pub active: bool,
    pub landings: Gauge<usize>,
    pub runs: Gauge<RunCounts>,
    /// Runs per landing (Rework: expected tries = 1/p). `null` with no landing.
    pub runs_per_landing: Gauge<Option<f64>>,
    pub gate_runs: Gauge<usize>,
    pub verdicts_go: Gauge<usize>,
    pub verdicts_return: Gauge<usize>,
    pub messages_sent: Gauge<usize>,
    pub messages_received: Gauge<usize>,
    /// Sent messages that name a goal.
    pub messages_naming_goal: Gauge<usize>,
    /// Sent decision kinds (ask, go, ruling…) that name no goal.
    pub decisions_without_goal: Gauge<usize>,
    pub items_created: Gauge<usize>,
    /// Goals held claimed and open at the window's end (Little's L).
    pub wip_at_end: Gauge<usize>,
    /// Hours with session activity.
    pub active_hours: Gauge<f64>,
}

/// Flow times, in minutes (Little: W).
#[derive(Debug, Clone, Serialize)]
pub struct Flow {
    pub lead_median_min: Gauge<Option<f64>>,
    pub lead_n: Gauge<usize>,
    pub review_median_min: Gauge<Option<f64>>,
    pub review_n: Gauge<usize>,
    pub gate_median_min: Gauge<Option<f64>>,
    pub gate_n: Gauge<usize>,
}

/// Tracker state (Branching).
#[derive(Debug, Clone, Serialize)]
pub struct Tracker {
    pub created: Gauge<usize>,
    pub closed: Gauge<usize>,
    /// created / closed; the backlog is bounded only while this stays below 1.
    pub branching: Gauge<Option<f64>>,
    pub open_at_end: Gauge<usize>,
    pub unowned_at_end: Gauge<usize>,
}

/// How well the joins held in this window (founding design §4).
#[derive(Debug, Clone, Serialize)]
pub struct Joins {
    pub landings: Gauge<usize>,
    pub landing_session: Gauge<usize>,
    pub landing_session_seen: Gauge<usize>,
    pub landings_citing_verdict: Gauge<usize>,
    pub cites: Gauge<usize>,
    pub cites_resolved: Gauge<usize>,
    pub runs: Gauge<usize>,
    pub runs_attributed: Gauge<usize>,
}

/// What the fold counted, before coverage decides what may be said.
struct Raw {
    seats: BTreeMap<String, Tally>,
    total: Tally,
    unattributed: Tally,
    flow: FlowRaw,
    tracker: TrackerRaw,
    hub: Option<Hub>,
    joins: JoinsRaw,
    runs_by_repo: BTreeMap<String, RunCounts>,
    wip_total: usize,
    wip_unattributed: usize,
}

/// The scope the unattributed card is gauged at: no seat's own coverage
/// applies to it, every seat's shared coverage does.
const UNATTRIBUTED: &str = "(unattributed)";

/// The only way a scorecard is made: every value through its registered
/// gauge, so coverage decides what is known.
fn publish(raw: Raw, coverage: &Coverage, window: Window) -> Scorecard {
    let g = Gauger {
        coverage,
        from: window.from,
        to: window.to,
    };
    let card = |t: Tally, wip: usize, seat: Option<&str>| SeatCard {
        configured: t.configured,
        active: t.active(),
        landings: g.gauge("card.landings", seat, None, t.landings),
        runs_per_landing: g.gauge("card.runs_per_landing", seat, None, t.runs_per_landing),
        runs: g.gauge("card.runs", seat, None, t.runs),
        gate_runs: g.gauge("card.gate_runs", seat, None, t.gate_runs),
        verdicts_go: g.gauge("card.verdicts_go", seat, None, t.verdicts_go),
        verdicts_return: g.gauge("card.verdicts_return", seat, None, t.verdicts_return),
        messages_sent: g.gauge("card.messages_sent", seat, None, t.messages_sent),
        messages_received: g.gauge("card.messages_received", seat, None, t.messages_received),
        messages_naming_goal: g.gauge(
            "card.messages_naming_goal",
            seat,
            None,
            t.messages_naming_goal,
        ),
        decisions_without_goal: g.gauge(
            "card.decisions_without_goal",
            seat,
            None,
            t.decisions_without_goal,
        ),
        items_created: g.gauge("card.items_created", seat, None, t.items_created),
        wip_at_end: g.gauge("card.wip_at_end", seat, None, wip),
        active_hours: g.gauge("card.active_hours", seat, None, t.active_hours),
    };
    let seats = raw
        .seats
        .into_iter()
        .map(|(s, t)| {
            let wip = t.wip_at_end;
            let c = card(t, wip, Some(s.as_str()));
            (s, c)
        })
        .collect();
    let f = raw.flow;
    let t = raw.tracker;
    let j = raw.joins;
    Scorecard {
        schema: 2,
        from: window.from,
        to: window.to,
        asked_to: window.asked_to,
        days: round2(window.length_days()),
        seats,
        total: card(raw.total, raw.wip_total, None),
        // whether a verdict could be attributed depends on change starts, as
        // a seat's does: the unattributed card reads what a seat's reads
        unattributed: card(raw.unattributed, raw.wip_unattributed, Some(UNATTRIBUTED)),
        flow: Flow {
            lead_median_min: g.gauge("flow.lead_median_min", None, None, f.lead_median_min),
            lead_n: g.gauge("flow.lead_n", None, None, f.lead_n),
            review_median_min: g.gauge("flow.review_median_min", None, None, f.review_median_min),
            review_n: g.gauge("flow.review_n", None, None, f.review_n),
            gate_median_min: g.gauge("flow.gate_median_min", None, None, f.gate_median_min),
            gate_n: g.gauge("flow.gate_n", None, None, f.gate_n),
        },
        tracker: Tracker {
            created: g.gauge("tracker.created", None, None, t.created),
            closed: g.gauge("tracker.closed", None, None, t.closed),
            branching: g.gauge("tracker.branching", None, None, t.branching),
            open_at_end: g.gauge("tracker.open_at_end", None, None, t.open_at_end),
            unowned_at_end: g.gauge("tracker.unowned_at_end", None, None, t.unowned_at_end),
        },
        hub: g.gauge("hub", None, None, raw.hub),
        joins: Joins {
            landings: g.gauge("joins.landings", None, None, j.landings),
            landing_session: g.gauge("joins.landing_session", None, None, j.landing_session),
            landing_session_seen: g.gauge(
                "joins.landing_session_seen",
                None,
                None,
                j.landing_session_seen,
            ),
            landings_citing_verdict: g.gauge(
                "joins.landings_citing_verdict",
                None,
                None,
                j.landings_citing_verdict,
            ),
            cites: g.gauge("joins.cites", None, None, j.cites),
            cites_resolved: g.gauge("joins.cites_resolved", None, None, j.cites_resolved),
            runs: g.gauge("joins.runs", None, None, j.runs),
            runs_attributed: g.gauge("joins.runs_attributed", None, None, j.runs_attributed),
        },
        runs_by_repo: g.gauge("runs_by_repo", None, None, raw.runs_by_repo),
    }
}

/// What the fold counted for one seat, or a total.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Tally {
    /// A seat from `[seats]`, shown even when idle; others only when active.
    #[serde(skip)]
    pub configured: bool,
    pub landings: usize,
    pub runs: RunCounts,
    /// Runs per landing (Rework: expected tries = 1/p). `None` with no landing.
    pub runs_per_landing: Option<f64>,
    pub gate_runs: usize,
    pub verdicts_go: usize,
    pub verdicts_return: usize,
    pub messages_sent: usize,
    pub messages_received: usize,
    /// Sent messages that name a goal.
    pub messages_naming_goal: usize,
    /// Sent decision kinds (ask, go, ruling…) that name no goal.
    pub decisions_without_goal: usize,
    pub items_created: usize,
    /// Goals this seat held claimed and open at the window's end (Little's L).
    pub wip_at_end: usize,
    /// Hours with session activity.
    pub active_hours: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RunCounts {
    pub total: usize,
    pub pass: usize,
    pub fail: usize,
    pub other: usize,
    /// Failed runs that kept neither a reason nor an output tail.
    pub fail_without_reason: usize,
}

/// Flow times, as counted, in minutes (Little: W).
#[derive(Debug, Clone, Default, Serialize)]
pub struct FlowRaw {
    /// Change start (author time) → landing.
    pub lead_median_min: Option<f64>,
    pub lead_n: usize,
    /// Change start → first verdict the landing cites.
    pub review_median_min: Option<f64>,
    pub review_n: usize,
    /// Passing land-gate runs, start → end.
    pub gate_median_min: Option<f64>,
    pub gate_n: usize,
}

/// Tracker state, as counted (Branching).
#[derive(Debug, Clone, Default, Serialize)]
pub struct TrackerRaw {
    pub created: usize,
    pub closed: usize,
    /// created / closed; the backlog is bounded only while this stays below 1.
    pub branching: Option<f64>,
    pub open_at_end: usize,
    pub unowned_at_end: usize,
}

/// The seat most messages go to (Single server).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Hub {
    pub seat: String,
    pub inbound: usize,
    pub inbound_per_day: f64,
    /// Decisions sent to the hub that name no goal.
    pub decisions_without_goal: usize,
    /// Median minutes from a message into the hub to the hub's first reply.
    pub reply_median_min: Option<f64>,
    pub reply_n: usize,
}

/// How well the joins held in this window (founding design §4).
#[derive(Debug, Clone, Default, Serialize)]
pub struct JoinsRaw {
    pub landings: usize,
    /// Landings whose session id is known.
    pub landing_session: usize,
    /// … and whose session appears in the sessions source.
    pub landing_session_seen: usize,
    /// Landings with a verdict: cited by trailer, or on their change.
    pub landings_citing_verdict: usize,
    /// Verdict citations on landings, and how many resolve to a verdict.
    pub cites: usize,
    pub cites_resolved: usize,
    pub runs: usize,
    pub runs_attributed: usize,
}

/// Fold a scorecard.
#[must_use]
pub fn scorecard(ledger: &Ledger, config: &Config, window: Window) -> Scorecard {
    let index = Index::new(ledger);
    let decisions: BTreeSet<&str> = config
        .messages
        .decision_kinds
        .iter()
        .map(String::as_str)
        .collect();
    let mut seats: BTreeMap<String, Tally> = config
        .seats
        .keys()
        .map(|s| {
            (
                s.clone(),
                Tally {
                    configured: true,
                    ..Tally::default()
                },
            )
        })
        .collect();
    let mut total = Tally::default();
    let mut unattributed = Tally::default();
    let mut tracker = TrackerRaw::default();

    for event in ledger.events().iter().filter(|e| window.contains(e.t)) {
        let seat_card = match &event.seat {
            Some(seat) => seats.entry(seat.clone()).or_default(),
            None => &mut unattributed,
        };
        apply(event, &decisions, &mut [seat_card, &mut total]);
        match event.kind {
            Kind::Message => {
                // a seat's messages to itself (or its own subseats) are not
                // load on it
                let own = event.seat.as_deref();
                for seat in event
                    .attr_strs("to_seats")
                    .into_iter()
                    .filter(|s| Some(*s) != own)
                {
                    seats.entry(seat.to_owned()).or_default().messages_received += 1;
                    total.messages_received += 1;
                }
            }
            Kind::Create => tracker.created += 1,
            Kind::Close => tracker.closed += 1,
            _ => {}
        }
    }
    active_hours(ledger, window, &mut seats, &mut total, &mut unattributed);
    let (joins, runs_by_repo) = joins(ledger, &index, window);

    // goals in progress at the end of the window
    let state = index.items_at(window.to);
    let mut wip_total = 0;
    let mut wip_unattributed = 0;
    for item in state.values() {
        if item.in_progress(config) {
            wip_total += 1;
            if item.owner.as_ref().is_none_or(|o| !seats.contains_key(o)) {
                wip_unattributed += 1;
            }
        }
    }
    for item in state.values() {
        if item.closed {
            continue;
        }
        tracker.open_at_end += 1;
        match &item.owner {
            None => tracker.unowned_at_end += 1,
            Some(owner) => {
                if item.in_progress(config)
                    && let Some(card) = seats.get_mut(owner)
                {
                    card.wip_at_end += 1;
                }
            }
        }
    }
    tracker.branching = ratio(tracker.created, tracker.closed);

    for card in seats.values_mut().chain([&mut total, &mut unattributed]) {
        card.runs_per_landing = ratio(card.runs.total, card.landings);
        card.active_hours = round2(card.active_hours);
    }

    let flow = flow(&index, window);
    let hub = hub(ledger, config, window, &decisions);

    let raw = Raw {
        seats,
        total,
        unattributed,
        flow,
        tracker,
        hub,
        joins,
        runs_by_repo,
        wip_total,
        wip_unattributed,
    };
    publish(raw, &Coverage::new(ledger), window)
}

/// Session hours: the part of each span inside the window.
fn active_hours(
    ledger: &Ledger,
    window: Window,
    seats: &mut BTreeMap<String, Tally>,
    total: &mut Tally,
    unattributed: &mut Tally,
) {
    for event in ledger.events() {
        if event.kind != Kind::SessionActive {
            continue;
        }
        let start = event.t.max(window.from);
        let end = event.t_end.unwrap_or(event.t).min(window.to);
        if end <= start {
            continue;
        }
        let hours = minutes(start, end) / 60.0;
        total.active_hours += hours;
        match &event.seat {
            Some(seat) => seats.entry(seat.clone()).or_default().active_hours += hours,
            None => unattributed.active_hours += hours,
        }
    }
}

/// Join rates, and runs per repository.
fn joins(
    ledger: &Ledger,
    index: &Index<'_>,
    window: Window,
) -> (JoinsRaw, BTreeMap<String, RunCounts>) {
    let mut joins = JoinsRaw::default();
    let mut runs_by_repo: BTreeMap<String, RunCounts> = BTreeMap::new();
    for event in ledger.events().iter().filter(|e| window.contains(e.t)) {
        match event.kind {
            Kind::Run => {
                joins.runs += 1;
                if event.seat.is_some() {
                    joins.runs_attributed += 1;
                }
                let repo = event.attr_str("repo").unwrap_or("(none)").to_owned();
                count_run(runs_by_repo.entry(repo).or_default(), event);
            }
            Kind::Land => {
                joins.landings += 1;
                if let Some(session) = &event.ids.session {
                    joins.landing_session += 1;
                    if index.sessions_seen.contains(session.as_str()) {
                        joins.landing_session_seen += 1;
                    }
                }
                if !index.verdicts_of(event).is_empty() {
                    joins.landings_citing_verdict += 1;
                }
                for cite in &event.ids.cites {
                    joins.cites += 1;
                    if index.verdicts.contains_key(cite.as_str()) {
                        joins.cites_resolved += 1;
                    }
                }
            }
            _ => {}
        }
    }
    (joins, runs_by_repo)
}

impl Tally {
    /// Whether anything happened for this seat in the window.
    #[must_use]
    pub fn active(&self) -> bool {
        self.landings
            + self.runs.total
            + self.gate_runs
            + self.verdicts_go
            + self.verdicts_return
            + self.messages_sent
            + self.messages_received
            + self.items_created
            + self.wip_at_end
            > 0
            || self.active_hours > 0.0
    }
}

/// Count one event into each card it touches.
fn apply(event: &Event, decisions: &BTreeSet<&str>, cards: &mut [&mut Tally]) {
    for card in cards.iter_mut() {
        match &event.kind {
            Kind::Land => card.landings += 1,
            Kind::Run => count_run(&mut card.runs, event),
            Kind::GateRun => card.gate_runs += 1,
            Kind::Verdict => match event.outcome {
                Some(Outcome::Go | Outcome::Pass) => card.verdicts_go += 1,
                Some(Outcome::Return | Outcome::Fail) => card.verdicts_return += 1,
                _ => {}
            },
            Kind::Message => {
                card.messages_sent += 1;
                let names_goal = names_goal(event);
                if names_goal {
                    card.messages_naming_goal += 1;
                }
                if !names_goal
                    && event
                        .attr_str("message_kind")
                        .is_some_and(|k| decisions.contains(k))
                {
                    card.decisions_without_goal += 1;
                }
            }
            Kind::Create => card.items_created += 1,
            _ => {}
        }
    }
}

/// Whether a message names a goal: a subject, or goal ids found in its text.
#[must_use]
pub fn names_goal(event: &Event) -> bool {
    event.subject.is_some() || !event.attr_strs("goals").is_empty()
}

fn count_run(counts: &mut RunCounts, event: &Event) {
    counts.total += 1;
    match event.outcome {
        Some(Outcome::Pass) => counts.pass += 1,
        Some(Outcome::Fail) => {
            counts.fail += 1;
            if event.attr_str("reason").is_none() && event.attr_str("failure_tail").is_none() {
                counts.fail_without_reason += 1;
            }
        }
        _ => counts.other += 1,
    }
}

fn flow(index: &Index<'_>, window: Window) -> FlowRaw {
    let mut lead = Vec::new();
    let mut review = Vec::new();
    for land in index.lands.iter().filter(|e| window.contains(e.t)) {
        let Some(start) = land
            .ids
            .commit
            .as_deref()
            .and_then(|c| index.change_start(c))
        else {
            continue;
        };
        lead.push(minutes(start, land.t));
        let first_verdict = index.verdicts_of(land).iter().map(|v| v.t).min();
        if let Some(verdict) = first_verdict {
            review.push(minutes(start, verdict));
        }
    }
    let gate: Vec<f64> = index
        .gate_runs
        .iter()
        .filter(|g| window.contains(g.t))
        .filter(|g| g.attr_str("recipe") == Some("land") && g.outcome == Some(Outcome::Pass))
        .filter_map(|g| g.t_end.map(|end| minutes(g.t, end)))
        .collect();
    FlowRaw {
        lead_median_min: median(&lead),
        lead_n: lead.len(),
        review_median_min: median(&review),
        review_n: review.len(),
        gate_median_min: median(&gate),
        gate_n: gate.len(),
    }
}

fn hub(
    ledger: &Ledger,
    config: &Config,
    window: Window,
    decisions: &BTreeSet<&str>,
) -> Option<Hub> {
    let messages: Vec<&Event> = ledger
        .events()
        .iter()
        .filter(|e| e.kind == Kind::Message && window.contains(e.t))
        .collect();
    let mut inbound: BTreeMap<&str, usize> = BTreeMap::new();
    for m in &messages {
        for seat in m.attr_strs("to_seats") {
            if Some(seat) != m.seat.as_deref() {
                *inbound.entry(seat).or_default() += 1;
            }
        }
    }
    let seat = if let Some(first) = config.messages.hubs.first() {
        first.clone()
    } else {
        let mut best: Option<(&str, usize)> = None;
        for (&seat, &n) in &inbound {
            if best.is_none_or(|(_, m)| n > m) {
                best = Some((seat, n));
            }
        }
        best?.0.to_owned()
    };
    let into: Vec<&&Event> = messages
        .iter()
        .filter(|m| m.attr_strs("to_seats").contains(&seat.as_str()))
        .filter(|m| m.seat.as_deref() != Some(seat.as_str()))
        .collect();
    // replies anywhere in the ledger (a reply may fall after the window)
    let mut first_reply: HashMap<&str, Timestamp> = HashMap::new();
    for m in ledger.events().iter().filter(|e| e.kind == Kind::Message) {
        if m.seat.as_deref() != Some(seat.as_str()) {
            continue;
        }
        if let Some(re) = m.attr_str("re") {
            first_reply.entry(re).or_insert(m.t);
        }
    }
    let replies: Vec<f64> = into
        .iter()
        .filter_map(|m| {
            let id = m.ids.message.as_deref()?;
            first_reply.get(id).map(|t| minutes(m.t, *t))
        })
        .collect();
    let decisions_without_goal = into
        .iter()
        .filter(|m| {
            !names_goal(m)
                && m.attr_str("message_kind")
                    .is_some_and(|k| decisions.contains(k))
        })
        .count();
    let days = window.length_days();
    Some(Hub {
        inbound: into.len(),
        #[allow(clippy::cast_precision_loss)]
        inbound_per_day: if days > 0.0 {
            round2(into.len() as f64 / days)
        } else {
            0.0
        },
        decisions_without_goal,
        reply_median_min: median(&replies),
        reply_n: replies.len(),
        seat,
    })
}
