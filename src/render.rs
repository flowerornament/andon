//! Text rendering. Plain columns, no color: stdout is read by agents and
//! hooks as often as by people. `--json` is the stable machine form.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::fold::board::{Board, Row};
use crate::fold::gauge::{Gauge, unknowns};
use crate::fold::scorecard::{Scorecard, SeatCard};

/// Align rows into columns; the first row is the header. A column whose
/// cells are all numbers (or `—`) aligns right; the last column is free text.
#[must_use]
pub fn table(rows: &[Vec<String>]) -> String {
    let Some(header) = rows.first() else {
        return String::new();
    };
    let columns = header.len();
    let mut widths = vec![0; columns];
    let mut numeric = vec![true; columns];
    for (r, row) in rows.iter().enumerate() {
        for (i, cell) in row.iter().enumerate().take(columns) {
            widths[i] = widths[i].max(cell.chars().count());
            if r > 0 && cell != "—" && cell != "?" && !looks_numeric(cell) {
                numeric[i] = false;
            }
        }
    }
    numeric[0] = false;
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate().take(columns) {
            if i > 0 {
                line.push_str("  ");
            }
            let width = widths[i];
            if i + 1 == columns {
                line.push_str(cell);
            } else if numeric[i] {
                let _ = write!(line, "{cell:>width$}");
            } else {
                let _ = write!(line, "{cell:<width$}");
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn looks_numeric(cell: &str) -> bool {
    !cell.is_empty()
        && cell
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '%' | '-' | '/'))
}

/// A short UTC time: `2026-10-11 07:28`.
#[must_use]
pub fn short_time(t: Timestamp) -> String {
    t.to_zoned(TimeZone::UTC)
        .strftime("%Y-%m-%d %H:%M")
        .to_string()
}

/// An age relative to `now`: `40m`, `5h`, `3d`.
#[must_use]
pub fn age(t: Timestamp, now: Timestamp) -> String {
    let secs = now.duration_since(t).as_secs();
    if secs < 0 {
        return "future".into();
    }
    if secs < 3_600 {
        format!("{}m", secs / 60)
    } else if secs < 172_800 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn opt(x: Option<f64>) -> String {
    x.map_or_else(|| "—".to_owned(), |v| format!("{v:.2}"))
}

fn minutes(x: Option<f64>) -> String {
    x.map_or_else(|| "—".to_owned(), |v| format!("{v:.1} min"))
}

#[allow(clippy::cast_precision_loss)]
fn percent(part: usize, whole: usize) -> String {
    if whole == 0 {
        "—".into()
    } else {
        format!("{:.0}%", 100.0 * part as f64 / whole as f64)
    }
}

/// A cell over two gauges: `?` unless both are known.
fn both<A, B>(a: &Gauge<A>, b: &Gauge<B>, show: impl FnOnce(&A, &B) -> String) -> String {
    match (a.get(), b.get()) {
        (Some(a), Some(b)) => show(a, b),
        _ => "?".to_owned(),
    }
}

fn count(g: &Gauge<usize>) -> String {
    g.cell(ToString::to_string)
}

fn card_row(name: &str, c: &SeatCard) -> Vec<String> {
    // an uncovered gauge is `?`: the ledger cannot say, which is not 0
    vec![
        name.to_owned(),
        count(&c.landings),
        c.runs.cell(|r| r.total.to_string()),
        c.runs.cell(|r| r.fail.to_string()),
        c.runs.cell(|r| r.fail_without_reason.to_string()),
        c.runs_per_landing.cell(|v| opt(*v)),
        count(&c.verdicts_go),
        count(&c.verdicts_return),
        count(&c.messages_sent),
        count(&c.messages_received),
        both(&c.messages_naming_goal, &c.messages_sent, |n, s| {
            percent(*n, *s)
        }),
        count(&c.decisions_without_goal),
        count(&c.items_created),
        count(&c.wip_at_end),
        c.active_hours.cell(|h| format!("{h:.1}")),
    ]
}

/// The scorecard's columns; `scorecard --help` defines each.
pub const SCORECARD_HEADER: &[&str] = &[
    "seat",
    "lands",
    "runs",
    "fail",
    "no-reason",
    "runs/land",
    "go",
    "return",
    "sent",
    "recv",
    "goal%",
    "asks-no-goal",
    "created",
    "wip",
    "active-h",
];

/// The scorecard as text.
#[must_use]
pub fn scorecard(card: &Scorecard) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "scorecard {} .. {} ({} days)\n",
        short_time(card.from),
        short_time(card.to),
        card.days
    );
    if card.asked_to.is_some() {
        let _ = writeln!(
            out,
            "counted to {}; the window ends in the future\n",
            short_time(card.to)
        );
    }
    let mut rows = vec![
        SCORECARD_HEADER
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>(),
    ];
    let mut idle = 0;
    for (seat, c) in &card.seats {
        if c.configured || c.active {
            rows.push(card_row(seat, c));
        } else {
            idle += 1;
        }
    }
    rows.push(card_row("(unattributed)", &card.unattributed));
    rows.push(card_row("total", &card.total));
    out.push_str(&table(&rows));
    if idle > 0 {
        let _ = writeln!(
            out,
            "({idle} more accounts appear with nothing counted; --json lists them)"
        );
    }
    out.push('\n');

    out.push_str(&summary(card));
    out
}

