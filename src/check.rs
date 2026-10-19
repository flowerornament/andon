//! The refusals (founding design §7). Each is called from a choke point the
//! project already owns (its land verb, its goal creation, its messenger's
//! send), prints one line, and exits 1 when it refuses. The line names the fix.
//!
//! | check            | refuses when                                         |
//! |------------------|------------------------------------------------------|
//! | `check commit`   | no goal cited; the goal is unknown, closed, or not   |
//! |                  | claimed by the committing seat; the seat is past WIP |
//! | `check goal`     | a section is missing; the done-when cannot fail; the |
//! |                  | budget is unbounded; the pool is at its cap        |
//! | `check message`  | a decision kind names no goal, or a closed one       |

use std::collections::BTreeMap;

use jiff::Timestamp;

use crate::commit::Commit;
use crate::config::Config;
use crate::event::attr;
use crate::fold::{Index, Item};
use crate::goal;
use crate::ledger::Ledger;

/// A check's answer: the line to print, and whether it refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Ok(String),
    /// Passes (exit 0) with advice.
    Warn(String),
    Refused(String),
}

impl Answer {
    #[must_use]
    pub fn line(&self) -> String {
        match self {
            Self::Ok(s) => format!("ok: {s}"),
            Self::Warn(s) => format!("warn: {s}"),
            Self::Refused(s) => format!("refused: {s}"),
        }
    }

    #[must_use]
    pub const fn refused(&self) -> bool {
        matches!(self, Self::Refused(_))
    }
}

/// `check commit`: the commit cites a goal its seat holds, within WIP.
#[must_use]
pub fn commit(commit: &Commit, ledger: &Ledger, config: &Config, now: Timestamp) -> Answer {
    let short = commit.id.get(..12).unwrap_or(&commit.id);
    let items = Index::new(ledger).items_at(now);
    let held = |seat: &str| -> Vec<&String> {
        items
            .iter()
            .filter(|(_, i)| i.in_progress(config) && i.owner.as_deref() == Some(seat))
            .map(|(id, _)| id)
            .collect()
    };
    let Ok(pattern) = config.goal_regex() else {
        return Answer::Refused(
            "project.goal_pattern does not compile; fix it in andon.toml".into(),
        );
    };
    let candidates = goal::candidates(&pattern, &commit.title, &commit.message, config);
    // the likeliest citation the tracker has; else the likeliest at all
    let known = candidates.iter().find(|g| items.contains_key(*g)).cloned();
    let Some(goal) = known.or_else(|| candidates.first().cloned()) else {
        let example = commit
            .seat
            .as_deref()
            .and_then(|s| held(s).first().map(|g| (*g).clone()))
            .unwrap_or_else(|| "goal-id".into());
        return Answer::Refused(format!(
            "{short} cites no goal; end its subject with ({example}): jj describe -r {short}"
        ));
    };
    let Some(seat) = commit.seat.as_deref() else {
        return Answer::Refused(format!(
            "{short} names no seat; describe it from your workspace so its trailers name you, or pass --seat"
        ));
    };
    let Some(item) = items.get(&goal) else {
        return Answer::Refused(format!(
            "{short} cites {goal}, which the tracker does not have; cite an existing goal"
        ));
    };
    if let Some(refusal) = held_by(item, &goal, seat, config) {
        return Answer::Refused(format!("{short} cites {goal}, but {refusal}"));
    }
    let holding = held(seat);
    let wip = config.wip_for(seat);
    if holding.len() > wip {
        let list: Vec<&str> = holding.iter().map(|g| g.as_str()).collect();
        return Answer::Refused(format!(
            "{seat} holds {} goals, WIP {wip}; land or park one of: {}",
            holding.len(),
            list.join(", ")
        ));
    }
    Answer::Ok(format!("{short} cites {goal}, claimed by {seat}"))
}

/// Why `seat` may not land on the goal, or `None` when it may.
fn held_by(item: &Item, goal: &str, seat: &str, config: &Config) -> Option<String> {
    if item.closed {
        return Some(format!("{goal} is closed; cite an open goal, or reopen it"));
    }
    match (&item.owner, item.claimed) {
        (Some(owner), true) if owner == seat => None,
        // claimed by an identity that is no seat (a shared default user):
        // "claim it" would loop, so name the identity fix instead
        (Some(owner), true) if !config.seats.is_empty() && !config.seats.contains_key(owner) => {
            Some(format!(
                "{goal} is claimed by '{owner}', which is not a seat; {}",
                crate::seat::identity_hint(config, goal, seat)
            ))
        }
        (Some(owner), true) => Some(format!(
            "{goal} is claimed by {owner}, not {seat}; cite a goal you hold"
        )),
        _ => Some(format!(
            "{goal} is not claimed; claim it: {}",
            crate::seat::claim_hint(config, goal)
        )),
    }
}

