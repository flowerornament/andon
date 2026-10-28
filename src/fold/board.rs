//! `andon board`: one row per seat, as of an instant (founding design §6).
//!
//! The instant is a parameter. `board` passes the wall clock at render;
//! `asof` passes a past instant and folds only the ledger prefix that existed
//! then, so a past board is reproduced exactly.

use std::collections::BTreeMap;

use jiff::{SignedDuration, Timestamp};
use serde::Serialize;

use crate::config::Config;
use crate::event::ALL_HISTORY;
use crate::event::attr;
use crate::event::{Event, Kind, Outcome};
use crate::fold::gauge::{Gauge, Gauger, Scope, Span, Spec, TRACKER};
use crate::fold::projection::{Projection, held, session_minutes};
use crate::fold::{Item, round2};
use crate::ledger::Ledger;
use crate::time::minutes;

/// The board's gauges. A gauge is declared here, beside the fold that
/// computes it, and listed in [`SPECS`].
pub const UNSEATED: Spec = Spec::new(Scope::Board, "unseated", TRACKER, Span::State);
pub const GOALS: Spec = Spec::new(Scope::Row, "goals", TRACKER, Span::State);
pub const LANDINGS: Spec = Spec::new(Scope::Row, "landings", &["land"], Span::Window);
pub const LAST_LAND: Spec = Spec::new(Scope::Row, "last_land", &["land"], Span::Origin);
pub const RUNS: Spec = Spec::new(Scope::Row, "runs", &["run"], Span::Window);
pub const RUNS_PASS: Spec = Spec::new(Scope::Row, "runs_pass", &["run"], Span::Window);
pub const RUNS_SINCE_LAND: Spec = Spec::new(
    Scope::Row,
    "runs_since_land",
    &["run", "land"],
    Span::Origin,
);
pub const LAST_VERDICT: Spec =
    Spec::new(Scope::Row, "last_verdict", &["verdict"], Span::Origin).for_seat(&["commit"]);
pub const FINDINGS: Spec =
    Spec::new(Scope::Row, "findings", &["other:comment"], Span::Origin).over(&[&GOALS]);
/// Budget spent: session hours, runs or wall-clock hours since the claim.
pub const USED: Spec = Spec::new(
    Scope::Row,
    "goals.*.used",
    &["session_active"],
    Span::Origin,
)
.over(&[&GOALS]);
pub const USED_RUNS: Spec = Spec::new(Scope::Row, "goals.*.used", &["run"], Span::Origin)
    .over(&[&GOALS])
    .variant("runs");
/// Whether a session runs at the seat now, and in what status: harness
/// session files, read as current state.
pub const STATE: Spec = Spec::new(Scope::Row, "state", &["presence"], Span::State);
/// Files the seat's workspace has changed and not landed that another
/// seat's has too: every workspace's state at the instant.
pub const OVERLAPS: Spec = Spec::new(Scope::Row, "overlaps", &["unlanded"], Span::State);
pub const USED_WALL: Spec = Spec::new(Scope::Row, "goals.*.used", &[], Span::Origin)
    .over(&[&GOALS])
    .variant("wall-clock");

pub const SPECS: &[&Spec] = &[
    &UNSEATED,
    &GOALS,
    &LANDINGS,
    &LAST_LAND,
    &RUNS,
    &RUNS_PASS,
    &RUNS_SINCE_LAND,
    &LAST_VERDICT,
    &FINDINGS,
    &OVERLAPS,
    &USED,
    &USED_RUNS,
    &USED_WALL,
    &STATE,
];

/// The board.
#[derive(Debug, Clone, Serialize)]
pub struct Board {
    pub schema: u32,
    pub asof: Timestamp,
    /// Counts in each row cover this many hours before `asof`.
    pub window_hours: f64,
    pub seats: Vec<Row>,
    /// Open goals claimed by an identity that is no configured seat (a
    /// shared default user): no row shows them, so the board names them.
    pub unseated: Gauge<Vec<Unseated>>,
    /// Whether a goal schema applies (`[goal]` declared): without one the
    /// text drops its goal and budget columns.
    #[serde(skip)]
    pub goal_schema: bool,
    /// The fix for goals claimed by a non-seat, for any goal (`<goal>`).
    #[serde(skip)]
    pub unseated_fix: String,
    /// Whether a source read the workspaces at this instant: otherwise (no
    /// jj, or a past instant) the text says nothing of overlaps.
    #[serde(skip)]
    pub overlaps_read: bool,
}

/// The flags a row can raise, and what each means: `board --help` reads
/// them from here, beside [`flags`] that raises them.
pub const FLAGS: &[(&str, &str)] = &[
    (
        "over budget on G",
        "more effort spent on G than its budget allows.",
    ),
    (
        "N goals in progress, WIP W",
        "more claimed goals of a counted type (`[goal] types`) than the seat may hold.",
    ),
    (
        "no landing in Nd",
        "the lead goal has gone N days with no landing (not for hubs).",
    ),
    (
        "last land refused",
        "the seat's last land run after its last landing failed or was refused.",
    ),
    (
        "idle Nh holding G",
        "the seat's live session has waited N hours for a prompt while it holds G (`[board] idle_minutes`).",
    ),
    (
        "N runs since last landing",
        "runs piling up with nothing landed (not for hubs).",
    ),
    (
        "overlaps S (N files)",
        "files the seat's workspace changed and has not landed that seat S's has too, as of each workspace's last jj snapshot; the files are listed under the table (`?` where a workspace could not be read).",
    ),
];

/// A goal claimed by no seat, and by whom.
#[derive(Debug, Clone, Serialize)]
pub struct Unseated {
    pub goal: String,
    pub owner: String,
    /// How to make the tracker record a seat instead, in this project's
    /// tracker: the note's fix, not a fact, so not in the output.
    #[serde(skip)]
    pub fix: String,
}

/// One seat. Every gauge is a [`Gauge`]: known, or unknown with the reason
/// (`.design/2026-10-14-coverage.md`).
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub seat: String,
    /// Goals the seat holds, claimed and open, the one its work cites first.
    pub goals: Gauge<Vec<Goal>>,
    pub landings: Gauge<usize>,
    pub last_land: Gauge<Option<Timestamp>>,
    pub runs: Gauge<usize>,
    pub runs_pass: Gauge<usize>,
    /// Runs since the seat's last landing; with no landing in the ledger,
    /// runs since `runs_from`.
    pub runs_since_land: Gauge<usize>,
    /// Where `runs_since_land` counts from when the seat has no landing: the
    /// later of where landings are covered and the seat's earliest run. Read
    /// by the flag that names it; not a gauge, so not in the output.
    #[serde(skip)]
    pub runs_from: Option<Timestamp>,
    pub last_verdict: Gauge<Option<Verdict>>,
    /// Comments on the seat's lead goal since it was claimed.
    pub findings: Gauge<usize>,
    /// The seat's live session now (`None` inside when none runs). Absent
    /// where no source of live sessions was read: a project without one
    /// has no such column, not a column of unknowns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Gauge<Option<Presence>>>,
    /// Other seats whose unlanded edits share files with this seat's.
    pub overlaps: Gauge<Vec<Overlap>>,
    /// Flags read only known gauges: a flag never speaks where the ledger
    /// could not look.
    pub flags: Vec<String>,
}

