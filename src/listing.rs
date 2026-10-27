//! The lists as text: `andon goals` and `andon seats`. Plain on a pipe;
//! each table goes through `render::aligned`, and only a title is cut, to
//! fit a terminal.

use std::fmt::Write as _;

use crate::fold::goals::{Goals, Listed};
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

/// `andon goals` as text.
#[must_use]
pub fn goals(list: &Goals, display: &Display) -> String {
    let now = list.asof;
    let Some(goals) = list.goals.get() else {
        return format!(
            "goals: ? {}\n",
            list.goals.reason().unwrap_or("the tracker is not read")
        );
    };
    if goals.is_empty() {
        return "no goals\n".to_owned();
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
    if goals.iter().any(|g| g.owner.is_some() && !g.seated) {
        out.push_str("\n* not a seat: `andon seats` names the fix\n");
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
            .map(|(kind, n)| format!("{n} {kind}"))
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
            s.workspace.clone().unwrap_or_else(|| "—".to_owned()),
            if joined.is_empty() {
                "—".to_owned()
            } else {
                joined.join(", ")
            },
        ]);
    }
    out.push_str(&fitted(rows, &[false; 5], display.width));
    if !roster.unseated.is_empty() {
        let _ = writeln!(
            out,
            "\nnames the sources recorded that join no seat, most facts first:"
        );
        let mut names: Vec<_> = roster.unseated.iter().collect();
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
                .map(|(kind, n)| format!("{n} {kind}"))
                .collect();
            let line = format!("  {}: {}", id.identity, facts.join(", "));
            out.push_str(&render::wrap(&line, display.width, 4));
            out.push('\n');
        }
        let fix = "fix: add each name to the aliases of the seat it is ([seats.<seat>] aliases), or make its tool record a seat's name";
        out.push_str(&render::wrap(fix, display.width, 2));
        out.push('\n');
    }
    out
}
