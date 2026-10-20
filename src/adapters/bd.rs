//! tracker: bd. Reads `bd export` (issues with their comments), which bd
//! prints without writing anything (founding design §5).
//!
//! Emits, per issue: `create` at its creation (by its creator, with the goal's
//! title and budget), `claim` at its start (by its assignee), `close` at its
//! close. Per comment: `verdict` when the comment is a verdict (the configured
//! prefix, then JSON), else `other:comment`, the goal's findings.
//!
//! An export is current state: a claim later released shows only as its
//! present owner, and of a goal closed twice only the last close shows. So
//! it covers claims and closes as the state at the capture instant
//! (`state:claim`, `state:close`), never as history.
//!
//! History comes from bd's events table, where bd can serve SQL (`bd sql`;
//! an embedded database cannot): `claim`, `close`, `reopen` and `assign`,
//! each row's stored time resolved by [`Clock`], replacing the export's
//! claims and closes. A change the table never recorded is reconciled
//! against the export.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;

use crate::adapters::run_tool_in;
use crate::config::{BdConfig, Config};
use crate::event::attr;
use crate::event::{Event, Kind, Outcome};
use crate::goal;
use crate::time::parse_instant;

/// Read the tracker.
///
/// # Errors
/// When bd cannot run or prints something other than JSONL.
pub fn collect(config: &Config, bc: &BdConfig) -> Result<Vec<Event>> {
    let dir = config.path(&bc.dir);
    // bd runs in its directory: one that cannot be read is the fault, not bd
    if let Err(e) = std::fs::read_dir(&dir) {
        anyhow::bail!(
            "cannot read {} ({e}); fix its permissions, or point [adapters.bd] dir at the project",
            dir.display()
        );
    }
    let out = run_tool_in(
        &dir,
        "bd",
        &["export"],
        "install bd, or remove [adapters.bd] from andon.toml",
    )?;
    let since = bc.since.as_deref().map(parse_instant).transpose()?;
    let source = format!("bd export ({})", dir.display());
    let mut events = Vec::new();
    let mut current = HashMap::new();
    for (i, line) in out.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let issue: Value = serde_json::from_str(line)
            .with_context(|| format!("bd export line {} is not JSON", i + 1))?;
        if let Some(id) = text(&issue, "id") {
            current.insert(
                id.to_owned(),
                Current {
                    closed: text(&issue, "status") == Some("closed"),
                    assignee: text(&issue, "assignee").unwrap_or_default().to_owned(),
                    updated: text(&issue, "updated_at").and_then(|t| parse_instant(t).ok()),
                },
            );
        }
        events.extend(issue_events(&issue, config, bc, &source)?);
    }
    if let Some(since) = since {
        events.retain(|e| e.t >= since);
    }
    // the export is current state. Creations and comments are history (each
    // is kept with its time); claims and closes are not: a claim released or
    // a goal reopened since leaves no trace, and 232 of one tracker's goals
    // were closed more than once. So the export vouches for them only as the
    // state at the capture instant, and only when it is whole (no `since`)
    let now = config.capture()?;
    let mut covers = vec!["create", "other:comment"];
    if bc.verdicts.is_some() {
        covers.push("verdict");
    }
    events.push(crate::event::coverage(
        since.unwrap_or(crate::event::ALL_HISTORY),
        Some(now),
        &covers,
        source.clone(),
        None,
    ));
    if since.is_none() {
        events.push(crate::event::coverage(
            now,
            Some(now),
            &STATE,
            source.clone(),
            None,
        ));
    }
    if bc.events
        && let Some(rows) = event_rows(&dir)
    {
        with_history(&mut events, &rows, &current, bc, &source, since, now)?;
    }
    Ok(events)
}

/// An issue as the export holds it now.
#[derive(Debug, Clone, Default)]
pub struct Current {
    pub closed: bool,
    pub assignee: String,
    pub updated: Option<Timestamp>,
}

