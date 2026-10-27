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

/// How text is shown to its reader: the zone its times print in. Read from
/// the environment at render, like the capture instant; a fold never sees it.
#[derive(Debug, Clone)]
pub struct Display {
    pub zone: TimeZone,
}

impl Display {
    /// UTC: for tests, and a reader who sets `TZ=UTC`.
    #[must_use]
    pub fn utc() -> Self {
        Self {
            zone: TimeZone::UTC,
        }
    }

    /// The reader's local zone (`TZ` honored).
    #[must_use]
    pub fn local() -> Self {
        Self {
            zone: TimeZone::system(),
        }
    }

    /// A time with its zone: `2026-10-26 23:42 PDT`.
    #[must_use]
    pub fn time(&self, t: Timestamp) -> String {
        t.to_zoned(self.zone.clone())
            .strftime("%Y-%m-%d %H:%M %Z")
            .to_string()
    }

    /// A time without its zone, for the start of a range whose end names it.
    #[must_use]
    pub fn bare_time(&self, t: Timestamp) -> String {
        t.to_zoned(self.zone.clone())
            .strftime("%Y-%m-%d %H:%M")
            .to_string()
    }
}

/// A span as a person says it: `1 day`, `3 days`, `7d 6h`, `5h 30m`, `40m`.
#[must_use]
pub fn span(from: Timestamp, to: Timestamp) -> String {
    let secs = to.duration_since(from).as_secs().max(0);
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    match (d, h, m) {
        (1, 0, 0) => "1 day".to_owned(),
        (d, 0, 0) if d > 0 => format!("{d} days"),
        (d, h, _) if d > 0 => format!("{d}d {h}h"),
        (0, h, 0) if h > 0 => format!("{h}h"),
        (0, h, m) if h > 0 => format!("{h}h {m}m"),
        (_, _, m) => format!("{m}m"),
    }
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

/// The scorecard's columns, in order; `scorecard --help` defines each from
/// here. An uncovered gauge is `?`: the ledger cannot say, which is not 0.
pub const SCORECARD: &[Column<SeatCard>] = &[
    Column {
        name: "seat",
        help: "the seat, then the unattributed and total rows.",
        cell: |c, _| c.name.clone(),
    },
    Column {
        name: "lands",
        help: "landings on the trunk.",
        cell: |c, _| count(&c.landings),
    },
    Column {
        name: "runs",
        help: "runner runs.",
        cell: |c, _| c.runs.cell(|r| r.total.to_string()),
    },
    Column {
        name: "fail",
        help: "runs that failed.",
        cell: |c, _| c.runs.cell(|r| r.fail.to_string()),
    },
    Column {
        name: "no-reason",
        help: "failed runs that kept neither a reason nor an output tail.",
        cell: |c, _| c.runs.cell(|r| r.fail_without_reason.to_string()),
    },
    Column {
        name: "runs/land",
        help: "runs per landing (— with no landing).",
        cell: |c, _| c.runs_per_landing.cell(|v| opt(*v)),
    },
    Column {
        name: "go",
        help: "GO verdicts on the seat's changes.",
        cell: |c, _| count(&c.verdicts_go),
    },
    Column {
        name: "return",
        help: "RETURN verdicts on the seat's changes.",
        cell: |c, _| count(&c.verdicts_return),
    },
    Column {
        name: "sent",
        help: "messages the seat sent.",
        cell: |c, _| count(&c.messages_sent),
    },
    Column {
        name: "recv",
        help: "messages the seat received.",
        cell: |c, _| count(&c.messages_received),
    },
    Column {
        name: "goal%",
        help: "the share of sent messages that name a goal.",
        cell: |c, _| {
            both(&c.messages_naming_goal, &c.messages_sent, |n, s| {
                percent(*n, *s)
            })
        },
    },
    Column {
        name: "asks-no-goal",
        help: "decision messages sent (`[messages] decision_kinds`) that name no goal.",
        cell: |c, _| count(&c.decisions_without_goal),
    },
    Column {
        name: "created",
        help: "tracker items the seat created.",
        cell: |c, _| count(&c.items_created),
    },
    Column {
        name: "wip",
        help: "goals of a counted type the seat held claimed and open at the window's end.",
        cell: |c, _| count(&c.wip_at_end),
    },
    Column {
        name: "active-h",
        help: "hours with session activity.",
        cell: |c, _| c.active_hours.cell(|h| format!("{h:.1}")),
    },
];

/// `scorecard --help`, from the columns declared once.
#[must_use]
pub fn scorecard_help() -> String {
    let mut out = String::from(
        "The four gauges per seat over a window: DAY, FROM..TO or FROM..\n\nColumns, per seat, over the window:\n",
    );
    out.push_str(&help(SCORECARD));
    out.push_str(
        "\nBelow the table: flow medians (change start to landing, to first verdict, land gate),\nthe tracker (created, closed, created/closed, open, unowned), the hub, and how well the joins held.\n`?` marks a value the sources do not cover; the reason is listed beneath.",
    );
    out
}

/// The scorecard as text, its times in UTC.
#[must_use]
pub fn scorecard(card: &Scorecard) -> String {
    scorecard_in(card, &Display::utc())
}

/// The scorecard as text, for `display`.
#[must_use]
pub fn scorecard_in(card: &Scorecard, display: &Display) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "scorecard {} .. {} ({})\n",
        display.bare_time(card.from),
        display.time(card.to),
        span(card.from, card.to)
    );
    if let Some(asked) = card.asked_to {
        let _ = writeln!(
            out,
            "counted to {}, when the sources were read; the window asked for ends {}\n",
            display.time(card.to),
            display.time(asked)
        );
    }
    let shown: Vec<&SeatCard> = card
        .seats
        .values()
        .filter(|c| c.configured || c.active)
        .chain([&card.unattributed, &card.total])
        .collect();
    let idle = card.seats.len() + 2 - shown.len();
    out.push_str(&grid(SCORECARD, shown, card.to));
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
        "minutes" => "min",
        other => other,
    }
}

