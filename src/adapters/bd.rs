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
//! (`state:claim`, `state:close`), never as history; a kept ledger (`andon
//! ledger`, captured over time) is how a project keeps that history.

use std::path::Path;

use anyhow::{Context, Result};
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
    for (i, line) in out.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let issue: Value = serde_json::from_str(line)
            .with_context(|| format!("bd export line {} is not JSON", i + 1))?;
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
            &["state:create", "state:claim", "state:close"],
            source,
            None,
        ));
    }
    Ok(events)
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
            &["create", "state:create", "state:claim", "state:close"],
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
}