/// Replace the export's claims and closes with bd's events: history, where
/// bd can serve it. The export's creations stay, with the assignee each
/// issue had then.
fn with_history(
    events: &mut Vec<Event>,
    rows: &[Row],
    current: &HashMap<String, Current>,
    bc: &BdConfig,
    source: &str,
    since: Option<Timestamp>,
    now: Timestamp,
) -> Result<()> {
    let clock = Clock::new(bc, rows, &anchors_of(events))?;
    let (mut history, at_creation) = history(rows, &clock, source, now);
    events.retain(|e| !matches!(e.kind, Kind::Claim | Kind::Close));
    for e in events.iter_mut().filter(|e| e.kind == Kind::Create) {
        // the export's assignee is today's; the events say who it was
        if let Some(a) = e.subject.as_ref().and_then(|id| at_creation.get(id)) {
            e.remove_attr(attr::ASSIGNEE);
            e.set_attr(attr::ASSIGNEE, a.clone());
        }
    }
    let created: HashMap<String, (Timestamp, String)> = events
        .iter()
        .filter(|e| e.kind == Kind::Create)
        .filter_map(|e| {
            let id = e.subject.clone()?;
            Some((
                id,
                (
                    e.t,
                    e.attr_str(attr::ASSIGNEE).unwrap_or_default().to_owned(),
                ),
            ))
        })
        .collect();
    let missed = reconcile(&history, current, &created, source, now);
    history.extend(missed);
    events.extend(
        history
            .into_iter()
            .filter(|e| e.kind == Kind::Coverage || since.is_none_or(|s| e.t >= s)),
    );
    events.push(crate::event::coverage(
        since.unwrap_or(crate::event::ALL_HISTORY),
        Some(now),
        HISTORY,
        format!("bd events ({source})"),
        None,
    ));
    Ok(())
}

/// The changes bd's events never recorded. Folded to now, each issue's
/// history must agree with the export's present state; where it does not,
/// a change went unrecorded (bd journals only its own write paths). It
/// happened after the issue's last recorded row and no later than its last
/// update: the export's state is put there, with a gap saying when is
/// unsure.
fn reconcile(
    history: &[Event],
    current: &HashMap<String, Current>,
    created: &HashMap<String, (Timestamp, String)>,
    source: &str,
    now: Timestamp,
) -> Vec<Event> {
    let mut by_issue: HashMap<&str, Vec<&Event>> = HashMap::new();
    for e in history.iter().filter(|e| e.kind != Kind::Coverage) {
        if let Some(id) = e.subject.as_deref() {
            by_issue.entry(id).or_default().push(e);
        }
    }
    let mut out = Vec::new();
    let mut ids: Vec<&String> = current.keys().collect();
    ids.sort();
    for id in ids {
        let now_state = &current[id];
        let (mut last, mut owner) = created
            .get(id)
            .cloned()
            .unwrap_or((crate::event::ALL_HISTORY, String::new()));
        let mut closed = false;
        let mut rows = by_issue.remove(id.as_str()).unwrap_or_default();
        rows.sort_by_key(|e| (e.t, rank(&e.kind)));
        for e in rows {
            last = last.max(e.t);
            match e.kind {
                Kind::Claim => {
                    e.attr_str(attr::ASSIGNEE)
                        .unwrap_or_default()
                        .clone_into(&mut owner);
                    closed = false;
                }
                Kind::Close => closed = true,
                Kind::Reopen => closed = false,
                Kind::Assign => e
                    .attr_str(attr::ASSIGNEE)
                    .unwrap_or_default()
                    .clone_into(&mut owner),
                _ => {}
            }
        }
        let mut kinds = Vec::new();
        if closed != now_state.closed {
            kinds.push(if now_state.closed {
                Kind::Close
            } else {
                Kind::Reopen
            });
        }
        if owner != now_state.assignee {
            kinds.push(Kind::Assign);
        }
        if kinds.is_empty() {
            continue;
        }
        let at = now_state
            .updated
            .filter(|u| *u >= last && *u <= now)
            .unwrap_or(now);
        let covers: Vec<&str> = kinds.iter().map(Kind::as_str).collect();
        let mut gap = crate::event::gap_over(
            last,
            at,
            &covers,
            format!("bd events ({source})"),
            None,
            format!(
                "bd events ({source}): {id} changed ({}) with no row recorded, after {last} and by {at}",
                covers.join(", ")
            ),
        );
        gap.set_attr(attr::TIMING, true);
        out.push(gap);
        for kind in kinds {
            let mut e = Event::new(at, kind.clone(), format!("bd events ({source})"));
            e.subject = Some(id.clone());
            e.ids.goal = Some(id.clone());
            if kind == Kind::Assign {
                e.set_attr(attr::ASSIGNEE, now_state.assignee.clone());
            }
            out.push(e);
        }
    }
    out
}

