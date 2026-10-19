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
use crate::fold::coverage::Coverage;
use crate::fold::gauge::{Gauge, Gauger};
use crate::fold::{Index, Item, round2};
use crate::ledger::Ledger;
use crate::time::minutes;

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
}

/// A goal claimed by no seat, and by whom.
#[derive(Debug, Clone, Serialize)]
pub struct Unseated {
    pub goal: String,
    pub owner: String,
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
    /// Flags read only known gauges: a flag never speaks where the ledger
    /// could not look.
    pub flags: Vec<String>,
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
    /// The seat's latest commit or landing that cites this goal, as read:
    /// only the key that puts the goal being worked first, not a gauge, so
    /// not in the output.
    #[serde(skip)]
    pub last_work: Option<Timestamp>,
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
    let index = Index::new(&ledger);
    let window_hours = config.board.window_hours;
    let since = asof
        .checked_sub(SignedDuration::from_secs_f64(window_hours * 3_600.0))
        .unwrap_or(Timestamp::MIN);
    let items = index.items_at(asof);

    let mut seats: Vec<String> = config.seats.keys().cloned().collect();
    let by_seat = events_by_seat(&ledger);
    let coverage = Coverage::new(&ledger);
    if config.seats.is_empty() {
        // no seats configured: a row for every seat active in the window or
        // holding a goal (a stalled seat is the one the board is for)
        let holding: Vec<&str> = items
            .values()
            .filter(|i| i.claimed && !i.closed)
            .filter_map(|i| i.owner.as_deref())
            .collect();
        for (seat, events) in &by_seat {
            if events.iter().any(|e| e.t >= since) || holding.contains(seat) {
                seats.push((*seat).to_owned());
            }
        }
    }
    let g = Gauger {
        coverage: &coverage,
        from: since,
        to: asof,
    };
    let rows = seats
        .into_iter()
        .map(|seat| {
            let events = by_seat.get(seat.as_str()).cloned().unwrap_or_default();
            let landings_from = coverage.covered_from("land", Some(&seat));
            let tally = tally(&events, since, landings_from);
            let held = goals(&g, &seat, &items, &events, config);
            let findings = findings(&ledger, &held);
            row(
                &g,
                seat,
                &tally,
                held,
                findings,
                &events,
                config,
                landings_from,
            )
        })
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
                })
            })
            .collect()
    };
    Board {
        schema: 2,
        asof,
        window_hours,
        unseated: g.gauge("board.unseated", None, None, unseated),
        seats: rows,
    }
}

/// The only way a row is made: every value through its registered gauge.
#[allow(clippy::too_many_arguments)]
fn row(
    g: &Gauger<'_>,
    seat: String,
    t: &Tally,
    held: Vec<Goal>,
    findings: usize,
    events: &[&Event],
    config: &Config,
    landings_from: Option<Timestamp>,
) -> Row {
    let s = Some(seat.as_str());
    // findings count from the lead goal's claim; with no goal held there is none
    // to find, which is known where comments are read at the instant
    let first_claim = held.first().and_then(|x| x.claimed_at);
    let goals = g.gauge("row.goals", s, None, held);
    let findings = g.over(
        &goals,
        "row.findings",
        s,
        first_claim.or(Some(g.to)),
        findings,
    );
    let mut row = Row {
        goals,
        landings: g.gauge("row.landings", s, None, t.landings),
        // a last landing is known if nothing after it went unread; no
        // landing at all, only if all history was read
        last_land: g.gauge(
            "row.last_land",
            s,
            Some(t.last_land.unwrap_or(ALL_HISTORY)),
            t.last_land,
        ),
        runs: g.gauge("row.runs", s, None, t.runs),
        runs_pass: g.gauge("row.runs_pass", s, None, t.runs_pass),
        runs_since_land: g.gauge(
            "row.runs_since_land",
            s,
            t.since_land_origin,
            t.runs_since_land,
        ),
        runs_from: t.runs_from,
        last_verdict: g.gauge(
            "row.last_verdict",
            s,
            Some(t.last_verdict.as_ref().map_or(ALL_HISTORY, |v| v.t)),
            t.last_verdict.clone(),
        ),
        findings,
        flags: Vec::new(),
        seat,
    };
    row.flags = flags(&row, events, config, g, landings_from);
    row
}