/// `check goal`: the description is a goal, and its pool has room.
#[must_use]
pub fn goal(
    id: &str,
    description: &str,
    ledger: &Ledger,
    config: &Config,
    now: Timestamp,
) -> Answer {
    let problems = goal::problems(description, config);
    if let Some(first) = problems.first() {
        let more = if problems.len() > 1 {
            format!(" (and {} more)", problems.len() - 1)
        } else {
            String::new()
        };
        return Answer::Refused(format!("{id}: {first}{more}"));
    }
    let sections = goal::sections(description);
    if let Some(pool) = goal::section(&sections, "pool").map(str::trim)
        && let Some(cap) = config.pools.get(pool).and_then(|p| p.cap)
    {
        let open = open_in_pool(ledger, pool, id, now);
        if open.len() >= cap {
            return Answer::Refused(format!(
                "pool {pool} holds {} open goals, cap {cap}; land or defer one of: {}",
                open.len(),
                open.join(", ")
            ));
        }
    }
    if let Some(advice) = goal::warnings(description, config).first() {
        return Answer::Warn(format!("{id}: {advice}"));
    }
    Answer::Ok(format!(
        "{id} has its sections, a done-when naming a check, and a finite budget"
    ))
}

/// Open goals in a pool, other than `except`.
fn open_in_pool(ledger: &Ledger, pool: &str, except: &str, now: Timestamp) -> Vec<String> {
    let mut pools: BTreeMap<&str, &str> = BTreeMap::new();
    for e in ledger.events() {
        if let (Some(id), Some(p)) = (e.subject.as_deref(), e.attr_str(attr::POOL)) {
            pools.insert(id, p.trim());
        }
    }
    let items = Index::new(ledger).items_at(now);
    items
        .iter()
        .filter(|(id, item)| !item.closed && id.as_str() != except)
        .filter(|(id, _)| pools.get(id.as_str()) == Some(&pool))
        .map(|(id, _)| id.clone())
        .collect()
}

/// `check message`: a decision names the goal it serves.
#[must_use]
pub fn message(
    kind: &str,
    goal: Option<&str>,
    text: &str,
    ledger: &Ledger,
    config: &Config,
    now: Timestamp,
) -> Answer {
    let kind = kind.to_lowercase();
    let kind = kind.as_str();
    let known = &config.messages.kinds;
    if !known.is_empty() && !known.iter().any(|k| k.eq_ignore_ascii_case(kind)) {
        return Answer::Refused(format!(
            "{kind} is not a kind the messenger defines; use one of: {}",
            known.join(", ")
        ));
    }
    let decision = config
        .messages
        .decision_kinds
        .iter()
        .any(|k| k.eq_ignore_ascii_case(kind));
    let found = goal
        .map(str::to_owned)
        .or_else(|| goal::find(&config.goal_regex().ok()?, text));
    let Some(goal) = found else {
        if decision {
            return Answer::Refused(format!(
                "{} {kind} carries a decision; name the goal it serves (--goal <id>, or its id in the headline)",
                article(kind)
            ));
        }
        return Answer::Ok(format!(
            "{} {kind} is not a decision; no goal needed",
            article(kind)
        ));
    };
    let items = Index::new(ledger).items_at(now);
    // with a tracker in the ledger, a goal it does not have is no goal
    if decision && !items.is_empty() && !items.contains_key(&goal) {
        return Answer::Refused(format!(
            "{} {kind} names {goal}, which the tracker does not have; name an existing goal",
            article(kind)
        ));
    }
    if decision && items.get(&goal).is_some_and(|i| i.closed) {
        return Answer::Refused(format!(
            "{goal} is closed; {} {kind} about it decides nothing. Name an open goal",
            article(kind)
        ));
    }
    Answer::Ok(format!("{} {kind} about {goal}", article(kind)))
}