/// Another seat whose workspace changed files this seat's changed, neither
/// landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Overlap {
    pub seat: String,
    /// Every shared path, relative to the workspace root, sorted.
    pub files: Vec<String>,
    /// When the other workspace was last snapshotted: what it holds is as
    /// of then.
    pub snapshot: Option<Timestamp>,
}

/// The board's flag for a seat's overlaps: whom, and how many files in all.
/// The paths are listed under the table, and every one in `--json`.
#[must_use]
pub fn overlaps_flag(overlaps: &[Overlap]) -> Option<String> {
    let seats: Vec<&str> = overlaps.iter().map(|o| o.seat.as_str()).collect();
    let files: std::collections::BTreeSet<&String> =
        overlaps.iter().flat_map(|o| &o.files).collect();
    let n = files.len();
    let unit = if n == 1 { "file" } else { "files" };
    (!seats.is_empty()).then(|| format!("overlaps {} ({n} {unit})", seats.join(", ")))
}

/// The hook's change for an overlap: the seat, the count and the first
/// paths.
#[must_use]
pub fn overlap(o: &Overlap) -> String {
    let n = o.files.len();
    let shown = match o.files.as_slice() {
        [a] => a.clone(),
        [a, b] => format!("{a}, {b}"),
        [a, b, rest @ ..] => format!("{a}, {b}, +{}", rest.len()),
        [] => String::new(),
    };
    let files = if n == 1 { "file" } else { "files" };
    format!("overlaps {} on {n} {files} ({shown})", o.seat)
}

#[derive(Debug, Clone, Serialize)]
pub struct Goal {
    pub id: String,
    pub title: Option<String>,
    pub claimed_at: Option<Timestamp>,
    /// The goal's budget as written.
    pub budget: Option<String>,
    /// The budget as a number of hours (or runs), when it states one.
    pub amount: Option<f64>,
    pub unit: Option<String>,
    /// Effort spent since the claim, in the budget's unit.
    pub used: Gauge<f64>,
    /// Whether it counts toward WIP (`[goal] types`; an epic does not).
    #[serde(skip)]
    pub counts: bool,
    /// The unit `used` is in: the budget's, else `[board] budget_unit`.
    #[serde(skip)]
    pub used_unit: String,
    /// The seat's latest commit or landing that cites this goal, as read:
    /// only the key that puts the goal being worked first, not a gauge, so
    /// not in the output.
    #[serde(skip)]
    pub last_work: Option<Timestamp>,
}

impl Goal {
    /// Whether more was spent than the budget allows; never where either
    /// is unknown.
    #[must_use]
    pub fn over_budget(&self) -> bool {
        matches!((self.used.get(), self.amount), (Some(u), Some(b)) if *u > b)
    }
}

/// The flag, and the hook's change, for a goal spent past its budget.
#[must_use]
pub fn over_budget(goal: &str) -> String {
    format!("over budget on {goal}")
}

/// A seat's live sessions at an instant, as one status: busy while any is
/// busy (since the longest-running turn began), else idle since the last
/// turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Presence {
    pub status: String,
    pub since: Timestamp,
    pub sessions: usize,
}

