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
    let mut numeric = vec![true; header.len()];
    for row in rows.iter().skip(1) {
        for (i, cell) in row.iter().enumerate().take(numeric.len()) {
            if cell != "—" && cell != "?" && !looks_numeric(cell) {
                numeric[i] = false;
            }
        }
    }
    numeric[0] = false;
    aligned(rows, &numeric)
}

/// Align rows into columns, right-aligning where `right` says; the first row
/// is the header, and the last column is free text.
#[must_use]
pub fn aligned(rows: &[Vec<String>], right: &[bool]) -> String {
    let Some(header) = rows.first() else {
        return String::new();
    };
    let columns = header.len();
    let mut widths = vec![0; columns];
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(columns) {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let numeric: Vec<bool> = (0..columns)
        .map(|i| right.get(i).copied().unwrap_or(false))
        .collect();
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate().take(columns) {
            if i > 0 {
                line.push_str("  ");
            }
            let width = widths[i];
            if numeric[i] {
                let _ = write!(line, "{cell:>width$}");
            } else if i + 1 == columns {
                line.push_str(cell);
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
    /// The columns text must fit, on a terminal or with `COLUMNS` set;
    /// `None` when piped, where nothing is cut.
    pub width: Option<usize>,
    /// Show every seat, the quiet ones too (`--all`, `--seat`).
    pub all: bool,
    /// Paint with ANSI color: on a terminal, unless `NO_COLOR` is set or
    /// `--color` says otherwise. Painted after layout, so width never counts
    /// an escape code.
    pub color: bool,
}

impl Display {
    /// UTC: for tests, and a reader who sets `TZ=UTC`.
    #[must_use]
    pub fn utc() -> Self {
        Self {
            zone: TimeZone::UTC,
            width: None,
            all: false,
            color: false,
        }
    }

    /// The reader's local zone (`TZ` honored), and the width of the
    /// terminal stdout is, if it is one (`COLUMNS` overrides).
    #[must_use]
    pub fn local() -> Self {
        let columns = std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok());
        let terminal =
            || terminal_size::terminal_size_of(std::io::stdout()).map(|(w, _)| usize::from(w.0));
        Self {
            zone: TimeZone::system(),
            width: columns.or_else(terminal).filter(|w| *w >= 20),
            all: false,
            color: auto_color(
                std::io::IsTerminal::is_terminal(&std::io::stdout()),
                std::env::var_os("NO_COLOR").as_deref(),
            ),
        }
    }

    /// `text` in an SGR style when painting, else as it is.
    fn paint(&self, style: Style, text: &str) -> String {
        if self.color && !text.is_empty() {
            format!("\x1b[{}m{text}\x1b[0m", style.code())
        } else {
            text.to_owned()
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

/// Whether to paint by default: only on a terminal, and never where
/// `NO_COLOR` is set to anything (<https://no-color.org>).
#[must_use]
pub fn auto_color(terminal: bool, no_color: Option<&std::ffi::OsStr>) -> bool {
    terminal && no_color.is_none_or(std::ffi::OsStr::is_empty)
}

/// What a painted span means; each maps to one SGR code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Style {
    Dim,
    Bold,
    /// Wants attention: WIP, a stall, an overlap.
    Warn,
    /// Wants acting on now: over budget, a refused land, idle while holding.
    Alarm,
}

impl Style {
    const fn code(self) -> &'static str {
        match self {
            Self::Dim => "2",
            Self::Bold => "1",
            Self::Warn => "33",
            Self::Alarm => "31",
        }
    }
}

/// How loud a flag is.
fn flag_style(flag: &str) -> Style {
    if flag.starts_with("over budget")
        || flag.starts_with("last land refused")
        || flag.starts_with("idle ")
    {
        Style::Alarm
    } else {
        Style::Warn
    }
}

/// The table's lines painted: the header bold, each flagged seat's name and
/// flags in their flag's color, every `?` and `—` dim.
fn paint_table(table: &str, rows: &[&Row], display: &Display) -> String {
    if !display.color {
        return table.to_owned();
    }
    let mut out = String::new();
    for (i, line) in table.lines().enumerate() {
        if i == 0 {
            let _ = writeln!(out, "{}", display.paint(Style::Bold, line));
            continue;
        }
        let row = rows.get(i - 1);
        let mut painted = dim_marks(line, display);
        if let Some(row) = row {
            for flag in &row.flags {
                painted =
                    painted.replacen(flag.as_str(), &display.paint(flag_style(flag), flag), 1);
            }
            if let Some(loudest) = row.flags.iter().map(|f| flag_style(f)).max() {
                painted =
                    painted.replacen(row.seat.as_str(), &display.paint(loudest, &row.seat), 1);
            }
        }
        let _ = writeln!(out, "{painted}");
    }
    out
}

/// `line` with each standalone `?` and `—` dimmed.
fn dim_marks(line: &str, display: &Display) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word == "?" || word == "—" {
            out.push_str(&display.paint(Style::Dim, word));
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in line.chars() {
        if c == ' ' {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
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

/// An age as said: `40m ago`, `5h ago`, `3d ago`.
#[must_use]
pub fn ago(t: Timestamp, now: Timestamp) -> String {
    match age(t, now).as_str() {
        "future" => "future".to_owned(),
        a => format!("{a} ago"),
    }
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
        right: false,
        help: "the seat, then the unattributed and total rows.",
        cell: |c, _| c.name.clone(),
    },
    Column {
        name: "lands",
        right: true,
        help: "landings on the trunk.",
        cell: |c, _| count(&c.landings),
    },
    Column {
        name: "runs",
        right: true,
        help: "runner runs.",
        cell: |c, _| c.runs.cell(|r| r.total.to_string()),
    },
    Column {
        name: "fail",
        right: true,
        help: "runs that failed.",
        cell: |c, _| c.runs.cell(|r| r.fail.to_string()),
    },
    Column {
        name: "unexplained",
        right: true,
        help: "failed runs that kept neither a reason nor an output tail.",
        cell: |c, _| c.runs.cell(|r| r.fail_without_reason.to_string()),
    },
    Column {
        name: "runs/land",
        right: true,
        help: "runs per landing (— with no landing).",
        cell: |c, _| c.runs_per_landing.cell(|v| opt(*v)),
    },
    Column {
        name: "go",
        right: true,
        help: "GO verdicts on the seat's changes.",
        cell: |c, _| count(&c.verdicts_go),
    },
    Column {
        name: "return",
        right: true,
        help: "RETURN verdicts on the seat's changes.",
        cell: |c, _| count(&c.verdicts_return),
    },
    Column {
        name: "sent",
        right: true,
        help: "messages the seat sent.",
        cell: |c, _| count(&c.messages_sent),
    },
    Column {
        name: "recv",
        right: true,
        help: "messages the seat received.",
        cell: |c, _| count(&c.messages_received),
    },
    Column {
        name: "on-goal",
        right: true,
        help: "the share of sent messages that name a goal.",
        cell: |c, _| {
            both(&c.messages_naming_goal, &c.messages_sent, |n, s| {
                percent(*n, *s)
            })
        },
    },
    Column {
        name: "asks-off-goal",
        right: true,
        help: "decision messages sent (`[messages] decision_kinds`) that name no goal.",
        cell: |c, _| count(&c.decisions_without_goal),
    },
    Column {
        name: "created",
        right: true,
        help: "tracker items the seat created.",
        cell: |c, _| count(&c.items_created),
    },
    Column {
        name: "wip",
        right: true,
        help: "goals of a counted type the seat held claimed and open at the window's end.",
        cell: |c, _| count(&c.wip_at_end),
    },
    Column {
        name: "active-h",
        right: true,
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
    let mut out = display.paint(
        Style::Bold,
        &format!(
            "scorecard {} .. {} ({})",
            display.bare_time(card.from),
            display.time(card.to),
            span(card.from, card.to)
        ),
    );
    out.push_str("\n\n");
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
    let tables = stacked(SCORECARD, SCORECARD_GROUPS, &shown, card.to, display.width);
    for line in tables.lines() {
        let painted = if line.starts_with("seat ") {
            display.paint(Style::Bold, line)
        } else {
            dim_marks(line, display)
        };
        let _ = writeln!(out, "{painted}");
    }
    if idle > 0 {
        let _ = writeln!(
            out,
            "({idle} more accounts appear with nothing counted; --json lists them)"
        );
    }
    out.push('\n');

    for line in summary(card).lines() {
        let reason = line.starts_with("? ");
        let text = wrap(line, display.width, if reason { 2 } else { 9 });
        let _ = writeln!(
            out,
            "{}",
            if reason {
                display.paint(Style::Dim, &text)
            } else {
                text
            }
        );
    }
    out
}

/// Where the scorecard's column groups begin: runs and verdicts, messages,
/// the tracker and activity.
const SCORECARD_GROUPS: &[&str] = &["lands", "sent", "created"];

/// `columns` over `rows` as one table if it fits `width`, else as tables
/// stacked by group (each led by the first column), as many groups to a
/// table as fit.
fn stacked<R>(
    columns: &[Column<R>],
    groups: &[&str],
    rows: &[&R],
    now: Timestamp,
    width: Option<usize>,
) -> String {
    let whole = grid(columns, rows.iter().copied(), now);
    if fits(&whole, width) {
        return whole;
    }
    let mut parts: Vec<Vec<Column<R>>> = Vec::new();
    for c in columns.iter().skip(1) {
        if groups.contains(&c.name) || parts.is_empty() {
            parts.push(Vec::new());
        }
        if let Some(part) = parts.last_mut() {
            part.push(*c);
        }
    }
    let mut tables: Vec<String> = Vec::new();
    let mut current: Vec<Column<R>> = vec![columns[0]];
    for part in parts {
        let mut wider = current.clone();
        wider.extend(part.iter().copied());
        if current.len() == 1 || fits(&grid(&wider, rows.iter().copied(), now), width) {
            current = wider;
        } else {
            tables.push(grid(&current, rows.iter().copied(), now));
            current = vec![columns[0]];
            current.extend(part);
        }
    }
    tables.push(grid(&current, rows.iter().copied(), now));
    tables.join("\n")
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
        let gauge = gauge_column(gauge).to_owned();
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

/// The column a gauge's JSON path shows in, so a `?` line names what the
/// reader sees: `last_verdict` is `verdict`, a goal's `used` is `budget`.
fn gauge_column(path: &str) -> &str {
    if path.starts_with("goals.") && path.split('.').nth(2) == Some("used") {
        return "budget";
    }
    match path.split('.').next().unwrap_or(path) {
        "goals" => "goal",
        "landings" => "lands",
        "last_land" => "landed",
        "runs_pass" => "pass",
        "runs_since_land" => "unlanded",
        "findings" => "comments",
        "last_verdict" => "verdict",
        "overlaps" => "flags",
        "messages_sent" => "sent",
        "messages_received" => "recv",
        "messages_naming_goal" => "on-goal",
        "decisions_without_goal" => "asks-off-goal",
        "items_created" => "created",
        "wip_at_end" => "wip",
        "active_hours" => "active-h",
        "verdicts_go" => "go",
        "verdicts_return" => "return",
        "runs_per_landing" => "runs/land",
        other => other,
    }
}

fn unit_short(unit: &str) -> &str {
    match unit {
        "agent-hours" => "agent-h",
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
    /// Right-aligned (a count, a share, an age), whatever its cells hold.
    pub right: bool,
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
    let right: Vec<bool> = columns.iter().map(|c| c.right).collect();
    aligned(&table_rows, &right)
}

/// Whether every line of `text` fits `width` (no width: always).
fn fits(text: &str, width: Option<usize>) -> bool {
    width.is_none_or(|w| text.lines().all(|l| l.chars().count() <= w))
}

/// `text` wrapped to `width` at its spaces, each line after the first
/// indented by `indent`; unwrapped with no width.
#[must_use]
pub fn wrap(text: &str, width: Option<usize>, indent: usize) -> String {
    let Some(width) = width else {
        return text.to_owned();
    };
    let mut out = String::new();
    let (mut len, mut fresh) = (0, true);
    for word in text.split(' ') {
        let n = visible(word);
        if !fresh && len + 1 + n > width {
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            len = indent;
        } else if !fresh {
            out.push(' ');
            len += 1;
        }
        out.push_str(word);
        len += n;
        fresh = false;
    }
    out
}

/// How many columns `text` takes on a terminal: its chars, less any ANSI
/// escape sequence.
fn visible(text: &str) -> usize {
    let mut n = 0;
    let mut escape = false;
    for c in text.chars() {
        match (escape, c) {
            (false, '\x1b') => escape = true,
            (true, 'm') => escape = false,
            (true, _) => {}
            (false, _) => n += 1,
        }
    }
    n
}

/// `items` after `lead`, as many as fit `width`, then `+N`; with no width,
/// the first `cap`.
fn fit_list(lead: &str, items: &[String], width: Option<usize>, cap: usize) -> String {
    let mut out = lead.to_owned();
    for (i, item) in items.iter().enumerate() {
        let rest = items.len() - i - 1;
        let more = if rest > 0 {
            format!(", +{rest}")
        } else {
            String::new()
        };
        let sep = if i == 0 { "" } else { ", " };
        let room = width.map_or(i < cap, |w| {
            out.chars().count() + sep.len() + item.chars().count() + more.chars().count() <= w
        });
        if !room && i > 0 {
            let _ = write!(out, ", +{}", items.len() - i);
            return out;
        }
        out.push_str(sep);
        match width {
            // the first item alone is too long: keep its end, the part that
            // tells one path from another
            Some(w) if !room => {
                let left = w.saturating_sub(out.chars().count() + more.chars().count() + 1);
                let n = item.chars().count();
                let tail: String = item.chars().skip(n.saturating_sub(left)).collect();
                out.push('…');
                out.push_str(&tail);
            }
            _ => out.push_str(item),
        }
    }
    out
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
                    ago(v.t, now)
                )
            },
        )
    })
}

/// The board's columns, in order; `board --help` defines each from here.
pub const BOARD: &[Column<Row>] = &[
    Column {
        name: "seat",
        right: false,
        help: "the seat.",
        cell: |r, _| r.seat.clone(),
    },
    Column {
        name: "goal",
        right: false,
        help: "the goal the seat's latest work cites, then +N for the others it holds.",
        cell: goal_cell,
    },
    Column {
        name: "budget",
        right: false,
        help: "effort spent since the claim / the effort its `## Budget` section allows (agent-hours, hours, minutes, days or runs).",
        cell: budget_cell,
    },
    Column {
        name: "lands",
        right: true,
        help: "the seat's landings on the trunk in the window.",
        cell: |r, _| count(&r.landings),
    },
    Column {
        name: "landed",
        right: true,
        help: "how long ago the seat last landed.",
        cell: |r, now| {
            r.last_land
                .cell(|l| l.map_or_else(|| "—".to_owned(), |t| ago(t, now)))
        },
    },
    Column {
        name: "runs",
        right: true,
        help: "the seat's runner runs in the window.",
        cell: |r, _| count(&r.runs),
    },
    Column {
        name: "pass",
        right: true,
        help: "the share of those runs that passed.",
        cell: |r, _| both(&r.runs_pass, &r.runs, |p, n| percent(*p, *n)),
    },
    Column {
        name: "unlanded",
        right: true,
        help: "runs since the seat's last landing (with none, since landings are first read).",
        cell: |r, _| count(&r.runs_since_land),
    },
    Column {
        name: "verdict",
        right: false,
        help: "the last review verdict on the seat's changes, and its age.",
        cell: verdict_cell,
    },
    Column {
        name: "comments",
        right: true,
        help: "comments on the lead goal since it was claimed (its findings).",
        cell: |r, _| count(&r.findings),
    },
    Column {
        name: "state",
        right: false,
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
        right: false,
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

/// Columns the board gives up, in this order, when it cannot fit its
/// width any other way.
const BOARD_DROPS: &[&str] = &["comments", "unlanded", "pass", "landed", "state", "budget"];

/// A seat with nothing to show: no goal (where the board shows goals),
/// nothing landed or run in the window, no flag, no turn running. Unknown
/// is never quiet.
fn quiet(row: &Row, goal_schema: bool) -> bool {
    let zero = |g: &Gauge<usize>| g.get().is_some_and(|n| *n == 0);
    let busy = row
        .state
        .as_ref()
        .and_then(Gauge::get)
        .is_some_and(|s| s.as_ref().is_some_and(|s| s.status == "busy"));
    (!goal_schema || row.goals.get().is_some_and(Vec::is_empty))
        && zero(&row.landings)
        && (zero(&row.runs) || !row.runs.is_known())
        && row.flags.is_empty()
        && !busy
}

/// The board as text, for `display`: fitted to its width, quiet seats
/// folded into one line, overlapping files and every `?` explained beneath.
#[must_use]
pub fn board_in(board: &Board, display: &Display) -> String {
    let width = display.width;
    let mut out = display.paint(
        Style::Bold,
        &format!(
            "board as of {} · counts over the last {}h",
            display.time(board.asof),
            board.window_hours
        ),
    );
    out.push_str("\n\n");
    let (shown, folded): (Vec<&Row>, Vec<&Row>) = board
        .seats
        .iter()
        .partition(|r| display.all || !quiet(r, board.goal_schema));
    let shown_seats: Vec<&str> = shown.iter().map(|r| r.seat.as_str()).collect();
    let mut entries = row_unknowns(&board.seats, |r| r.seat.as_str());
    // a column the board does not show has no `?` to explain, nor does a
    // flag no source could raise, nor a seat folded away
    let hidden = |path: &str| {
        (!board.goal_schema && path.starts_with("goals"))
            || (!board.overlaps_read && path.starts_with("overlaps"))
    };
    entries.retain(|(key, _)| {
        key.split_once(' ')
            .is_none_or(|(seat, path)| !hidden(path) && shown_seats.contains(&seat))
    });
    let mut columns = board_columns(board.goal_schema, &board.seats);
    let unsourced = unsourced(&mut columns, &shown, &mut entries);
    let (table, flags_beneath, dropped) = fitted(columns, &shown, board.asof, width);
    out.push_str(&paint_table(&table, &shown, display));
    if flags_beneath {
        out.push_str(&flags_list(&shown, display));
    }
    if !dropped.is_empty() {
        let note = format!(
            "(hidden to fit {} columns: {}; a wider terminal or a pipe shows them)",
            width.unwrap_or_default(),
            dropped.join(", ")
        );
        let _ = writeln!(
            out,
            "\n{}",
            display.paint(Style::Dim, &wrap(&note, width, 1))
        );
    }
    if !folded.is_empty() {
        let names: Vec<String> = folded.iter().map(|r| r.seat.clone()).collect();
        let n = names.len();
        let lead = format!(
            "{n} quiet {} (--all shows {}): ",
            if n == 1 { "seat" } else { "seats" },
            if n == 1 { "it" } else { "them" }
        );
        let _ = writeln!(
            out,
            "\n{}",
            display.paint(Style::Dim, &fit_list(&lead, &names, width, 6))
        );
    }
    out.push_str(&overlapping(&shown, display));
    if !unsourced.is_empty() {
        let note = format!("not shown, no source reads them: {}", unsourced.join(", "));
        let _ = writeln!(
            out,
            "\n{}",
            display.paint(Style::Dim, &wrap(&note, width, 2))
        );
    }
    if !entries.is_empty() {
        out.push('\n');
        for line in reasons(&entries).lines() {
            let _ = writeln!(out, "{}", display.paint(Style::Dim, &wrap(line, width, 2)));
        }
    }
    out.push_str(&unseated(board, display));
    out
}

/// Take out of `columns` each that no source feeds, `?` on every shown row
/// for want of a source: it is named once, not shown, and its `?` lines go.
fn unsourced(
    columns: &mut Vec<Column<Row>>,
    shown: &[&Row],
    entries: &mut Vec<(String, String)>,
) -> Vec<&'static str> {
    let mut names = Vec::new();
    columns.retain(|c| {
        let unread = |seat: &str| {
            entries.iter().any(|(key, why)| {
                key.split_once(' ').is_some_and(|(s, path)| {
                    s == seat && gauge_column(path) == c.name && why.starts_with("no source reads")
                })
            })
        };
        let none = !shown.is_empty() && shown.iter().all(|r| unread(&r.seat));
        if none {
            names.push(c.name);
        }
        !none
    });
    entries.retain(|(key, _)| {
        key.split_once(' ')
            .is_none_or(|(_, path)| !names.contains(&gauge_column(path)))
    });
    names
}

/// The table, fitted to `width`: too wide, the flags go beneath it, then
/// the least needed columns go. Returns whether the flags went, and which
/// columns.
fn fitted(
    mut columns: Vec<Column<Row>>,
    shown: &[&Row],
    asof: Timestamp,
    width: Option<usize>,
) -> (String, bool, Vec<&'static str>) {
    let mut table = grid(&columns, shown.iter().copied(), asof);
    let mut dropped = Vec::new();
    if fits(&table, width) {
        return (table, false, dropped);
    }
    columns.retain(|c| c.name != "flags");
    table = grid(&columns, shown.iter().copied(), asof);
    for name in BOARD_DROPS {
        if fits(&table, width) {
            break;
        }
        if columns.iter().any(|c| c.name == *name) {
            columns.retain(|c| c.name != *name);
            dropped.push(*name);
            table = grid(&columns, shown.iter().copied(), asof);
        }
    }
    (table, true, dropped)
}

/// Each flagged seat's flags, one seat to a line, wrapped under the table.
fn flags_list(shown: &[&Row], display: &Display) -> String {
    let flagged: Vec<&&Row> = shown.iter().filter(|r| !r.flags.is_empty()).collect();
    if flagged.is_empty() {
        return String::new();
    }
    let pad = flagged
        .iter()
        .map(|r| r.seat.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = format!("\n{}\n", display.paint(Style::Bold, "flags"));
    for row in flagged {
        let loudest = row
            .flags
            .iter()
            .map(|f| flag_style(f))
            .max()
            .unwrap_or(Style::Warn);
        let flags: Vec<String> = row
            .flags
            .iter()
            .map(|f| display.paint(flag_style(f), f))
            .collect();
        let seat = format!("{:<pad$}", row.seat);
        let line = format!("  {}  {}", display.paint(loudest, &seat), flags.join(" · "));
        let _ = writeln!(out, "{}", wrap(&line, display.width, pad + 4));
    }
    out
}

/// The unlanded files seats' workspaces share: each set of files once,
/// with every seat that holds it, as many paths as fit.
fn overlapping(rows: &[&Row], display: &Display) -> String {
    let width = display.width;
    let mut sets: BTreeMap<&[String], Vec<&str>> = BTreeMap::new();
    for row in rows {
        for o in row.overlaps.get().into_iter().flatten() {
            let holders = sets.entry(o.files.as_slice()).or_default();
            for seat in [row.seat.as_str(), o.seat.as_str()] {
                if !holders.contains(&seat) {
                    holders.push(seat);
                }
            }
        }
    }
    if sets.is_empty() {
        return String::new();
    }
    let mut lines: Vec<(String, &[String])> = sets
        .into_iter()
        .map(|(files, mut seats)| {
            seats.sort_unstable();
            (seats.join(", "), files)
        })
        .collect();
    lines.sort();
    let pad = lines
        .iter()
        .map(|(who, _)| who.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = format!(
        "\n{}\n",
        display.paint(Style::Bold, "unlanded files shared by workspaces")
    );
    for (who, files) in lines {
        let who = if width.is_some_and(|w| pad + 24 > w) {
            // too many seats to share a line with paths: they get their own
            let _ = writeln!(out, "  {}", wrap(&who, width.map(|w| w - 2), 2));
            String::new()
        } else {
            format!("{who:<pad$}  ")
        };
        let count = if files.len() > 3 {
            format!("{} files: ", files.len())
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "{}",
            fit_list(&format!("  {who}{count}"), files, width, 3)
        );
    }
    out
}

/// Goals no row shows, claimed by an identity that is no seat: one line per
/// identity, with how many and the fix (`--json` lists every goal).
fn unseated(board: &Board, display: &Display) -> String {
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
        out.push_str(&display.paint(Style::Warn, &wrap(&line, display.width, 2)));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Los Angeles as a POSIX rule: a build sandbox has no zone database.
    const LOS_ANGELES: &str = "PST8PDT,M3.2.0,M11.1.0";

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
    fn color_is_for_a_terminal_without_no_color() {
        use std::ffi::OsStr;
        assert!(auto_color(true, None));
        assert!(auto_color(true, Some(OsStr::new(""))));
        assert!(!auto_color(true, Some(OsStr::new("1"))));
        assert!(!auto_color(false, None));
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
            zone: TimeZone::posix(LOS_ANGELES).unwrap_or_else(|e| panic!("{e}")),
            ..Display::utc()
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