fn events_by_seat(ledger: &Ledger) -> BTreeMap<&str, Vec<&Event>> {
    let mut by_seat: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for event in ledger.events() {
        if let Some(seat) = event.seat.as_deref() {
            by_seat.entry(seat).or_default().push(event);
        }
    }
    by_seat
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
    let mut goals: Vec<Goal> = items
        .iter()
        .filter(|(_, item)| item.claimed && !item.closed && item.owner.as_deref() == Some(seat))
        .map(|(id, item)| {
            let (amount, unit) = item
                .budget
                .as_deref()
                .and_then(crate::goal::parse_budget)
                .map_or((None, None), |(n, u)| (Some(n), Some(u)));
            let unit_read = unit.as_deref().unwrap_or(&config.board.budget_unit);
            let path = match unit_read {
                "runs" => "row.goals.used.runs",
                "wall-clock" => "row.goals.used.wall",
                _ => "row.goals.used",
            };
            let used = g.gauge(
                path,
                Some(seat),
                item.claimed_at,
                item.claimed_at
                    .map_or(0.0, |claimed| spent(unit_read, events, claimed, g.to)),
            );
            Goal {
                id: id.clone(),
                title: item.title.clone(),
                claimed_at: item.claimed_at,
                budget: item.budget.clone(),
                counts: item.in_progress(config),
                amount,
                unit,
                used,
                last_work: None,
            }
        })
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
        _ => {
            events
                .iter()
                .filter(|e| e.kind == Kind::SessionActive)
                .map(|e| {
                    let start = e.t.max(from);
                    let end = e.t_end.unwrap_or(e.t).min(to);
                    if end > start {
                        minutes(start, end)
                    } else {
                        0.0
                    }
                })
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
    events: &[&Event],
    config: &Config,
    g: &Gauger<'_>,
    landings_from: Option<Timestamp>,
) -> Vec<String> {
    let mut flags = Vec::new();
    let seat = Some(row.seat.as_str());
    let asof = g.to;
    let wip = config.wip_for(&row.seat);
    let held = row.goals.get().map_or(&[][..], Vec::as_slice);
    let counted = held.iter().filter(|g| g.counts).count();
    if counted > wip {
        flags.push(format!("{counted} goals in progress, WIP {wip}"));
    }
    for goal in held {
        if let (Some(amount), Some(used)) = (goal.amount, goal.used.get())
            && *used > amount
        {
            flags.push(format!("over budget on {}", goal.id));
        }
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
    // a refused land run after the last landing: only where the last landing
    // is known, so no later landing went unread
    if row.last_land.is_known()
        && let Some(run) = events
            .iter()
            .rev()
            .find(|e| e.kind == Kind::GateRun && e.attr_str(attr::RECIPE) == Some("land"))
        && last_land.is_none_or(|l| run.t > l)
        && matches!(run.outcome, Some(Outcome::Fail | Outcome::Refused))
    {
        flags.push("last land refused".to_owned());
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
            covered("1970-01-01T00:00:00Z", &["create", "claim", "close"]),
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
                covered("1970-01-01T00:00:00Z", &["create", "claim", "close"]),
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
                &["create", "claim", "close", "other:comment"],
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
            covered("1970-01-01T00:00:00Z", &["create", "claim", "close"]),
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
            covered("1970-01-01T00:00:00Z", &["create", "claim", "close"]),
        ];
        let ledger = adapters::resolve(&config(), events);
        let b = board(&ledger, &config(), at("2026-10-13T12:00:00Z"));
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
    }
}