fn article(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::adapters;
    use crate::event::{Event, Kind};
    use crate::time::parse_instant;

    fn at(s: &str) -> Timestamp {
        parse_instant(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn config(extra: &str) -> Config {
        Config::parse(
            &format!(
                "[project]\ngoal_pattern = '\\bandon-[a-z0-9]{{3}}\\b'\n[seats.andon-1a]\n[seats.andon-1b]\n[adapters.bd]\ndir = \".\"\n{extra}"
            ),
            Path::new("/"),
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn item(kind: Kind, t: &str, id: &str, actor: Option<&str>) -> Event {
        let mut e = Event::new(at(t), kind, "bd");
        e.subject = Some(id.into());
        e.ids.goal = Some(id.into());
        e.actor = actor.map(str::to_owned);
        e
    }

    fn ledger(config: &Config) -> Ledger {
        adapters::resolve(
            config,
            vec![
                item(Kind::Create, "2026-10-13T09:00:00Z", "andon-xa4", None),
                item(
                    Kind::Claim,
                    "2026-10-13T09:01:00Z",
                    "andon-xa4",
                    Some("andon-1b"),
                ),
                item(Kind::Create, "2026-10-13T09:00:00Z", "andon-76c", None),
                item(Kind::Create, "2026-10-13T09:00:00Z", "andon-old", None),
                item(Kind::Close, "2026-10-13T09:30:00Z", "andon-old", None),
                item(Kind::Create, "2026-10-13T09:00:00Z", "andon-sh1", None),
                item(
                    Kind::Claim,
                    "2026-10-13T09:03:00Z",
                    "andon-sh1",
                    Some("Flower Ornament"),
                ),
                item(Kind::Create, "2026-10-13T09:00:00Z", "andon-1a1", None),
                item(
                    Kind::Claim,
                    "2026-10-13T09:02:00Z",
                    "andon-1a1",
                    Some("andon-1a"),
                ),
            ],
        )
    }

    fn check(title: &str, seat: Option<&str>, extra: &str) -> Answer {
        let c = config(extra);
        let commit = Commit {
            id: "7d1c241adb66aaaa".into(),
            title: title.into(),
            message: title.into(),
            seat: seat.map(str::to_owned),
        };
        super::commit(&commit, &ledger(&c), &c, at("2026-10-13T12:00:00Z"))
    }

    #[test]
    fn a_commit_citing_a_held_goal_passes() {
        assert_eq!(
            check("board: rows (andon-xa4)", Some("andon-1b"), ""),
            Answer::Ok("7d1c241adb66 cites andon-xa4, claimed by andon-1b".into())
        );
    }

    #[test]
    fn an_uncited_commit_is_refused_with_the_seats_goal_as_the_fix() {
        assert_eq!(
            check("board: rows", Some("andon-1b"), "").line(),
            "refused: 7d1c241adb66 cites no goal; end its subject with (andon-xa4): jj describe -r 7d1c241adb66"
        );
    }

    #[test]
    fn a_goal_held_by_another_seat_is_refused() {
        assert_eq!(
            check("board: rows (andon-1a1)", Some("andon-1b"), "").line(),
            "refused: 7d1c241adb66 cites andon-1a1, but andon-1a1 is claimed by andon-1a, not andon-1b; cite a goal you hold"
        );
    }

    #[test]
    fn an_epic_held_does_not_count_toward_wip() {
        // andon-1b holds andon-xa4 and, as its lane's owner, an epic: WIP 1
        // is not exceeded, since only feature, bug and task count
        let c = config("");
        let mut events = ledger(&c).events().to_vec();
        let mut epic = item(Kind::Create, "2026-10-13T08:00:00Z", "andon-ep1", None);
        epic.set_attr(attr::ITEM_TYPE, "epic");
        events.push(epic);
        events.push(item(
            Kind::Claim,
            "2026-10-13T08:01:00Z",
            "andon-ep1",
            Some("andon-1b"),
        ));
        let l = adapters::resolve(&c, events);
        let commit = Commit {
            id: "7d1c241adb66aaaa".into(),
            title: "x (andon-xa4)".into(),
            message: "x (andon-xa4)".into(),
            seat: Some("andon-1b".into()),
        };
        assert!(
            super::commit(&commit, &l, &c, at("2026-10-13T12:00:00Z"))
                .line()
                .starts_with("ok:"),
            "{}",
            super::commit(&commit, &l, &c, at("2026-10-13T12:00:00Z")).line()
        );
    }

    #[test]
    fn a_goal_claimed_by_no_seat_names_the_identity_fix() {
        // a shared actor (bd's default user) claimed it: saying "claim it"
        // would loop, since claiming again assigns the same actor
        assert_eq!(
            check("x (andon-sh1)", Some("andon-1b"), "").line(),
            "refused: 7d1c241adb66 cites andon-sh1, but andon-sh1 is claimed by 'Flower Ornament', which is not a seat; set BEADS_ACTOR=andon-1b or bd update andon-sh1 --assignee andon-1b"
        );
    }

    #[test]
    fn unclaimed_closed_and_unknown_goals_are_refused() {
        assert_eq!(
            check("x (andon-76c)", Some("andon-1b"), "").line(),
            "refused: 7d1c241adb66 cites andon-76c, but andon-76c is not claimed; claim it: bd update andon-76c --claim"
        );
        assert!(
            check("x (andon-old)", Some("andon-1b"), "")
                .line()
                .contains("andon-old is closed")
        );
        assert!(
            check("x (andon-zzz)", Some("andon-1b"), "")
                .line()
                .contains("the tracker does not have")
        );
        assert!(
            check("x (andon-xa4)", None, "")
                .line()
                .contains("names no seat")
        );
    }

    #[test]
    fn a_seat_past_its_wip_is_refused() {
        let c = config("");
        let mut events: Vec<Event> = ledger(&c).events().to_vec();
        events.push(item(
            Kind::Claim,
            "2026-10-13T10:00:00Z",
            "andon-76c",
            Some("andon-1b"),
        ));
        let l = adapters::resolve(&c, events);
        let commit = Commit {
            id: "abc".into(),
            title: "x (andon-xa4)".into(),
            message: String::new(),
            seat: Some("andon-1b".into()),
        };
        assert_eq!(
            super::commit(&commit, &l, &c, at("2026-10-13T12:00:00Z")).line(),
            "refused: andon-1b holds 2 goals, WIP 1; land or park one of: andon-76c, andon-xa4"
        );
    }

    const GOAL: &str = "## Why\n\nx\n\n## Done when\n\n`just check` passes\n\n## Budget\n\n4 agent-hours\n\n## Pool\n\ncore\n";

    #[test]
    fn goal_check_refuses_a_missing_section_and_a_full_pool() {
        let c = config("[pools.core]\ncap = 1\nseats = [\"andon-1b\"]\n");
        let mut events: Vec<Event> = ledger(&c).events().to_vec();
        let mut pooled = item(Kind::Create, "2026-10-13T09:00:00Z", "andon-76c", None);
        pooled.set_attr(attr::POOL, "core");
        events.push(pooled);
        let l = adapters::resolve(&c, events);
        let now = at("2026-10-13T12:00:00Z");
        assert_eq!(
            goal("andon-new", "## Why\n\nx\n", &l, &c, now).line(),
            "refused: andon-new: add a \"## Done when\" section (and 1 more)"
        );
        assert_eq!(
            goal("andon-new", GOAL, &l, &c, now).line(),
            "refused: pool core holds 1 open goals, cap 1; land or defer one of: andon-76c"
        );
        let roomy = config("[pools.core]\ncap = 2\nseats = [\"andon-1b\"]\n");
        assert!(!goal("andon-new", GOAL, &l, &roomy, now).refused());
    }

    #[test]
    fn message_check_refuses_a_decision_without_a_goal() {
        let c = config("");
        let l = ledger(&c);
        let now = at("2026-10-13T12:00:00Z");
        assert_eq!(
            message("ask", None, "can you review?", &l, &c, now).line(),
            "refused: an ask carries a decision; name the goal it serves (--goal <id>, or its id in the headline)"
        );
        assert!(!message("ask", None, "review andon-xa4?", &l, &c, now).refused());
        assert!(!message("fyi", None, "status", &l, &c, now).refused());
        // case does not hide a decision, and a typo is not a kind
        assert!(message("GO", None, "", &l, &c, now).refused());
        assert_eq!(
            message("rulling", None, "", &l, &c, now).line(),
            "refused: rulling is not a kind the messenger defines; use one of: ask, go, nogo, ruling, block, stop, fyi, done, announce, hold, release"
        );
        assert!(message("go", Some("andon-old"), "", &l, &c, now).refused());
        assert_eq!(
            message("go", Some("andon-zzz"), "", &l, &c, now).line(),
            "refused: a go names andon-zzz, which the tracker does not have; name an existing goal"
        );
    }
}