/// A column of a table: its header, the line `--help` defines it with,
/// and its cell. A column is declared once, here, and the table, the hook's
/// line and `--help` all read it.
pub struct Column<R> {
    pub name: &'static str,
    pub help: &'static str,
    pub cell: fn(&R, Timestamp) -> String,
}

impl<R> Clone for Column<R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R> Copy for Column<R> {}

/// `columns` as a table over `rows`, at `now`.
#[must_use]
pub fn grid<'r, R: 'r>(
    columns: &[Column<R>],
    rows: impl IntoIterator<Item = &'r R>,
    now: Timestamp,
) -> String {
    let mut table_rows = vec![columns.iter().map(|c| c.name.to_owned()).collect()];
    for row in rows {
        table_rows.push(columns.iter().map(|c| (c.cell)(row, now)).collect());
    }
    table(&table_rows)
}

/// Each column after the first, defined in one line for `--help`.
#[must_use]
pub fn help<R>(columns: &[Column<R>]) -> String {
    let mut out = String::new();
    for c in columns.iter().skip(1) {
        let _ = writeln!(out, "  {}: {}", c.name, c.help);
    }
    out
}

/// Every unknown gauge in `rows`, as (`label gauge`, reason): read off the
/// output itself, so a cell cannot be `?` without its reason.
#[must_use]
pub fn row_unknowns<R: serde::Serialize>(
    rows: &[R],
    label: fn(&R) -> &str,
) -> Vec<(String, String)> {
    rows.iter()
        .flat_map(|row| {
            let json = serde_json::to_value(row).unwrap_or_default();
            unknowns(&json)
                .into_iter()
                .map(move |(path, why)| (format!("{} {path}", label(row)), why))
        })
        .collect()
}

fn goal_cell(row: &Row, _: Timestamp) -> String {
    row.goals.cell(|goals| match (goals.first(), goals.len()) {
        (None, _) => "—".to_owned(),
        (Some(g), 1) => g.id.clone(),
        (Some(g), n) => format!("{} +{}", g.id, n - 1),
    })
}

