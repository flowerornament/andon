//! The lists as text: `andon goals` and `andon seats`. Plain on a pipe;
//! each table goes through `render::aligned`, and only a title is cut, to
//! fit a terminal.

use std::fmt::Write as _;

use crate::fold::goals::{Ask, Goals, Listed};
use crate::fold::seats::Roster;
use crate::render::{self, Display};

/// A budget as the board shows it: spent of allowed, in its unit.
fn budget(g: &Listed) -> String {
    let short = |u: &str| match u {
        "agent-hours" => "agent-h".to_owned(),
        "wall-clock" => "h".to_owned(),
        "minutes" => "min".to_owned(),
        other => other.to_owned(),
    };
    match (&g.used, g.amount, g.unit.as_deref()) {
        (None, Some(amount), Some(unit)) => format!("—/{amount:.0} {}", short(unit)),
        (None, _, _) => "—".to_owned(),
        (Some(used), amount, unit) => used.cell(|u| match (amount, unit) {
            (Some(a), Some(unit)) => format!("{u:.1}/{a:.0} {}", short(unit)),
            (_, Some(unit)) => format!("{u:.1} {}", short(unit)),
            _ => format!("{u:.1}"),
        }),
    }
}

/// A kind as a reader names it: `comment`, not `other:comment`.
fn kind(name: &str) -> &str {
    name.strip_prefix("other:").unwrap_or(name)
}

/// `path` with the home directory written `~`.
fn home(path: &str) -> String {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .and_then(|h| path.strip_prefix(&h).map(|rest| format!("~{rest}")))
        .unwrap_or_else(|| path.to_owned())
}

/// `text` cut from the left to `max` characters, starting with `…` when
/// cut: a path keeps the end that tells it apart.
fn cut_left(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().skip(n + 1 - max.max(1)).collect();
    format!("…{kept}")
}

/// `text` cut to `max` characters, ending in `…` when cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// A table whose last column is cut so each line fits `width`.
fn fitted(mut rows: Vec<Vec<String>>, right: &[bool], width: Option<usize>) -> String {
    let Some(width) = width else {
        return render::aligned(&rows, right);
    };
    let last = rows.first().map_or(0, Vec::len).saturating_sub(1);
    // the width every column but the last takes, with its separators
    let lead = (0..last)
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0) + 2)
        .sum::<usize>();
    let room = width.saturating_sub(lead).max(12);
    for row in rows.iter_mut().skip(1) {
        if let Some(cell) = row.get_mut(last) {
            *cell = cut(cell, room);
        }
    }
    render::aligned(&rows, right)
}

/// The line beneath a list that leaves out the goals no one holds.
fn unheld(list: &Goals, ask: &Ask<'_>) -> Option<String> {
    let n = list.unheld.filter(|n| *n > 0)?;
    if ask.open || ask.all || ask.seat.is_some() {
        return None;
    }
    let goals = if n == 1 { "goal" } else { "goals" };
    let assigned = match list.assigned.unwrap_or(0) {
        0 => String::new(),
        k => format!(", {k} of them assigned"),
    };
    Some(format!(
        "{n} open {goals} no one holds{assigned} (--open lists them)\n"
    ))
}

/// The footnote for owners that are no seat, naming each with its count.
fn unseated(goals: &[Listed]) -> Option<String> {
    let mut held: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for g in goals.iter().filter(|g| !g.seated) {
        if let Some(owner) = g.owner.as_deref() {
            *held.entry(owner).or_default() += 1;
        }
    }
    if held.is_empty() {
        return None;
    }
    let mut held: Vec<_> = held.into_iter().collect();
    held.sort_by_key(|(owner, n)| (std::cmp::Reverse(*n), *owner));
    let names: Vec<String> = held
        .iter()
        .map(|(owner, n)| format!("{owner} ({n})"))
        .collect();
    let is = if names.len() == 1 {
        "is no seat"
    } else {
        "are no seats"
    };
    Some(format!(
        "* {} {is}; `andon seats` names the fix",
        names.join(", ")
    ))
}