/// The scorecard's summary lines: flow, tracker, hub, joins, repositories.
fn summary(card: &Scorecard) -> String {
    let mut out = String::new();
    let f = &card.flow;
    let _ = writeln!(
        out,
        "flow     change start → land {} (n {}) · → first verdict {} (n {}) · land gate {} (n {})",
        f.lead_median_min.cell(|v| minutes(*v)),
        count(&f.lead_n),
        f.review_median_min.cell(|v| minutes(*v)),
        count(&f.review_n),
        f.gate_median_min.cell(|v| minutes(*v)),
        count(&f.gate_n)
    );
    let t = &card.tracker;
    let _ = writeln!(
        out,
        "tracker  created {} · closed {} · created/closed {} · open at end {} · unowned {}",
        count(&t.created),
        count(&t.closed),
        t.branching.cell(|v| opt(*v)),
        count(&t.open_at_end),
        count(&t.unowned_at_end)
    );
    match card.hub.get() {
        None => {
            let _ = writeln!(out, "hub      ?");
        }
        Some(Some(h)) => {
            let _ = writeln!(
                out,
                "hub      {}: {} in ({}/day) · {} decisions without a goal · first reply median {} (n {})",
                h.seat,
                h.inbound,
                h.inbound_per_day,
                h.decisions_without_goal,
                minutes(h.reply_median_min),
                h.reply_n
            );
        }
        Some(None) => {}
    }
    let j = &card.joins;
    let _ = writeln!(
        out,
        "joins    landing → session {}/{} (seen {}) · verdict citations {}/{} on {} landings · runs attributed {}/{}",
        count(&j.landing_session),
        count(&j.landings),
        count(&j.landing_session_seen),
        count(&j.cites_resolved),
        count(&j.cites),
        count(&j.landings_citing_verdict),
        count(&j.runs_attributed),
        count(&j.runs)
    );
    if let Some(repos) = card.runs_by_repo.get()
        && repos.keys().any(|r| r != "(none)")
    {
        let parts: Vec<String> = repos
            .iter()
            .map(|(repo, r)| {
                format!(
                    "{repo} {} ({} fail, {} kept no reason)",
                    r.total, r.fail, r.fail_without_reason
                )
            })
            .collect();
        let _ = writeln!(out, "repos    {}", parts.join(" · "));
    }
    // why each `?` is unknown, read off the output itself, so a gauge the
    // text shows cannot be unknown without its reason
    let json = serde_json::to_value(card).unwrap_or_default();
    let shown = |seat: &str| {
        card.seats
            .get(seat)
            .is_some_and(|c| c.configured || c.active)
    };
    let entries: Vec<(String, String)> = unknowns(&json)
        .into_iter()
        .filter_map(|(path, why)| {
            let key = if let Some(rest) = path.strip_prefix("seats.") {
                let (seat, gauge) = rest.rsplit_once('.')?;
                if !shown(seat) {
                    return None;
                }
                format!("{seat} {gauge}")
            } else if let Some(gauge) = path.strip_prefix("total.") {
                format!("total {gauge}")
            } else if let Some(gauge) = path.strip_prefix("unattributed.") {
                format!("(unattributed) {gauge}")
            } else {
                path
            };
            // the text shows no gate-run column; flow's gate line covers it
            (!key.ends_with(" gate_runs")).then_some((key, why))
        })
        .collect();
    out.push_str(&reasons(&entries));
    out
}

/// Each reason once, with how much it hides: `? landings, runs (11 seats):
/// landings are read from …`.
fn reasons(entries: &[(String, String)]) -> String {
    let mut by_reason: BTreeMap<&str, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for (key, why) in entries {
        let (who, gauge) = key
            .split_once(' ')
            .map_or(("", key.as_str()), |(w, g)| (w, g));
        let (gauges, seats) = by_reason.entry(why.as_str()).or_default();
        let gauge = if gauge.starts_with("goals.") && gauge.split('.').nth(2) == Some("used") {
            "budget".to_owned()
        } else {
            gauge.split('.').next().unwrap_or(gauge).to_owned()
        };
        if !gauges.contains(&gauge) {
            gauges.push(gauge);
        }
        if !who.is_empty() && !seats.contains(&who.to_owned()) {
            seats.push(who.to_owned());
        }
    }
    let mut out = String::new();
    for (why, (gauges, seats)) in by_reason {
        let whose = match seats.len() {
            0 => String::new(),
            1..=3 => format!(" ({})", seats.join(", ")),
            n => format!(" ({n} rows)"),
        };
        let _ = writeln!(out, "? {}{whose}: {why}", gauges.join(", "));
    }
    out
}