/// The tracker's state at the capture, as the export holds it.
const STATE: [&str; 5] = [
    "state:create",
    "state:claim",
    "state:close",
    "state:reopen",
    "state:assign",
];

/// What bd's events table is read for.
const HISTORY: &[&str] = &["claim", "close", "reopen", "assign"];

/// One row of bd's events table, with only what andon reads.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Row {
    pub issue_id: String,
    pub event_type: String,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub new_value: Option<String>,
    /// The time as bd stored it: true UTC, or local time labelled UTC.
    pub created_at: String,
    /// The assignee before the change, where the row keeps the issue as it
    /// was (`old_known`).
    #[serde(default)]
    pub old_assignee: Option<String>,
    #[serde(default)]
    pub old_known: Option<i64>,
}

/// The rows andon reads, from `bd sql`; `None` where bd cannot serve SQL
/// (an embedded database) or the read fails, and the export stands alone.
fn event_rows(dir: &Path) -> Option<Vec<Row>> {
    const QUERY: &str = "SELECT issue_id, event_type, actor, new_value, created_at, \
        CASE WHEN old_value LIKE '{%' THEN JSON_UNQUOTE(JSON_EXTRACT(old_value, '$.assignee')) END AS old_assignee, \
        (old_value LIKE '{%') AS old_known FROM events \
        WHERE event_type IN ('created', 'claimed', 'closed', 'reopened') \
        OR (event_type IN ('updated', 'status_changed') AND new_value LIKE '%\"assignee\"%')";
    let out = run_tool_in(dir, "bd", &["sql", "--json", QUERY], "install bd").ok()?;
    serde_json::from_str(&out).ok()
}

/// Each issue's own timestamps, which calibrate the rows: when it was
/// created, and when it was last closed.
fn anchors_of(events: &[Event]) -> HashMap<String, (Option<Timestamp>, Option<Timestamp>)> {
    let mut out: HashMap<String, (Option<Timestamp>, Option<Timestamp>)> = HashMap::new();
    for e in events {
        let Some(id) = e.subject.clone() else {
            continue;
        };
        let entry = out.entry(id).or_default();
        match e.kind {
            Kind::Create => entry.0 = Some(e.t),
            Kind::Close => entry.1 = Some(e.t),
            _ => {}
        }
    }
    out
}

/// How a row's stored time becomes an instant. bd once wrote local time
/// labelled UTC (in whatever zone the machine was in), and later true UTC,
/// so no single correction holds. A row whose issue timestamps pin it (a
/// creation, a last close) says the correction in force when it was
/// written; any other row takes the correction of the pinned rows around
/// it, or, with a configured cutover, local time before and UTC after.
pub struct Clock {
    zone: TimeZone,
    utc_from: Option<Timestamp>,
    /// Pinned rows, by stored time, with the correction each needed.
    pinned: Vec<(Timestamp, SignedDuration)>,
}

/// The largest correction a time zone can need.
const MAX_OFFSET: SignedDuration = SignedDuration::from_hours(14);

impl Clock {
    /// # Errors
    /// When the configured zone or cutover cannot be read.
    pub fn new(
        bc: &BdConfig,
        rows: &[Row],
        issues: &HashMap<String, (Option<Timestamp>, Option<Timestamp>)>,
    ) -> Result<Self> {
        let zone = match bc.events_zone.as_deref() {
            None => TimeZone::system(),
            Some(name) => TimeZone::get(name)
                .or_else(|_| TimeZone::posix(name))
                .with_context(|| format!("[adapters.bd] events_zone {name:?} is no time zone"))?,
        };
        let utc_from = bc
            .events_utc_from
            .as_deref()
            .map(parse_instant)
            .transpose()?;
        // the export keeps only an issue's last close: only its last closed
        // row is pinned by it
        let mut last_close: HashMap<&str, Timestamp> = HashMap::new();
        for row in rows.iter().filter(|r| r.event_type == "closed") {
            if let Ok(raw) = parse_instant(&row.created_at) {
                let last = last_close.entry(row.issue_id.as_str()).or_insert(raw);
                *last = (*last).max(raw);
            }
        }
        let mut pinned = Vec::new();
        for row in rows {
            let Ok(raw) = parse_instant(&row.created_at) else {
                continue;
            };
            let (created, closed) = issues.get(&row.issue_id).copied().unwrap_or_default();
            let own = match row.event_type.as_str() {
                "created" => created,
                "closed" if last_close.get(row.issue_id.as_str()) == Some(&raw) => closed,
                _ => None,
            };
            // a zone's offset is whole quarter hours, to the second: anything
            // else is an issue whose timestamps came from elsewhere (an import)
            if let Some(d) = own.map(|own| own.duration_since(raw))
                && d.abs() <= MAX_OFFSET
                && d.as_secs() % 900 == 0
                && d.subsec_nanos() == 0
            {
                pinned.push((raw, d));
            }
        }
        pinned.sort();
        Ok(Self {
            zone,
            utc_from,
            pinned,
        })
    }