/// The lead goal's budget: unknown when the goals are.
fn budget_cell(row: &Row, _: Timestamp) -> String {
    match row.goals.get() {
        None => "?".to_owned(),
        Some(goals) => goals.first().map_or_else(
            || "—".to_owned(),
            |g| {
                g.used.cell(|used| match (g.amount, g.unit.as_deref()) {
                    (Some(amount), Some(unit)) => {
                        format!("{used:.1}/{amount:.0} {}", unit_short(unit))
                    }
                    // the goal states no amount: show what was spent, in
                    // the unit it was counted in
                    _ => format!("{used:.1} {}", unit_short(&g.used_unit)),
                })
            },
        ),
    }
}

fn verdict_cell(row: &Row, now: Timestamp) -> String {
    row.last_verdict.cell(|v| {
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
    })
}

/// The board's columns, in order; `board --help` defines each from here.
pub const BOARD: &[Column<Row>] = &[
    Column {
        name: "seat",
        help: "the seat.",
        cell: |r, _| r.seat.clone(),
    },
    Column {
        name: "goal",
        help: "the goal the seat's latest work cites, then +N for the others it holds.",
        cell: goal_cell,
    },
    Column {
        name: "budget",
        help: "effort spent since the claim / the effort its `## Budget` section allows (agent-hours, hours, minutes, days or runs).",
        cell: budget_cell,
    },
    Column {
        name: "lands",
        help: "the seat's landings on the trunk in the window.",
        cell: |r, _| count(&r.landings),
    },
    Column {
        name: "last",
        help: "how long ago the seat last landed.",
        cell: |r, now| {
            r.last_land
                .cell(|l| l.map_or_else(|| "—".to_owned(), |t| age(t, now)))
        },
    },
    Column {
        name: "runs",
        help: "the seat's runner runs in the window.",
        cell: |r, _| count(&r.runs),
    },
    Column {
        name: "pass",
        help: "the share of those runs that passed.",
        cell: |r, _| both(&r.runs_pass, &r.runs, |p, n| percent(*p, *n)),
    },
    Column {
        name: "since-land",
        help: "runs since the seat's last landing (with none, since landings are first read).",
        cell: |r, _| count(&r.runs_since_land),
    },
    Column {
        name: "verdict",
        help: "the last review verdict on the seat's changes, and its age.",
        cell: verdict_cell,
    },
    Column {
        name: "findings",
        help: "comments on the lead goal since it was claimed.",
        cell: |r, _| count(&r.findings),
    },
    Column {
        name: "state",
        help: "the seat's live session now and for how long: busy (a turn runs) or idle (waiting for a prompt); — when none runs.",
        cell: |r, now| {
            r.state.as_ref().map_or_else(String::new, |g| {
                g.cell(|s| {
                    s.as_ref().map_or_else(
                        || "—".to_owned(),
                        |s| format!("{} {}", s.status, age(s.since, now)),
                    )
                })
            })
        },
    },
    Column {
        name: "flags",
        help: "what needs acting on, below.",
        cell: |r, _| r.flags.join(" · "),
    },
];

/// The columns a goal schema adds; without `[goal]` the board has none.
const GOAL_COLUMNS: &[&str] = &["goal", "budget"];

/// The board's columns: all, or without the goal schema's when no `[goal]`
/// applies, and without `state` when no row read live sessions.
#[must_use]
pub fn board_columns(goal_schema: bool, rows: &[Row]) -> Vec<Column<Row>> {
    let presence = rows.iter().any(|r| r.state.is_some());
    BOARD
        .iter()
        .filter(|c| goal_schema || !GOAL_COLUMNS.contains(&c.name))
        .filter(|c| presence || c.name != "state")
        .copied()
        .collect()
}

/// The board's header row.
#[must_use]
pub fn board_header() -> Vec<String> {
    BOARD.iter().map(|c| c.name.to_owned()).collect()
}