/// `andon goals` as text.
#[must_use]
pub fn goals(list: &Goals, ask: &Ask<'_>, display: &Display) -> String {
    let now = list.asof;
    let Some(goals) = list.goals.get() else {
        return format!(
            "goals: ? {}\n",
            list.goals.reason().unwrap_or("the tracker is not read")
        );
    };
    if goals.is_empty() {
        let none = if ask.open || ask.all {
            "no goals\n"
        } else {
            "no goals held\n"
        };
        return none.to_owned() + &unheld(list, ask).unwrap_or_default();
    }
    let mut rows = vec![
        ["goal", "owner", "status", "budget", "claimed", "title"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for g in goals {
        let owner = match (&g.owner, g.seated) {
            (None, _) => "—".to_owned(),
            (Some(o), true) => o.clone(),
            // an identity that is no seat is marked, as the board names it
            (Some(o), false) => format!("{o}*"),
        };
        rows.push(vec![
            g.id.clone(),
            owner,
            g.status.to_owned(),
            budget(g),
            g.claimed_at
                .map_or_else(|| "—".to_owned(), |t| render::age(t, now)),
            g.title.clone().unwrap_or_default(),
        ]);
    }
    let mut out = fitted(
        rows,
        &[false, false, false, true, true, false],
        display.width,
    );
    // a `?` is never without its reason
    let mut why: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for g in goals {
        if let Some(reason) = g.used.as_ref().and_then(|u| u.reason()) {
            why.entry(reason).or_default().push(&g.id);
        }
    }
    if !why.is_empty() {
        out.push('\n');
    }
    for (reason, ids) in why {
        let whose = match ids.as_slice() {
            [one] => (*one).to_owned(),
            [first, rest @ ..] => format!("{first}, +{}", rest.len()),
            [] => String::new(),
        };
        let line = format!("? budget ({whose}): {reason}");
        out.push_str(&render::wrap(&line, display.width, 2));
        out.push('\n');
    }
    if let Some(note) = unseated(goals) {
        out.push('\n');
        out.push_str(&render::wrap(&note, display.width, 2));
        out.push('\n');
    }
    if let Some(line) = unheld(list, ask) {
        out.push('\n');
        out.push_str(&line);
    }
    out
}

/// `andon seats` as text.
#[must_use]
pub fn seats(roster: &Roster, display: &Display) -> String {
    let now = roster.asof;
    let mut out = String::new();
    let mut rows = vec![
        ["seat", "aliases", "session", "workspace", "joined"]
            .map(str::to_owned)
            .to_vec(),
    ];
    let joined_head = format!("joined ({}h)", roster.window_hours);
    rows[0][4] = joined_head;
    for s in &roster.seats {
        let names: Vec<String> = s
            .aliases
            .iter()
            .cloned()
            .chain(s.prefixes.iter().map(|p| format!("{p}*")))
            .collect();
        let joined: Vec<String> = s
            .joined
            .iter()
            .map(|(k, n)| format!("{n} {}", kind(k)))
            .collect();
        rows.push(vec![
            s.seat.clone(),
            if names.is_empty() {
                "—".to_owned()
            } else {
                names.join(", ")
            },
            s.session.as_ref().map_or_else(
                || "—".to_owned(),
                |p| format!("{} {}", p.status, render::age(p.since, now)),
            ),
            s.workspace.as_deref().map_or_else(|| "—".to_owned(), home),
            if joined.is_empty() {
                "—".to_owned()
            } else {
                joined.join(", ")
            },
        ]);
    }
    if let Some(width) = display.width {
        // a workspace path gives way before what joined, down to its tail
        let widest = |rows: &[Vec<String>]| {
            (0..5)
                .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0) + 2)
                .sum::<usize>()
        };
        let over = widest(&rows).saturating_sub(width + 2);
        let path = rows.iter().map(|r| r[3].chars().count()).max().unwrap_or(0);
        let keep = path.saturating_sub(over).max(24);
        for row in rows.iter_mut().skip(1) {
            row[3] = cut_left(&row[3], keep);
        }
    }
    out.push_str(&fitted(rows, &[false; 5], display.width));
    // a name with only messages is a correspondent from elsewhere, not a
    // seat's name gone astray
    let (letters, strays): (Vec<_>, Vec<_>) = roster
        .unseated
        .iter()
        .partition(|id| id.facts.keys().all(|k| k == "message"));
    if !strays.is_empty() {
        let _ = writeln!(
            out,
            "\nnames the sources recorded that join no seat, most facts first:"
        );
        let mut names = strays;
        names.sort_by_key(|id| {
            (
                std::cmp::Reverse(id.facts.values().sum::<usize>()),
                id.identity.clone(),
            )
        });
        for id in names {
            let facts: Vec<String> = id
                .facts
                .iter()
                .map(|(k, n)| format!("{n} {}", kind(k)))
                .collect();
            let line = format!("  {}: {}", id.identity, facts.join(", "));
            out.push_str(&render::wrap(&line, display.width, 4));
            out.push('\n');
        }
        let fix = "fix: add each name to the aliases of the seat it is ([seats.<seat>] aliases), or make its tool record a seat's name";
        out.push_str(&render::wrap(fix, display.width, 2));
        out.push('\n');
    }
    if !letters.is_empty() {
        let names: Vec<&str> = letters.iter().map(|id| id.identity.as_str()).collect();
        let line = format!(
            "\nmessages from {} name{} outside this project's seats: {}",
            names.len(),
            if names.len() == 1 { "" } else { "s" },
            names.join(", ")
        );
        out.push_str(&render::wrap(&line, display.width, 2));
        out.push('\n');
    }
    out
}