    /// The instant a stored time means if it was local time in the zone.
    fn local(&self, raw: Timestamp) -> Option<Timestamp> {
        let civil = raw.to_zoned(TimeZone::UTC).datetime();
        civil
            .to_zoned(self.zone.clone())
            .ok()
            .map(|z| z.timestamp())
    }

    /// The instant a stored time means, or the span it lies in when the
    /// correction cannot be told. Between two pinned rows that needed
    /// different corrections, the change lies between their true instants:
    /// a row read with the earlier correction must fall before the later
    /// pin, and with the later one after the earlier pin. Where both
    /// readings fit, the row is unresolved.
    ///
    /// # Errors
    /// The span the instant lies in, when the correction cannot be told.
    pub fn resolve(&self, raw: Timestamp) -> Result<Timestamp, (Timestamp, Timestamp)> {
        if let Some(cutover) = self.utc_from {
            return if raw >= cutover {
                Ok(raw)
            } else {
                self.local(raw).ok_or((raw, raw + MAX_OFFSET))
            };
        }
        let next = self.pinned.partition_point(|(pin, _)| *pin <= raw);
        let before = next.checked_sub(1).map(|i| self.pinned[i]);
        let after = self.pinned.get(next).copied();
        let correction = match (before, after) {
            (Some((_, early)), Some((_, late))) if early == late => Some(early),
            (Some((_, only)), None) | (None, Some((_, only))) => Some(only),
            (Some((pin_a, early)), Some((pin_b, late))) => {
                let fits_early = raw + early < pin_b + late;
                let fits_late = raw + late > pin_a + early;
                match (fits_early, fits_late) {
                    (true, false) => Some(early),
                    (false, true) => Some(late),
                    _ => {
                        let (one, other) = (raw + early, raw + late);
                        return Err((one.min(other), one.max(other)));
                    }
                }
            }
            (None, None) => None,
        };
        correction
            .map(|d| raw + d)
            .ok_or((raw - MAX_OFFSET, raw + MAX_OFFSET))
    }
}

/// Claims, closes, reopens and assignments from bd's events, each at its
/// resolved instant; a row whose instant cannot be resolved is a gap over
/// the span it lies in. Also each issue's assignee at its creation, where a
/// row keeps the issue as it was before (its earliest such row).
pub fn history(
    rows: &[Row],
    clock: &Clock,
    source: &str,
    now: Timestamp,
) -> (Vec<Event>, HashMap<String, String>) {
    let source = format!("bd events ({source})");
    let mut out = Vec::new();
    let mut first_known: HashMap<String, (Timestamp, String)> = HashMap::new();
    for row in rows {
        let Ok(raw) = parse_instant(&row.created_at) else {
            out.push(crate::event::gap(
                now,
                HISTORY,
                source.clone(),
                None,
                format!("{source}: a row on {} has no time", row.issue_id),
            ));
            continue;
        };
        let kinds: Vec<Kind> = row_kinds(row);
        if kinds.is_empty() && row.old_known != Some(1) {
            continue;
        }
        // a row whose era cannot be told happened at one of two instants:
        // its facts are kept at the later, and a gap says only when is unsure
        let t = match clock.resolve(raw) {
            Ok(t) => t,
            Err((from, to)) => {
                if !kinds.is_empty() {
                    let covers: Vec<&str> = kinds.iter().map(Kind::as_str).collect();
                    let mut gap = crate::event::gap_over(
                        from,
                        to,
                        &covers,
                        source.clone(),
                        None,
                        format!(
                            "{source}: a {} row on {} stored at {raw} needs one of two time corrections, and no row near it says which",
                            row.event_type, row.issue_id
                        ),
                    );
                    gap.set_attr(attr::TIMING, true);
                    out.push(gap);
                }
                to
            }
        };
        if row.old_known == Some(1) {
            let before = row.old_assignee.clone().unwrap_or_default();
            let entry = first_known
                .entry(row.issue_id.clone())
                .or_insert((t, before.clone()));
            if t < entry.0 {
                *entry = (t, before);
            }
        }
        for kind in kinds {
            let mut e = Event::new(t, kind.clone(), source.clone());
            e.subject = Some(row.issue_id.clone());
            e.ids.goal = Some(row.issue_id.clone());
            let new: Value = row
                .new_value
                .as_deref()
                .filter(|v| v.starts_with('{'))
                .and_then(|v| serde_json::from_str(v).ok())
                .unwrap_or(Value::Null);
            match kind {
                Kind::Claim => {
                    let who = new["assignee"]
                        .as_str()
                        .filter(|a| !a.is_empty())
                        .map(str::to_owned)
                        .or_else(|| row.actor.clone());
                    e.actor.clone_from(&who);
                    e.set_attr(attr::ASSIGNEE, who.unwrap_or_default());
                }
                Kind::Close => {
                    e.actor.clone_from(&row.actor);
                    e.set_attr(
                        attr::CLOSE_REASON,
                        row.new_value.clone().unwrap_or_default(),
                    );
                }
                Kind::Assign => {
                    e.set_attr(
                        attr::ASSIGNEE,
                        new["assignee"].as_str().unwrap_or_default().to_owned(),
                    );
                }
                _ => e.actor.clone_from(&row.actor),
            }
            out.push(e);
        }
    }
    // rows written in one second (a claim, then a reassignment) keep the
    // order a tracker makes them in: a claim starts, an assignment follows
    out.sort_by_key(|e| (e.t, rank(&e.kind)));
    let at_creation = first_known
        .into_iter()
        .map(|(id, (_, a))| (id, a))
        .collect();
    (out, at_creation)
}