/// One seat's row as cells: every column, but `state` only where live
/// sessions were read.
#[must_use]
pub fn board_row(row: &Row, now: Timestamp) -> Vec<String> {
    board_columns(true, std::slice::from_ref(row))
        .iter()
        .map(|c| (c.cell)(row, now))
        .collect()
}

/// `board --help`, from the columns and flags declared once.
#[must_use]
pub fn board_help() -> String {
    let mut out = String::from(
        "One row per seat: goal, budget spent, landings, runs, verdict, flags.\n\nColumns (counts cover the last `[board] window_hours`, 24 by default):\n",
    );
    out.push_str(&help(BOARD));
    out.push_str("\nFlags:\n");
    for (flag, means) in crate::fold::board::FLAGS {
        let _ = writeln!(out, "  {flag}: {means}");
    }
    out.push_str(
        "\n`?` marks a value the sources do not cover; the reason is listed under the table.",
    );
    out
}

/// The board as text, its times in UTC.
#[must_use]
pub fn board(board: &Board) -> String {
    board_in(board, &Display::utc())
}

/// The board as text, for `display`.
#[must_use]
pub fn board_in(board: &Board, display: &Display) -> String {
    let mut out = format!(
        "board as of {} · counts over the last {}h\n\n",
        display.time(board.asof),
        board.window_hours
    );
    out.push_str(&grid(
        &board_columns(board.goal_schema, &board.seats),
        &board.seats,
        board.asof,
    ));
    out.push_str(&unseated(board));
    let mut entries = row_unknowns(&board.seats, |r| r.seat.as_str());
    // a column the board does not show has no `?` to explain, nor does a
    // flag no source could raise
    let hidden = |path: &str| {
        (!board.goal_schema && path.starts_with("goals"))
            || (!board.overlaps_read && path.starts_with("overlaps"))
    };
    entries.retain(|(key, _)| key.split_once(' ').is_none_or(|(_, path)| !hidden(path)));
    if !entries.is_empty() {
        out.push('\n');
        out.push_str(&reasons(&entries));
    }
    out
}

/// Goals no row shows, claimed by an identity that is no seat: one line per
/// identity, with how many and the fix (`--json` lists every goal).
fn unseated(board: &Board) -> String {
    let mut by_owner: BTreeMap<&str, Vec<&crate::fold::board::Unseated>> = BTreeMap::new();
    for u in board.unseated.get().into_iter().flatten() {
        by_owner.entry(u.owner.as_str()).or_default().push(u);
    }
    if by_owner.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n");
    for (owner, goals) in by_owner {
        let line = match goals.as_slice() {
            [one] => format!(
                "{} is claimed by '{owner}', which is not a seat; {}",
                one.goal, one.fix
            ),
            [first, rest @ ..] => format!(
                "{} goals ({}, +{}) are claimed by '{owner}', which is not a seat; {}",
                goals.len(),
                first.goal,
                rest.len(),
                board.unseated_fix
            ),
            [] => continue,
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
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
    fn spans_read_as_people_say_them() {
        let t = |s: &str| -> Timestamp { s.parse().unwrap_or_else(|e| panic!("{e}")) };
        let from = t("2026-10-20T00:00:00Z");
        assert_eq!(span(from, t("2026-10-21T00:00:00Z")), "1 day");
        assert_eq!(span(from, t("2026-10-23T00:00:00Z")), "3 days");
        assert_eq!(span(from, t("2026-10-27T06:44:00Z")), "7d 6h");
        assert_eq!(span(from, t("2026-10-20T05:30:00Z")), "5h 30m");
        assert_eq!(span(from, t("2026-10-20T00:40:00Z")), "40m");
    }

    #[test]
    fn a_header_time_names_its_zone() {
        let t: Timestamp = "2026-10-27T06:42:00Z"
            .parse()
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(Display::utc().time(t), "2026-10-27 06:42 UTC");
        let pdt = Display {
            zone: TimeZone::get("America/Los_Angeles").unwrap_or_else(|e| panic!("{e}")),
        };
        assert_eq!(pdt.time(t), "2026-10-26 23:42 PDT");
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