fn unit_short(unit: &str) -> &str {
    match unit {
        "agent-hours" => "ah",
        "wall-clock" => "h",
        other => other,
    }
}

/// The board's header row.
#[must_use]
pub fn board_header() -> Vec<String> {
    [
        "seat",
        "goal",
        "budget",
        "lands",
        "last",
        "runs",
        "pass",
        "since-land",
        "verdict",
        "findings",
        "flags",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// One seat's row as cells.
#[must_use]
pub fn board_row(row: &Row, now: Timestamp) -> Vec<String> {
    let goal_cell = row.goals.cell(|goals| match (goals.first(), goals.len()) {
        (None, _) => "—".to_owned(),
        (Some(g), 1) => g.id.clone(),
        (Some(g), n) => format!("{} +{}", g.id, n - 1),
    });
    // budget is the lead goal's: unknown when the goals are
    let budget = match row.goals.get() {
        None => "?".to_owned(),
        Some(goals) => goals.first().map_or_else(
            || "—".to_owned(),
            |g| {
                g.used.cell(|used| match (g.amount, g.unit.as_deref()) {
                    (Some(amount), Some(unit)) => {
                        format!("{used:.1}/{amount:.0} {}", unit_short(unit))
                    }
                    // the goal states no amount: show what was spent
                    _ => format!("{used:.1} ah"),
                })
            },
        ),
    };
    let verdict = row.last_verdict.cell(|v| {
        v.as_ref().map_or_else(
            || "—".to_owned(),
            |v| {
                format!(
                    "{} {}",
                    v.outcome.map_or("verdict", |o| o.as_str()).to_uppercase(),
                    age(v.t, now)
                )
            },
        )
    });
    vec![
        row.seat.clone(),
        goal_cell,
        budget,
        count(&row.landings),
        row.last_land
            .cell(|l| l.map_or_else(|| "—".to_owned(), |t| age(t, now))),
        count(&row.runs),
        both(&row.runs_pass, &row.runs, |p, r| percent(*p, *r)),
        count(&row.runs_since_land),
        verdict,
        count(&row.findings),
        if row.flags.is_empty() {
            String::new()
        } else {
            row.flags.join(" · ")
        },
    ]
}

/// The board as text.
#[must_use]
pub fn board(board: &Board) -> String {
    let mut out = format!(
        "board as of {} · counts over the last {}h\n\n",
        short_time(board.asof),
        board.window_hours
    );
    let mut rows = vec![board_header()];
    for row in &board.seats {
        rows.push(board_row(row, board.asof));
    }
    out.push_str(&table(&rows));
    // goals no row shows: claimed by an identity that is no seat
    for u in board.unseated.get().into_iter().flatten() {
        let _ = writeln!(
            out,
            "\n{} is claimed by '{}', which is not a seat; {}",
            u.goal,
            u.owner,
            crate::seat::identity_hint_any(&u.goal)
        );
    }
    let entries = row_reasons(board);
    if !entries.is_empty() {
        out.push('\n');
        out.push_str(&reasons(&entries));
    }
    out
}

/// Every unknown gauge on the board, as (`seat gauge`, reason).
fn row_reasons(board: &Board) -> Vec<(String, String)> {
    let json = serde_json::to_value(board).unwrap_or_default();
    unknowns(&json)
        .into_iter()
        .filter_map(|(path, why)| {
            let rest = path.strip_prefix("seats.")?;
            let (i, gauge) = rest.split_once('.')?;
            let seat = &board.seats.get(i.parse::<usize>().ok()?)?.seat;
            Some((format!("{seat} {gauge}"), why))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_align_text_left_and_numbers_right() {
        let row = |a: &str, b: &str, c: &str| vec![a.to_owned(), b.to_owned(), c.to_owned()];
        let rows = vec![
            row("seat", "runs", "flags"),
            row("x-seat-2b", "479", "479 runs since last landing"),
            row("x", "—", ""),
        ];
        assert_eq!(
            table(&rows),
            "seat       runs  flags\nx-seat-2b   479  479 runs since last landing\nx             —\n"
        );
    }

    #[test]
    fn ages_pick_a_unit() {
        let now: Timestamp = "2026-10-13T00:00:00Z"
            .parse()
            .unwrap_or_else(|e| panic!("{e}"));
        let t = |s: &str| -> Timestamp { s.parse().unwrap_or_else(|e| panic!("{e}")) };
        assert_eq!(age(t("2026-10-12T23:20:00Z"), now), "40m");
        assert_eq!(age(t("2026-10-12T19:00:00Z"), now), "5h");
        assert_eq!(age(t("2026-10-10T00:00:00Z"), now), "3d");
    }
}