/// The order of facts written in one instant.
fn rank(kind: &Kind) -> u8 {
    match kind {
        Kind::Claim => 0,
        Kind::Assign => 1,
        Kind::Reopen => 2,
        Kind::Close => 3,
        _ => 4,
    }
}

/// What a row records, in andon's kinds.
fn row_kinds(row: &Row) -> Vec<Kind> {
    match row.event_type.as_str() {
        "claimed" => vec![Kind::Claim],
        "closed" => vec![Kind::Close],
        "reopened" => vec![Kind::Reopen],
        "updated" | "status_changed" => {
            let assigns = row
                .new_value
                .as_deref()
                .and_then(|v| serde_json::from_str::<Value>(v).ok())
                .is_some_and(|v| v.get("assignee").is_some());
            if assigns {
                vec![Kind::Assign]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// The events one exported issue yields. Pure; tested without bd.
///
/// # Errors
/// When a timestamp cannot be parsed.
pub fn issue_events(
    issue: &Value,
    config: &Config,
    bc: &BdConfig,
    source: &str,
) -> Result<Vec<Event>> {
    let mut out = Vec::new();
    let Some(id) = text(issue, "id") else {
        // an issue andon cannot name: its goal, claim and close are unknown
        out.push(crate::event::gap(
            config.capture()?,
            &[
                "create",
                "state:create",
                "state:claim",
                "state:close",
                "state:reopen",
                "state:assign",
            ],
            source.to_owned(),
            None,
            format!("{source}: an issue has no id"),
        ));
        return Ok(out);
    };
    let at = |key: &str| text(issue, key).map(parse_instant).transpose();
    let item = |t, kind| {
        let mut e = Event::new(t, kind, source.to_owned());
        e.subject = Some(id.to_owned());
        e.ids.goal = Some(id.to_owned());
        e
    };

    if let Some(t) = at("created_at")? {
        let mut e = item(t, Kind::Create);
        e.actor = text(issue, "created_by").map(str::to_owned);
        e.set_attr(attr::TITLE, text(issue, "title").unwrap_or_default());
        e.set_attr(
            attr::ITEM_TYPE,
            text(issue, "issue_type").unwrap_or_default(),
        );
        // an assignee owns the goal even before starting it (it is not a
        // claim: a claim is a start); the export says who it is now
        e.set_attr(attr::ASSIGNEE, text(issue, "assignee").unwrap_or_default());
        let description = goal::with_fields(
            text(issue, "description").unwrap_or_default(),
            issue,
            &config.goal.fields,
        );
        let sections = goal::sections(&description);
        if let Some(budget) = goal::section(&sections, &config.goal.budget) {
            e.set_attr(attr::BUDGET, budget.to_owned());
        }
        if let Some(pool) = goal::section(&sections, "pool") {
            e.set_attr(attr::POOL, pool.to_owned());
        }
        out.push(e);
    }

    // a claim is bd's start of work: in progress (or closed after starting),
    // with its owner; an assignee on an issue nobody started is not a claim
    let status = text(issue, "status").unwrap_or_default();
    let started = at("started_at")?;
    let claimed = status == "in_progress" || (status == "closed" && started.is_some());
    if claimed
        && let Some(assignee) = text(issue, "assignee")
        && let Some(t) = started.or(at("updated_at")?).or(at("created_at")?)
    {
        let mut e = item(t, Kind::Claim);
        e.actor = Some(assignee.to_owned());
        e.set_attr(attr::ASSIGNEE, assignee.to_owned());
        if started.is_none() {
            // no start recorded: the last update stands in, and moves
            e.set_attr(attr::CLAIM_TIME, "updated_at");
        }
        out.push(e);
    }

    if status == "closed"
        && let Some(t) = at("closed_at")?.or(at("updated_at")?)
    {
        let mut e = item(t, Kind::Close);
        e.set_attr(
            attr::CLOSE_REASON,
            text(issue, "close_reason").unwrap_or_default(),
        );
        out.push(e);
    }

    for comment in issue["comments"].as_array().into_iter().flatten() {
        out.push(comment_event(comment, id, bc, source, config.capture()?)?);
    }
    Ok(out)
}

/// A goal's description, for `check goal`; `None` when bd has no such issue.
///
/// # Errors
/// When bd cannot run.
pub fn description(config: &Config, bc: &BdConfig, id: &str) -> Result<Option<String>> {
    let dir = config.path(&bc.dir);
    let out = match run_tool_in(&dir, "bd", &["show", id, "--json"], "install bd") {
        Ok(out) => out,
        // bd says so when it has no such issue; any other failure is bd's
        Err(error) if error.to_string().contains("not found") => return Ok(None),
        Err(error) => return Err(error),
    };
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    let issue = v.as_array().and_then(|a| a.first()).unwrap_or(&v);
    Ok(issue["description"]
        .as_str()
        .map(|d| goal::with_fields(d, issue, &config.goal.fields)))
}

fn comment_event(
    comment: &Value,
    issue: &str,
    bc: &BdConfig,
    source: &str,
    now: jiff::Timestamp,
) -> Result<Event> {
    let body = text(comment, "text").unwrap_or_default();
    let verdict_like = bc
        .verdicts
        .as_ref()
        .is_some_and(|f| body.trim_start().starts_with(f.prefix.as_str()));
    let Some(t) = text(comment, "created_at") else {
        // a comment with no time cannot be placed: findings (and verdicts,
        // if it is one) are unknown, never quietly fewer
        let covers: &[&str] = if verdict_like {
            &["other:comment", "verdict"]
        } else {
            &["other:comment"]
        };
        return Ok(crate::event::gap(
            now,
            covers,
            source.to_owned(),
            None,
            format!("{source}: a comment on {issue} has no time"),
        ));
    };
    let mut e = Event::new(
        parse_instant(t)?,
        Kind::Other("other:comment".into()),
        source.to_owned(),
    );
    e.subject = Some(issue.to_owned());
    e.ids.goal = Some(issue.to_owned());
    e.actor = text(comment, "author").map(str::to_owned);
    let id = text(comment, "id").map(str::to_owned);
    let verdict = match bc.verdicts.as_ref() {
        Some(format) if verdict_like => {
            let rest = body.trim_start()[format.prefix.len()..].trim_start();
            match serde_json::Deserializer::from_str(rest)
                .into_iter::<Value>()
                .next()
            {
                Some(Ok(v)) => Some((format, v)),
                // a verdict andon cannot read is a gap in verdicts, never an
                // ordinary comment
                Some(Err(why)) => return unreadable_verdict(t, source, issue, &why.to_string()),
                None => return unreadable_verdict(t, source, issue, "no JSON after the prefix"),
            }
        }
        _ => None,
    };
    if let Some((format, v)) = verdict {
        e.kind = Kind::Verdict;
        // the comment's author gave the verdict; it is on the reviewed
        // change's seat, which its commit and change ids resolve to
        let reviewer = e.actor.take().unwrap_or_default();
        e.set_attr(attr::REVIEWER, reviewer);
        e.ids.verdict = id;
        let at = |path: &Option<String>| path.as_deref().and_then(|p| field(&v, p));
        e.ids.commit = at(&format.commit)
            .and_then(Value::as_str)
            .map(str::to_owned);
        e.ids.critic_session = at(&format.session)
            .and_then(Value::as_str)
            .map(str::to_owned);
        let changes: Vec<String> = match at(&format.change) {
            Some(Value::String(c)) => vec![c.clone()],
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|c| c.as_array().and_then(|c| c.first()).or(Some(c)))
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        e.ids.change = changes.first().cloned();
        e.set_attr(attr::CHANGES, changes);
        let said = field(&v, &format.outcome)
            .and_then(Value::as_str)
            .map(str::to_lowercase);
        e.outcome = Some(match said.as_deref() {
            Some(w) if ["go", "approve", "pass"].iter().any(|p| w.starts_with(p)) => Outcome::Go,
            Some(w)
                if ["return", "reject", "fail"]
                    .iter()
                    .any(|p| w.starts_with(p)) =>
            {
                Outcome::Return
            }
            _ => Outcome::Unknown,
        });
        return Ok(e);
    }
    e.ids.verdict = None;
    e.set_attr(attr::COMMENT, id.unwrap_or_default());
    Ok(e)
}

/// A comment that starts as a verdict but cannot be read as one.
/// Its time is known, so it stands only over spans that hold it: a comment
/// cannot be edited, and a gap at no time would hide every verdict forever.
fn unreadable_verdict(t: &str, source: &str, issue: &str, why: &str) -> Result<Event> {
    Ok(crate::event::gap_at(
        parse_instant(t)?,
        &["verdict"],
        source.to_owned(),
        None,
        format!("{source}: a verdict on {issue} is not JSON ({why})"),
    ))
}

/// A dotted path into a JSON object: `critic.session`.
fn field<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |v, key| v.get(key))
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// For tests and callers that already hold an export.
///
/// # Errors
/// When a line is not JSON.
pub fn from_export(export: &str, config: &Config, bc: &BdConfig, dir: &Path) -> Result<Vec<Event>> {
    let source = format!("bd export ({})", dir.display());
    let mut events = Vec::new();
    for line in export.lines().filter(|l| !l.trim().is_empty()) {
        let issue: Value = serde_json::from_str(line)?;
        events.extend(issue_events(&issue, config, bc, &source)?);
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bc() -> BdConfig {
        BdConfig {
            dir: ".".into(),
            verdicts: Some(crate::config::VerdictFormat {
                prefix: "Review-Verdict:".into(),
                outcome: "verdict".into(),
                commit: Some("candidate".into()),
                change: Some("changes".into()),
                session: Some("critic.session".into()),
            }),
            since: None,
            events: true,
            events_zone: None,
            events_utc_from: None,
        }
    }

    const EXPORT: &str = r###"{"id":"andon-xa4","title":"Instrument","description":"## Why\n\nx\n\n## Done when\n\n`cargo test`\n\n## Budget\n\nabout 8 agent-hours\n","status":"in_progress","assignee":"andon-1b","created_at":"2026-10-13T23:10:49Z","created_by":"andon","started_at":"2026-10-13T23:10:56Z","updated_at":"2026-10-13T23:10:56Z","comments":[{"id":"c1","issue_id":"andon-xa4","author":"andon-1a","text":"finding: the hub is andon-1a","created_at":"2026-10-13T23:20:00Z"},{"id":"c2","issue_id":"andon-xa4","author":"andon-1a","text":"Review-Verdict: {\"verdict\":\"RETURN\",\"candidate\":\"abc\",\"changes\":[[\"wnon\",\"x\"]],\"critic\":{\"session\":\"s9\"}} trailing prose","created_at":"2026-10-13T23:30:00Z"}]}
{"id":"andon-p1","title":"Assigned, not started","status":"open","assignee":"andon-1a","created_at":"2026-10-13T20:00:00Z"}
{"id":"andon-76c","title":"Census","status":"closed","created_at":"2026-10-13T20:00:00Z","closed_at":"2026-10-13T21:00:00Z","close_reason":"done"}"###;

    #[test]
    fn an_export_yields_create_claim_close_comment_and_verdict() {
        let config = Config {
            capture: Some(jiff::Timestamp::UNIX_EPOCH),
            ..Config::default()
        };
        let events =
            from_export(EXPORT, &config, &bc(), Path::new(".")).unwrap_or_else(|e| panic!("{e}"));
        let kinds: Vec<String> = events.iter().map(|e| e.kind.to_string()).collect();
        assert_eq!(
            kinds,
            // the open, assigned, unstarted issue yields a create and no claim
            vec![
                "create",
                "claim",
                "other:comment",
                "verdict",
                "create",
                "create",
                "close"
            ]
        );
        assert_eq!(
            events[0].attr_str(attr::BUDGET),
            Some("about 8 agent-hours")
        );
        assert_eq!(events[1].actor.as_deref(), Some("andon-1b"));
        let verdict = &events[3];
        assert_eq!(verdict.outcome, Some(Outcome::Return));
        assert_eq!(verdict.ids.verdict.as_deref(), Some("c2"));
        assert_eq!(verdict.ids.change.as_deref(), Some("wnon"));
        assert_eq!(verdict.ids.commit.as_deref(), Some("abc"));
        assert_eq!(verdict.actor, None);
        assert_eq!(verdict.attr_str(attr::REVIEWER), Some("andon-1a"));
        assert_eq!(events[6].attr_str(attr::CLOSE_REASON), Some("done"));
    }

    fn row(issue: &str, kind: &str, at: &str) -> Row {
        Row {
            issue_id: issue.into(),
            event_type: kind.into(),
            actor: None,
            new_value: None,
            created_at: at.into(),
            old_assignee: None,
            old_known: None,
        }
    }

    fn t(s: &str) -> Timestamp {
        parse_instant(s).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn each_row_takes_the_correction_of_the_pinned_rows_around_it() {
        // a pinned 7h behind (local time), b pinned true, c pinned 4h behind
        // (the machine moved zone), each by its creation
        let rows = vec![
            row("a", "created", "2026-10-01T03:00:00Z"),
            row("b", "created", "2026-10-05T10:00:00Z"),
            row("c", "created", "2026-10-08T06:00:00Z"),
        ];
        let issues: HashMap<String, (Option<Timestamp>, Option<Timestamp>)> = [
            ("a", "2026-10-01T10:00:00Z"),
            ("b", "2026-10-05T10:00:00Z"),
            ("c", "2026-10-08T10:00:00Z"),
        ]
        .into_iter()
        .map(|(id, c)| (id.to_owned(), (Some(t(c)), None)))
        .collect();
        let clock = Clock::new(&bc(), &rows, &issues).unwrap_or_else(|e| panic!("{e}"));
        // before the first pin, and between pins that agree
        assert_eq!(
            clock.resolve(t("2026-09-30T00:00:00Z")),
            Ok(t("2026-09-30T07:00:00Z"))
        );
        // between a 7h pin and a true one: read as local it falls before the
        // later pin, read as UTC it does not fall after the earlier one
        assert_eq!(
            clock.resolve(t("2026-10-01T04:00:00Z")),
            Ok(t("2026-10-01T11:00:00Z"))
        );
        // between those pins, a row that fits both readings is unresolved, and
        // one read as local that would fall after the true pin is UTC
        assert_eq!(
            clock.resolve(t("2026-10-03T00:00:00Z")),
            Err((t("2026-10-03T00:00:00Z"), t("2026-10-03T07:00:00Z")))
        );
        assert_eq!(
            clock.resolve(t("2026-10-05T09:00:00Z")),
            Ok(t("2026-10-05T09:00:00Z"))
        );
        // after the last pin, its correction
        assert_eq!(
            clock.resolve(t("2026-10-09T00:00:00Z")),
            Ok(t("2026-10-09T04:00:00Z"))
        );
    }

    #[test]
    fn only_an_issues_last_close_pins_its_row() {
        // closed at 11:00 and again at 12:00: the export's closed_at is the
        // second, and says nothing of the first
        let rows = vec![
            row("x", "closed", "2026-10-10T11:00:00Z"),
            row("x", "closed", "2026-10-10T12:00:00Z"),
        ];
        let issues: HashMap<String, (Option<Timestamp>, Option<Timestamp>)> =
            [("x".to_owned(), (None, Some(t("2026-10-10T12:00:00Z"))))].into();
        let clock = Clock::new(&bc(), &rows, &issues).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            clock.pinned,
            vec![(t("2026-10-10T12:00:00Z"), SignedDuration::ZERO)]
        );
    }
}