/// The status of the seat's sessions that run at `asof`.
#[must_use]
pub fn presence(events: &[&Event], asof: Timestamp) -> Option<Presence> {
    let live: Vec<&Event> = events
        .iter()
        .copied()
        .filter(|e| e.kind == Kind::Presence && e.t <= asof && e.t_end.is_some_and(|t| t >= asof))
        .collect();
    let busy: Vec<Timestamp> = live
        .iter()
        .filter(|e| e.attr_str(attr::STATUS) == Some("busy"))
        .map(|e| e.t)
        .collect();
    let (status, since) = if let Some(t) = busy.iter().min() {
        ("busy", *t)
    } else {
        let e = live.iter().max_by_key(|e| e.t)?;
        (e.attr_str(attr::STATUS).unwrap_or("live"), e.t)
    };
    Some(Presence {
        status: status.to_owned(),
        since,
        sessions: live.len(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    pub outcome: Option<Outcome>,
    pub t: Timestamp,
    pub subject: Option<String>,
}

/// What the fold counted for one seat, before coverage decides what may be
/// said.
#[derive(Default)]
struct Tally {
    landings: usize,
    last_land: Option<Timestamp>,
    runs: usize,
    runs_pass: usize,
    runs_since_land: usize,
    runs_from: Option<Timestamp>,
    /// Where the since-land count starts: the last landing, else where
    /// landings are covered.
    since_land_origin: Option<Timestamp>,
    last_verdict: Option<Verdict>,
}

/// Fold the board as of `asof`.
#[must_use]
pub fn board(ledger: &Ledger, config: &Config, asof: Timestamp) -> Board {
    let ledger = ledger.prefix(asof);
    let p = Projection::new(&ledger, config);
    board_of(&p, asof)
}

/// The board over a projection of the ledger as it stood at `asof`.
#[must_use]
pub fn board_of(p: &Projection<'_>, asof: Timestamp) -> Board {
    let config = p.config;
    let window_hours = config.board.window_hours;
    let g = gauger(p, asof);
    let items = p.index.items_at(asof);
    let mut seats: Vec<String> = config.seats.keys().cloned().collect();
    if config.seats.is_empty() {
        // no seats configured: a row for every seat active in the window or
        // holding a goal (a stalled seat is the one the board is for)
        let holding: Vec<&str> = items
            .values()
            .filter(|i| i.claimed && !i.closed)
            .filter_map(|i| i.owner.as_deref())
            .collect();
        for (seat, events) in p.seats() {
            if events.iter().any(|e| e.t >= g.from) || holding.contains(&seat) {
                seats.push(seat.to_owned());
            }
        }
    }
    let rows = seats
        .into_iter()
        .map(|seat| row_of(p, &g, &items, seat))
        .collect();
    let unseated: Vec<Unseated> = if config.seats.is_empty() {
        Vec::new()
    } else {
        items
            .iter()
            .filter(|(_, i)| i.claimed && !i.closed)
            .filter_map(|(goal, i)| {
                let owner = i.owner.as_deref()?;
                (!config.seats.contains_key(owner)).then(|| Unseated {
                    goal: goal.clone(),
                    owner: owner.to_owned(),
                    fix: crate::seat::identity_hint_any(config, goal),
                })
            })
            .collect()
    };
    Board {
        schema: 4,
        asof,
        window_hours,
        goal_schema: config.goal_schema(),
        unseated_fix: crate::seat::identity_hint_any(config, "<goal>"),
        overlaps_read: looked_at(p, "state:unlanded", asof),
        unseated: g.gauge(&UNSEATED, None, None, unseated),
        seats: rows,
    }
}

/// The gauges' window for a board at `asof`: the trailing `[board]
/// window_hours`.
#[must_use]
pub fn gauger<'p>(p: &'p Projection<'_>, asof: Timestamp) -> Gauger<'p> {
    let since = asof
        .checked_sub(SignedDuration::from_secs_f64(
            p.config.board.window_hours * 3_600.0,
        ))
        .unwrap_or(Timestamp::MIN);
    Gauger {
        coverage: &p.coverage,
        from: since,
        to: asof,
    }
}

/// One seat's row, at the gauger's instant.
#[must_use]
pub fn row_of(
    p: &Projection<'_>,
    g: &Gauger<'_>,
    items: &BTreeMap<String, Item>,
    seat: String,
) -> Row {
    let events = p.events(&seat);
    let landings_from = p.coverage.covered_from("land", Some(&seat));
    let tally = tally(events, g.from, landings_from);
    let held = goals(g, &seat, items, events, p.config);
    let findings = findings(p.ledger, &held);
    row(g, seat, &tally, held, findings, p, landings_from)
}

/// The goals `seat` held at the gauger's instant, with budget spent then.
#[must_use]
pub fn goals_at(p: &Projection<'_>, g: &Gauger<'_>, seat: &str) -> Gauge<Vec<Goal>> {
    let items = p.index.items_at(g.to);
    let held = goals(g, seat, &items, p.events(seat), p.config);
    g.gauge(&GOALS, Some(seat), None, held)
}

/// The only way a row is made: every value through its registered gauge.
fn row(
    g: &Gauger<'_>,
    seat: String,
    t: &Tally,
    held: Vec<Goal>,
    findings: usize,
    p: &Projection<'_>,
    landings_from: Option<Timestamp>,
) -> Row {
    let s = Some(seat.as_str());
    // findings count from the lead goal's claim; with no goal held there is none
    // to find, which is known where comments are read at the instant
    let first_claim = held.first().and_then(|x| x.claimed_at);
    let goals = g.gauge(&GOALS, s, None, held);
    let findings = g.over(&goals, &FINDINGS, s, first_claim.or(Some(g.to)), findings);
    let overlaps = g.gauge(&OVERLAPS, None, None, overlaps(p, &seat, g.to));
    let mut row = Row {
        goals,
        landings: g.gauge(&LANDINGS, s, None, t.landings),
        // a last landing is known if nothing after it went unread; no
        // landing at all, only if all history was read
        last_land: g.gauge(
            &LAST_LAND,
            s,
            Some(t.last_land.unwrap_or(ALL_HISTORY)),
            t.last_land,
        ),
        runs: g.gauge(&RUNS, s, None, t.runs),
        runs_pass: g.gauge(&RUNS_PASS, s, None, t.runs_pass),
        runs_since_land: g.gauge(&RUNS_SINCE_LAND, s, t.since_land_origin, t.runs_since_land),
        runs_from: t.runs_from,
        last_verdict: g.gauge(
            &LAST_VERDICT,
            s,
            Some(t.last_verdict.as_ref().map_or(ALL_HISTORY, |v| v.t)),
            t.last_verdict.clone(),
        ),
        findings,
        state: p
            .coverage
            .reads("state:presence")
            .then(|| g.gauge(&STATE, s, None, presence(p.events(&seat), g.to))),
        overlaps,
        flags: Vec::new(),
        seat,
    };
    row.flags = flags(&row, p, g, landings_from);
    row
}

/// The files `seat`'s workspace held unlanded at `at`, each with when its
/// newest commit touching it was written.
#[must_use]
pub fn unlanded(
    events: &[&Event],
    at: Timestamp,
) -> BTreeMap<String, (Timestamp, Option<Timestamp>)> {
    events
        .iter()
        .filter(|e| e.kind == Kind::Unlanded && e.t <= at && e.t_end.is_some_and(|t| t >= at))
        .filter_map(|e| {
            let snapshot = e
                .attr_str(crate::event::attr::SNAPSHOT)
                .and_then(|s| crate::time::parse_instant(s).ok());
            Some((
                e.attr_str(crate::event::attr::PATH)?.to_owned(),
                (e.t, snapshot),
            ))
        })
        .collect()
}

/// The other seats whose workspaces held, unlanded at `at`, files `seat`'s
/// held too.
#[must_use]
pub fn overlaps(p: &Projection<'_>, seat: &str, at: Timestamp) -> Vec<Overlap> {
    let mine = unlanded(p.events(seat), at);
    if mine.is_empty() {
        return Vec::new();
    }
    p.seats()
        .filter(|(other, _)| *other != seat)
        .filter_map(|(other, events)| {
            let theirs = unlanded(events, at);
            let files: Vec<String> = theirs
                .keys()
                .filter(|f| mine.contains_key(*f))
                .cloned()
                .collect();
            (!files.is_empty()).then(|| Overlap {
                seat: other.to_owned(),
                snapshot: theirs.values().find_map(|(_, s)| *s),
                files,
            })
        })
        .collect()
}

/// Whether a source looked at `kind` at the instant: a span or a gap of it
/// holds `at`.
fn looked_at(p: &Projection<'_>, kind: &str, at: Timestamp) -> bool {
    p.ledger.events().iter().any(|e| {
        e.kind == Kind::Coverage
            && e.t <= at
            && e.t_end.is_some_and(|t| t >= at)
            && e.attr_strs(crate::event::attr::COVERS).contains(&kind)
    })
}

/// Counts, last landing, last verdict.
fn tally(events: &[&Event], since: Timestamp, landings_from: Option<Timestamp>) -> Tally {
    let mut t = Tally::default();
    for event in events {
        match event.kind {
            Kind::Land => {
                if event.t >= since {
                    t.landings += 1;
                }
                t.last_land = Some(event.t);
            }
            Kind::Run if event.t >= since => {
                t.runs += 1;
                if event.outcome == Some(Outcome::Pass) {
                    t.runs_pass += 1;
                }
            }
            Kind::Verdict => {
                t.last_verdict = Some(Verdict {
                    outcome: event.outcome,
                    t: event.t,
                    subject: event.subject.clone(),
                });
            }
            _ => {}
        }
    }
    let first_run = events.iter().find(|e| e.kind == Kind::Run).map(|e| e.t);
    if t.last_land.is_none() {
        // a seat with no landing: runs count only where landings are covered
        t.runs_from = landings_from.zip(first_run).map(|(l, r)| l.max(r));
    }
    // an origin after the instant, or none, makes the count unknown (C1)
    t.since_land_origin = t.last_land.or(t.runs_from).or(landings_from);
    t.runs_since_land = events
        .iter()
        .filter(|e| e.kind == Kind::Run)
        .filter(|e| match (t.last_land, t.runs_from) {
            (Some(l), _) => e.t > l,
            (None, Some(from)) => e.t >= from,
            (None, None) => false,
        })
        .count();
    t
}

/// The seat's open, claimed goals, with budget spent.
fn goals(
    g: &Gauger<'_>,
    seat: &str,
    items: &BTreeMap<String, Item>,
    events: &[&Event],
    config: &Config,
) -> Vec<Goal> {
    let mut goals: Vec<Goal> = held(items, seat)
        .map(|(id, item)| goal_of(g, seat, id, item, events, config))
        .collect();
    // the goal the seat is working on leads: the one its latest commits
    // cite; the claim order only breaks ties (a fresh claim with no work yet
    // is not the work)
    for goal in &mut goals {
        goal.last_work = events
            .iter()
            .filter(|e| matches!(e.kind, Kind::Commit | Kind::Land | Kind::Freeze))
            .filter(|e| e.subject.as_deref() == Some(goal.id.as_str()))
            .map(|e| e.t)
            .max();
    }
    goals.sort_by(|a, b| {
        b.last_work
            .cmp(&a.last_work)
            .then_with(|| b.claimed_at.cmp(&a.claimed_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    goals
}

/// One goal `seat` holds, with its budget spent since the claim, at the
/// gauger's instant; `events` are the seat's.
#[must_use]
pub fn goal_of(
    g: &Gauger<'_>,
    seat: &str,
    id: &str,
    item: &Item,
    events: &[&Event],
    config: &Config,
) -> Goal {
    let (amount, unit) = item
        .budget
        .as_deref()
        .and_then(crate::goal::parse_budget)
        .map_or((None, None), |(n, u)| (Some(n), Some(u)));
    let unit_read = unit.as_deref().unwrap_or(&config.board.budget_unit);
    let spec = match unit_read {
        "runs" => &USED_RUNS,
        "wall-clock" => &USED_WALL,
        _ => &USED,
    };
    let used = g.gauge(
        spec,
        Some(seat),
        item.claimed_at,
        item.claimed_at
            .map_or(0.0, |claimed| spent(unit_read, events, claimed, g.to)),
    );
    Goal {
        id: id.to_owned(),
        title: item.title.clone(),
        claimed_at: item.claimed_at,
        budget: item.budget.clone(),
        counts: item.in_progress(config),
        used_unit: unit_read.to_owned(),
        amount,
        unit,
        used,
        last_work: None,
    }
}

/// Effort since `from`: session hours, wall-clock hours or runs.
fn spent(unit: &str, events: &[&Event], from: Timestamp, to: Timestamp) -> f64 {
    let value = match unit {
        "runs" => {
            #[allow(clippy::cast_precision_loss)]
            let n = events
                .iter()
                .filter(|e| e.kind == Kind::Run && e.t >= from && e.t <= to)
                .count() as f64;
            n
        }
        "wall-clock" => minutes(from, to) / 60.0,
        "minutes" => events
            .iter()
            .map(|e| session_minutes(e, from, to))
            .sum::<f64>(),
        _ => {
            events
                .iter()
                .map(|e| session_minutes(e, from, to))
                .sum::<f64>()
                / 60.0
        }
    };
    // `+ 0.0`: a sum of nothing is -0.0, which must not print as such
    round2(value) + 0.0
}

/// Comments on the seat's goals since each was claimed.
/// Comments on the lead goal (the one the seat's work cites) since it was
/// claimed: what the seat has been told about the work in hand, not every
/// comment on everything it holds.
fn findings(ledger: &Ledger, goals: &[Goal]) -> usize {
    let Some(lead) = goals.first() else { return 0 };
    ledger
        .events()
        .iter()
        .filter(|e| e.kind == Kind::Other("other:comment".to_owned()))
        .filter(|e| {
            e.subject.as_deref() == Some(lead.id.as_str())
                && lead.claimed_at.is_none_or(|c| e.t >= c)
        })
        .count()
}

fn flags(
    row: &Row,
    p: &Projection<'_>,
    g: &Gauger<'_>,
    landings_from: Option<Timestamp>,
) -> Vec<String> {
    let config = p.config;
    let events = p.events(&row.seat);
    let mut flags = Vec::new();
    let seat = Some(row.seat.as_str());
    let asof = g.to;
    let wip = config.wip_for(&row.seat);
    let held = row.goals.get().map_or(&[][..], Vec::as_slice);
    // WIP and budgets are the goal schema's rules: without `[goal]` the
    // project chose neither
    let counted = held.iter().filter(|g| g.counts).count();
    if config.goal_schema() && counted > wip {
        flags.push(format!("{counted} goals in progress, WIP {wip}"));
    }
    for goal in held
        .iter()
        .filter(|g| config.goal_schema() && g.over_budget())
    {
        flags.push(over_budget(&goal.id));
    }
    let last_land = row.last_land.get().copied().flatten();
    // a hub (`[messages] hubs`) coordinates and does not land: quiet is its
    // role, not a stall, so the landing flags do not apply to it
    let hub = config.messages.hubs.contains(&row.seat);
    if let Some(goal) = held.first().filter(|_| !hub) {
        // quiet since the later of the last landing and the claim, and never
        // from before landings are covered; absence speaks only where
        // landings are covered from then to now
        let quiet_since = last_land.max(goal.claimed_at).max(landings_from);
        let covered = quiet_since
            .is_some_and(|t| t <= asof && g.coverage.check(&["land"], seat, t, asof).is_none());
        if let (Some(t), true) = (quiet_since, covered) {
            let days = minutes(t, asof) / 1_440.0;
            if days >= config.board.no_land_days {
                flags.push(format!("no landing in {days:.1}d"));
            }
        }
    }
    // idle past the threshold while holding a goal: someone is being waited on
    if let (Some(Some(state)), Some(goal)) = (row.state.as_ref().and_then(Gauge::get), held.first())
        && state.status == "idle"
        && minutes(state.since, asof) >= config.board.idle_minutes
    {
        let hours = minutes(state.since, asof) / 60.0;
        flags.push(format!("idle {hours:.1}h holding {}", goal.id));
    }
    // a refused land run after the last landing: only where the last landing
    // is known, so no later landing went unread
    if row.last_land.is_known()
        && let Some(run) = events
            .iter()
            .rev()
            .find(|e| e.kind == Kind::GateRun && p.is_land_gate(e))
        && last_land.is_none_or(|l| run.t > l)
        && p.refused_land(run)
    {
        flags.push("last land refused".to_owned());
    }
    // an overlap speaks where every workspace was read; `?` where one was
    // looked at and could not be read
    match row.overlaps.get() {
        Some(overlaps) => flags.extend(overlaps_flag(overlaps)),
        None if looked_at(p, "state:unlanded", asof) => flags.push("overlaps ?".to_owned()),
        None => {}
    }
    // the count speaks only where it is known (C2)
    if let Some(n) = row.runs_since_land.get()
        && !hub
        && *n >= config.board.runs_without_land
    {
        if last_land.is_some() {
            flags.push(format!("{n} runs since last landing"));
        } else if let Some(from) = row.runs_from {
            // no landing in the ledger: say from where it could have seen one
            flags.push(format!(
                "{n} runs and no landing since {}",
                from.to_zoned(jiff::tz::TimeZone::UTC).strftime("%Y-%m-%d")
            ));
        }
    }
    flags
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::Path;

    use super::*;
    use crate::adapters;
    use crate::time::parse_instant;

    fn at(s: &str) -> Timestamp {
        parse_instant(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn event(t: &str, kind: Kind, actor: &str, goal: Option<&str>) -> Event {
        let mut e = Event::new(at(t), kind, "test");
        e.actor = Some(actor.to_owned());
        e.subject = goal.map(str::to_owned);
        e.ids.goal = e.subject.clone();
        e
    }

    fn config() -> Config {
        Config::parse(
            "[seats.x-1a]\n[seats.x-1b]\n[board]\nruns_without_land = 2\n",
            Path::new("/"),
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn the_lead_goal_is_the_one_the_seats_work_cites() {
        // x-1a claimed x-old first and worked on it; x-new is claimed later
        // and has no commits yet: the work leads, not the newest claim
        let events = vec![
            event("2026-10-13T08:00:00Z", Kind::Claim, "x-1a", Some("x-old")),
            event("2026-10-13T09:00:00Z", Kind::Commit, "x-1a", Some("x-old")),
            event("2026-10-13T10:00:00Z", Kind::Claim, "x-1a", Some("x-new")),
            covered(
                "1970-01-01T00:00:00Z",
                &["create", "claim", "close", "reopen", "assign"],
            ),
        ];
        let ledger = adapters::resolve(&config(), events);
        let b = board(&ledger, &config(), at("2026-10-13T12:00:00Z"));
        let goals: Vec<&str> = b.seats[0]
            .goals
            .get()
            .map(|g| g.iter().map(|g| g.id.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(goals, vec!["x-old", "x-new"]);
    }

    fn covered(from: &str, kinds: &[&str]) -> Event {
        crate::event::coverage(at(from), None, kinds, "test", None)
    }

    /// A live session at `seat` in `status` since `since`, read at `capture`.
    fn live(seat: &str, status: &str, since: &str, capture: &str) -> Vec<Event> {
        let mut e = event(since, Kind::Presence, seat, None);
        e.t_end = Some(at(capture));
        e.set_attr(attr::STATUS, status);
        let read = crate::event::coverage(
            at(capture),
            Some(at(capture)),
            &["state:presence"],
            "t",
            None,
        );
        vec![e, read]
    }

    fn presence_board(status: &str, since: &str) -> Board {
        let noon = "2026-10-13T12:00:00Z";
        let mut events = vec![
            event("2026-10-13T08:00:00Z", Kind::Claim, "x-1a", Some("x-goal")),
            covered(
                "1970-01-01T00:00:00Z",
                &[
                    "create", "claim", "close", "reopen", "assign", "land", "run",
                ],
            ),
        ];
        events.extend(live("x-1a", status, since, noon));
        let ledger = adapters::resolve(&config(), events);
        board(&ledger, &config(), at(noon))
    }

    #[test]
    fn an_idle_seat_holding_a_goal_is_flagged_and_a_busy_one_is_not() {
        let b = presence_board("idle", "2026-10-13T09:30:00Z");
        let row = &b.seats[0];
        let state = row.state.as_ref().and_then(Gauge::get).cloned().flatten();
        assert_eq!(state.map(|s| s.status), Some("idle".to_owned()));
        assert!(
            row.flags.contains(&"idle 2.5h holding x-goal".to_owned()),
            "{:?}",
            row.flags
        );
        // x-1b runs nothing: known, and no session
        assert_eq!(b.seats[1].state.as_ref().and_then(Gauge::get), Some(&None));

        let b = presence_board("busy", "2026-10-13T09:30:00Z");
        assert!(
            !b.seats[0].flags.iter().any(|f| f.starts_with("idle")),
            "{:?}",
            b.seats[0].flags
        );
        // idle, but not long enough
        let b = presence_board("idle", "2026-10-13T11:30:00Z");
        assert!(
            !b.seats[0].flags.iter().any(|f| f.starts_with("idle")),
            "{:?}",
            b.seats[0].flags
        );
    }

    #[test]
    fn presence_before_the_capture_is_unknown_and_without_a_source_absent() {
        let noon = "2026-10-13T12:00:00Z";
        let ledger = adapters::resolve(
            &config(),
            live("x-1a", "busy", "2026-10-13T09:00:00Z", noon),
        );
        // the files say who runs now, not who ran at ten
        let b = board(&ledger, &config(), at("2026-10-13T10:00:00Z"));
        let state = b.seats[0]
            .state
            .as_ref()
            .unwrap_or_else(|| panic!("a source was read"));
        assert!(!state.is_known(), "{state:?}");
        let none = adapters::resolve(&config(), Vec::new());
        assert!(board(&none, &config(), at(noon)).seats[0].state.is_none());
    }

    #[test]
    fn no_landing_is_counted_only_where_landings_are_covered() {
        // x-1b claimed in August and ran from 10-07, but landings are read
        // from 10-09: before that, the ledger cannot say x-1b did not land
        let base = || {
            vec![
                event("2026-08-24T00:00:00Z", Kind::Claim, "x-1b", Some("x-goal")),
                event("2026-10-07T00:00:00Z", Kind::Run, "x-1b", None),
                event("2026-10-09T00:00:00Z", Kind::Land, "x-1a", None),
                event("2026-10-10T00:00:00Z", Kind::Run, "x-1b", None),
                event("2026-10-11T00:00:00Z", Kind::Run, "x-1b", None),
                covered("2026-10-01T00:00:00Z", &["run"]),
                covered(
                    "1970-01-01T00:00:00Z",
                    &["create", "claim", "close", "reopen", "assign"],
                ),
            ]
        };
        let mut events = base();
        events.push(covered("2026-10-09T00:00:00Z", &["land"]));
        let ledger = adapters::resolve(&config(), events);
        let b = board(&ledger, &config(), at("2026-10-12T00:00:00Z"));
        let row = b
            .seats
            .iter()
            .find(|r| r.seat == "x-1b")
            .unwrap_or_else(|| panic!("row"));
        assert_eq!(row.runs_since_land, 2);
        assert!(
            row.flags
                .contains(&"2 runs and no landing since 2026-10-09".to_owned()),
            "{:?}",
            row.flags
        );
        // quiet is counted from where landings are covered, not the claim
        assert!(
            row.flags.contains(&"no landing in 3.0d".to_owned()),
            "{:?}",
            row.flags
        );
        // knock the landing coverage out: no absence may be claimed at all
        let ledger = adapters::resolve(&config(), base());
        let b = board(&ledger, &config(), at("2026-10-12T00:00:00Z"));
        let row = b
            .seats
            .iter()
            .find(|r| r.seat == "x-1b")
            .unwrap_or_else(|| panic!("row"));
        assert!(
            row.flags.iter().all(|f| !f.contains("no landing")),
            "{:?}",
            row.flags
        );
        assert_eq!(row.landings.reason(), Some("no source reads landings"));
        // an instant before the horizon is unknown with the horizon named,
        // never "no landing"
        let mut events = base();
        events.push(covered("2026-10-09T00:00:00Z", &["land"]));
        let ledger = adapters::resolve(&config(), events);
        let b = board(&ledger, &config(), at("2026-10-05T00:00:00Z"));
        let row = b
            .seats
            .iter()
            .find(|r| r.seat == "x-1b")
            .unwrap_or_else(|| panic!("row"));
        assert!(
            row.flags.iter().all(|f| !f.contains("no landing")),
            "{:?}",
            row.flags
        );
        assert!(
            row.landings
                .reason()
                .is_some_and(|w| w.starts_with("landings are read from 2026-10-09")),
            "{:?}",
            row.landings
        );
    }

    #[test]
    fn findings_are_the_lead_goals_since_its_claim() {
        let comment =
            |t: &str, goal: &str| event(t, Kind::Other("other:comment".into()), "x-1b", Some(goal));
        let events = vec![
            comment("2026-10-13T07:00:00Z", "x-old"), // before the claim
            event("2026-10-13T08:00:00Z", Kind::Claim, "x-1a", Some("x-old")),
            comment("2026-10-13T08:30:00Z", "x-old"),
            event("2026-10-13T09:00:00Z", Kind::Commit, "x-1a", Some("x-old")),
            event("2026-10-13T10:00:00Z", Kind::Claim, "x-1a", Some("x-new")),
            comment("2026-10-13T10:30:00Z", "x-new"), // not the lead goal
            covered(
                "1970-01-01T00:00:00Z",
                &[
                    "create",
                    "claim",
                    "close",
                    "reopen",
                    "assign",
                    "other:comment",
                ],
            ),
        ];
        let ledger = adapters::resolve(&config(), events);
        let b = board(&ledger, &config(), at("2026-10-13T12:00:00Z"));
        assert_eq!(b.seats[0].findings, 1);
    }

    #[test]
    fn a_hub_gets_no_landing_flags() {
        // a coordinator holds a goal and does not land: its role, not a stall
        let config = Config::parse(
            "[seats.x-1a]\n[seats.x-1b]\n[board]\nruns_without_land = 2\n[messages]\nhubs = [\"x-1b\"]\n",
            Path::new("/"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let events = vec![
            event("2026-10-08T00:00:00Z", Kind::Claim, "x-1b", Some("x-goal")),
            event("2026-10-10T00:00:00Z", Kind::Run, "x-1b", None),
            event("2026-10-11T00:00:00Z", Kind::Run, "x-1b", None),
            covered("2026-10-01T00:00:00Z", &["land", "run"]),
            covered(
                "1970-01-01T00:00:00Z",
                &["create", "claim", "close", "reopen", "assign"],
            ),
        ];
        let ledger = adapters::resolve(&config, events);
        let b = board(&ledger, &config, at("2026-10-12T00:00:00Z"));
        let row = b
            .seats
            .iter()
            .find(|r| r.seat == "x-1b")
            .unwrap_or_else(|| panic!("row"));
        assert!(
            row.flags.iter().all(|f| !f.contains("landing")),
            "{:?}",
            row.flags
        );
    }

    #[test]
    fn a_goal_claimed_by_no_seat_is_on_the_board_with_the_fix() {
        let events = vec![
            event(
                "2026-10-13T08:00:00Z",
                Kind::Claim,
                "Flower Ornament",
                Some("x-shared"),
            ),
            covered(
                "1970-01-01T00:00:00Z",
                &["create", "claim", "close", "reopen", "assign"],
            ),
        ];
        let bd = Config::parse(
            "[seats.x-1a]\n[seats.x-1b]\n[adapters.bd]\ndir = \".\"\n",
            Path::new("/"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let ledger = adapters::resolve(&bd, events.clone());
        let b = board(&ledger, &bd, at("2026-10-13T12:00:00Z"));
        let unseated = b.unseated.get().cloned().unwrap_or_default();
        assert_eq!(unseated.len(), 1);
        assert_eq!(
            (unseated[0].goal.as_str(), unseated[0].owner.as_str()),
            ("x-shared", "Flower Ornament")
        );
        let text = crate::render::board(&b);
        assert!(
            text.contains("x-shared is claimed by 'Flower Ornament', which is not a seat; set BEADS_ACTOR=<seat> or bd update x-shared --assignee <seat>"),
            "{text}"
        );
        // a project with no bd is never told to run bd
        let ledger = adapters::resolve(&config(), events);
        let text = crate::render::board(&board(&ledger, &config(), at("2026-10-13T12:00:00Z")));
        assert!(
            !text.contains("bd ") && !text.contains("BEADS_ACTOR"),
            "{text}"
        );
        assert!(text.contains("set x-shared's assignee to <seat>"), "{text}");
    }

    #[test]
    fn a_budget_with_no_amount_renders_in_the_unit_it_was_counted_in() {
        let runs = Config::parse(
            "[seats.x-1a]\n[board]\nbudget_unit = \"runs\"\n",
            Path::new("/"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let mut create = event("2026-10-13T07:00:00Z", Kind::Create, "x-1a", Some("x-g"));
        create.set_attr(crate::event::attr::BUDGET, "small");
        let events = vec![
            create,
            event("2026-10-13T08:00:00Z", Kind::Claim, "x-1a", Some("x-g")),
            event("2026-10-13T09:00:00Z", Kind::Run, "x-1a", None),
            event("2026-10-13T10:00:00Z", Kind::Run, "x-1a", None),
            covered(
                "1970-01-01T00:00:00Z",
                &["create", "claim", "close", "reopen", "assign", "run"],
            ),
        ];
        let ledger = adapters::resolve(&runs, events);
        let b = board(&ledger, &runs, at("2026-10-13T12:00:00Z"));
        let row = crate::render::board_row(&b.seats[0], b.asof);
        assert_eq!(row[2], "2.0 runs", "{row:?}");
    }

    #[test]
    fn budget_and_wip_flags_are_the_goal_schemas() {
        let mut create = event("2026-10-13T07:00:00Z", Kind::Create, "x-1a", Some("x-g"));
        create.set_attr(crate::event::attr::BUDGET, "1 agent-hour");
        create.set_attr(crate::event::attr::ITEM_TYPE, "task");
        let mut other = event("2026-10-13T07:00:00Z", Kind::Create, "x-1a", Some("x-h"));
        other.set_attr(crate::event::attr::ITEM_TYPE, "task");
        let mut session = event("2026-10-13T08:00:00Z", Kind::SessionActive, "x-1a", None);
        session.t_end = Some(at("2026-10-13T10:00:00Z"));
        let events = vec![
            create,
            other,
            event("2026-10-13T08:00:00Z", Kind::Claim, "x-1a", Some("x-g")),
            event("2026-10-13T08:00:00Z", Kind::Claim, "x-1a", Some("x-h")),
            session,
            covered(
                "1970-01-01T00:00:00Z",
                &[
                    "create",
                    "claim",
                    "close",
                    "reopen",
                    "assign",
                    "session_active",
                    "land",
                    "run",
                ],
            ),
        ];
        let flags = |text: &str| {
            let c = Config::parse(text, Path::new("/")).unwrap_or_else(|e| panic!("{e}"));
            let ledger = adapters::resolve(&c, events.clone());
            board(&ledger, &c, at("2026-10-13T12:00:00Z")).seats[0]
                .flags
                .clone()
        };
        let with = flags("[goal]\n[seats.x-1a]\n");
        assert!(with.contains(&"over budget on x-g".to_owned()), "{with:?}");
        assert!(
            with.contains(&"2 goals in progress, WIP 1".to_owned()),
            "{with:?}"
        );
        let without = flags("[seats.x-1a]\n");
        assert!(
            !without
                .iter()
                .any(|f| f.contains("budget") || f.contains("WIP")),
            "{without:?}"
        );
    }

    #[test]
    fn goals_claimed_by_one_non_seat_are_one_line() {
        let bd = Config::parse(
            "[goal]\n[seats.x-1a]\n[adapters.bd]\ndir = \".\"\n",
            Path::new("/"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let mut events: Vec<Event> = ["x-a", "x-b", "x-c"]
            .iter()
            .map(|g| event("2026-10-13T08:00:00Z", Kind::Claim, "Shared User", Some(g)))
            .collect();
        events.push(event(
            "2026-10-13T08:00:00Z",
            Kind::Claim,
            "2b",
            Some("x-d"),
        ));
        events.push(covered(
            "1970-01-01T00:00:00Z",
            &["create", "claim", "close", "reopen", "assign"],
        ));
        let ledger = adapters::resolve(&bd, events);
        let b = board(&ledger, &bd, at("2026-10-13T12:00:00Z"));
        // every goal is in the output; the text gives one line per identity
        assert_eq!(b.unseated.get().map(Vec::len), Some(4));
        let text = crate::render::board(&b);
        let notes: Vec<&str> = text.lines().filter(|l| l.contains("not a seat")).collect();
        assert_eq!(
            notes,
            vec![
                "x-d is claimed by '2b', which is not a seat; set BEADS_ACTOR=<seat> or bd update x-d --assignee <seat>",
                "3 goals (x-a, +2) are claimed by 'Shared User', which is not a seat; set BEADS_ACTOR=<seat> or bd update <goal> --assignee <seat>",
            ]
        );
    }

    #[test]
    fn the_land_verb_is_the_configured_recipe() {
        let mut gate = event("2026-10-13T09:00:00Z", Kind::GateRun, "x-1a", None);
        gate.set_attr(crate::event::attr::RECIPE, "ship");
        gate.outcome = Some(Outcome::Refused);
        let events = vec![
            gate,
            covered("1970-01-01T00:00:00Z", &["land", "run", "gate_run"]),
        ];
        let flags = |text: &str| {
            let c = Config::parse(text, Path::new("/")).unwrap_or_else(|e| panic!("{e}"));
            let ledger = adapters::resolve(&c, events.clone());
            board(&ledger, &c, at("2026-10-13T12:00:00Z")).seats[0]
                .flags
                .clone()
        };
        assert_eq!(
            flags("[project]\nland_recipe = \"ship\"\n[seats.x-1a]\n"),
            vec!["last land refused".to_owned()]
        );
        assert_eq!(flags("[seats.x-1a]\n"), Vec::<String>::new());
    }

    /// A file `seat`'s workspace held unlanded, read at 12:00.
    fn unlanded(t: &str, seat: &str, path: &str) -> Event {
        let mut e = event(t, Kind::Unlanded, seat, None);
        e.t_end = Some(at("2026-10-13T12:00:00Z"));
        e.set_attr(crate::event::attr::PATH, path);
        e
    }

    fn workspaces_read(kinds: &[&str]) -> Vec<Event> {
        let noon = at("2026-10-13T12:00:00Z");
        vec![
            covered("1970-01-01T00:00:00Z", kinds),
            crate::event::coverage(noon, Some(noon), &["state:unlanded"], "jj", None),
        ]
    }

    /// The board at `asof` over `events`, the workspaces read at 12:00.
    fn overlap_board(events: Vec<Event>, asof: &str) -> Board {
        let mut events = events;
        events.extend(workspaces_read(&["land", "run"]));
        let ledger = adapters::resolve(&config(), events);
        board(&ledger, &config(), at(asof))
    }

    /// A crowded board: six seats with long names, four of them
    /// holding the same long paths unlanded, one with nothing at all.
    fn crowded() -> Board {
        let seats = [
            "proj-0a", "proj-0b", "proj-1a", "proj-1b", "proj-2a", "proj-2b",
        ];
        let toml = seats.iter().fold(String::new(), |mut t, s| {
            let _ = writeln!(t, "[seats.{s}]");
            t
        });
        let config = Config::parse(&toml, Path::new("/")).unwrap_or_else(|e| panic!("{e}"));
        let paths: Vec<String> = (0..17)
            .map(|i| format!("lib/project/integrations/sources/provider_{i}/module.ex"))
            .collect();
        let mut events = workspaces_read(&["land", "run"]);
        for seat in &seats[..4] {
            for p in &paths {
                events.push(unlanded("2026-10-13T09:00:00Z", seat, p));
            }
        }
        for seat in &seats[..5] {
            events.push(event("2026-10-13T08:00:00Z", Kind::Land, seat, None));
        }
        let ledger = adapters::resolve(&config, events);
        board(&ledger, &config, at("2026-10-13T12:00:00Z"))
    }

    #[test]
    fn a_crowded_board_fits_its_terminal_and_names_each_shared_file_once() {
        let b = crowded();
        for width in [80, 120] {
            let display = crate::render::Display {
                width: Some(width),
                ..crate::render::Display::utc()
            };
            let text = crate::render::board_in(&b, &display);
            for line in text.lines() {
                assert!(line.chars().count() <= width, "{width}: {line:?}\n{text}");
            }
            // a flag names whom and how many, never a path
            assert!(
                text.contains("overlaps proj-0b, proj-1a, proj-1b (17 files)"),
                "{text}"
            );
            // the files four seats share are listed once, with all four
            assert_eq!(
                text.matches("proj-0a, proj-0b, proj-1a, proj-1b  17 files: ")
                    .count(),
                1,
                "{text}"
            );
            // the seat with nothing is folded, and named
            assert!(
                text.contains("1 quiet seat (--all shows it): proj-2b"),
                "{text}"
            );
            assert!(!text.lines().any(|l| l.starts_with("proj-2b ")), "{text}");
        }
        // piped: nothing cut, every column kept
        let piped = crate::render::board(&b);
        assert!(piped.lines().next().is_some());
        assert!(
            piped.contains("overlaps proj-0b, proj-1a, proj-1b (17 files)"),
            "{piped}"
        );
        let all = crate::render::Display {
            all: true,
            ..crate::render::Display::utc()
        };
        assert!(
            crate::render::board_in(&b, &all)
                .lines()
                .any(|l| l.starts_with("proj-2b "))
        );
    }

    #[test]
    fn two_seats_changing_one_file_are_flagged() {
        let b = overlap_board(
            vec![
                unlanded("2026-10-13T09:00:00Z", "x-1a", "src/render.rs"),
                unlanded("2026-10-13T09:05:00Z", "x-1a", "src/board.rs"),
                unlanded("2026-10-13T10:00:00Z", "x-1b", "src/render.rs"),
                unlanded("2026-10-13T10:05:00Z", "x-1b", "src/hook.rs"),
            ],
            "2026-10-13T12:00:00Z",
        );
        assert_eq!(b.seats[0].flags, vec!["overlaps x-1b (1 file)"]);
        assert_eq!(b.seats[1].flags, vec!["overlaps x-1a (1 file)"]);
    }

    #[test]
    fn the_flag_counts_the_files_and_json_lists_every_one() {
        let files = ["a.rs", "b.rs", "c.rs", "d.rs"];
        let mut events: Vec<Event> = files
            .iter()
            .flat_map(|f| {
                [
                    unlanded("2026-10-13T09:00:00Z", "x-1a", f),
                    unlanded("2026-10-13T10:00:00Z", "x-1b", f),
                ]
            })
            .collect();
        for e in &mut events {
            e.set_attr(crate::event::attr::SNAPSHOT, "2026-10-13T11:58:00Z");
        }
        let b = overlap_board(events, "2026-10-13T12:00:00Z");
        assert_eq!(b.seats[0].flags, vec!["overlaps x-1b (4 files)"]);
        let json = serde_json::to_value(&b.seats[0].overlaps).unwrap_or_default();
        assert_eq!(
            json,
            serde_json::json!([{"seat": "x-1b", "files": files, "snapshot": "2026-10-13T11:58:00Z"}])
        );
    }

    #[test]
    fn an_unreadable_workspace_makes_the_overlap_unknown() {
        let noon = at("2026-10-13T12:00:00Z");
        let b = overlap_board(
            vec![
                unlanded("2026-10-13T09:00:00Z", "x-1a", "src/render.rs"),
                crate::event::gap_at(
                    noon,
                    &["state:unlanded"],
                    "jj",
                    None,
                    "workspace x-1b: unreadable",
                ),
            ],
            "2026-10-13T12:00:00Z",
        );
        assert!(!b.seats[0].overlaps.is_known());
        assert_eq!(b.seats[0].flags, vec!["overlaps ?"]);
        assert!(crate::render::board(&b).contains("workspace x-1b: unreadable"));
    }

    #[test]
    fn a_past_instant_or_no_jj_says_nothing_of_overlaps() {
        let events = vec![
            unlanded("2026-10-13T09:00:00Z", "x-1a", "src/render.rs"),
            unlanded("2026-10-13T09:00:00Z", "x-1b", "src/render.rs"),
        ];
        // the workspaces were read at noon only: at 11:00 the overlap is
        // unknown, and the text is as it was before overlaps were read
        let b = overlap_board(events, "2026-10-13T11:00:00Z");
        assert!(!b.seats[0].overlaps.is_known());
        assert_eq!(b.seats[0].flags, Vec::<String>::new());
        assert!(!crate::render::board(&b).contains("overlaps"));
        let ledger = adapters::resolve(&config(), vec![covered("1970-01-01T00:00:00Z", &["land"])]);
        let b = board(&ledger, &config(), at("2026-10-13T12:00:00Z"));
        assert!(!b.seats[0].overlaps.is_known());
        assert!(!crate::render::board(&b).contains("overlaps"));
    }
}
