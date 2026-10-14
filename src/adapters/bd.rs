//! tracker: bd. Reads `bd export` (issues with their comments), which bd
//! prints without writing anything (founding design §5).
//!
//! Emits, per issue: `create` at its creation (by its creator, with the goal's
//! title and appetite), `claim` at its start (by its assignee), `close` at its
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
    let now = config.capture();
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
        e.set_attr("title", text(issue, "title").unwrap_or_default());
        e.set_attr("item_type", text(issue, "issue_type").unwrap_or_default());
        // an assignee owns the goal even before starting it (it is not a
        // claim: a claim is a start); the export says who it is now
        e.set_attr("assignee", text(issue, "assignee").unwrap_or_default());
        let description = text(issue, "description").unwrap_or_default();
        let sections = goal::sections(description);
        if let Some(appetite) = goal::section(&sections, &config.goal.appetite) {
            e.set_attr("appetite", appetite.to_owned());
        }
        if let Some(pool) = goal::section(&sections, "pool") {
            e.set_attr("pool", pool.to_owned());
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
        e.set_attr("assignee", assignee.to_owned());
        if started.is_none() {
            // no start recorded: the last update stands in, and moves
            e.set_attr("claim_time", "updated_at");
        }
        out.push(e);
    }

    if status == "closed"
        && let Some(t) = at("closed_at")?.or(at("updated_at")?)
    {
        let mut e = item(t, Kind::Close);
        e.set_attr("reason", text(issue, "close_reason").unwrap_or_default());
        out.push(e);
    }

    for comment in issue["comments"].as_array().into_iter().flatten() {
        if let Some(e) = comment_event(comment, id, bc, source)? {
            out.push(e);
        }
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
    Ok(issue["description"].as_str().map(str::to_owned))
}

fn comment_event(
    comment: &Value,
    issue: &str,
    bc: &BdConfig,
    source: &str,
) -> Result<Option<Event>> {
    let Some(t) = text(comment, "created_at") else {
        return Ok(None);
    };
    let body = text(comment, "text").unwrap_or_default();
    let mut e = Event::new(
        parse_instant(t)?,
        Kind::Other("other:comment".into()),
        source.to_owned(),
    );
    e.subject = Some(issue.to_owned());
    e.ids.goal = Some(issue.to_owned());
    e.actor = text(comment, "author").map(str::to_owned);
    let id = text(comment, "id").map(str::to_owned);
    let verdict = bc.verdicts.as_ref().and_then(|format| {
        let rest = body.trim_start().strip_prefix(format.prefix.as_str())?;
        let v = serde_json::Deserializer::from_str(rest.trim_start())
            .into_iter::<Value>()
            .next()?
            .ok()?;
        Some((format, v))
    });
    if let Some((format, v)) = verdict {
        e.kind = Kind::Verdict;
        // the comment's author gave the verdict; it is on the reviewed
        // change's seat, which its commit and change ids resolve to
        let reviewer = e.actor.take().unwrap_or_default();
        e.set_attr("reviewer", reviewer);
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
        e.set_attr("changes", changes);
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
        return Ok(Some(e));
    }
    e.ids.verdict = None;
    e.set_attr("comment", id.unwrap_or_default());
    Ok(Some(e))
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

    const EXPORT: &str = r###"{"id":"andon-xa4","title":"Instrument","description":"## Why\n\nx\n\n## Done when\n\n`cargo test`\n\n## Appetite\n\nabout 8 agent-hours\n","status":"in_progress","assignee":"andon-1b","created_at":"2026-10-13T23:10:49Z","created_by":"andon","started_at":"2026-10-13T23:10:56Z","updated_at":"2026-10-13T23:10:56Z","comments":[{"id":"c1","issue_id":"andon-xa4","author":"andon-1a","text":"finding: the hub is andon-1a","created_at":"2026-10-13T23:20:00Z"},{"id":"c2","issue_id":"andon-xa4","author":"andon-1a","text":"Review-Verdict: {\"verdict\":\"RETURN\",\"candidate\":\"abc\",\"changes\":[[\"wnon\",\"x\"]],\"critic\":{\"session\":\"s9\"}} trailing prose","created_at":"2026-10-13T23:30:00Z"}]}
{"id":"andon-p1","title":"Assigned, not started","status":"open","assignee":"andon-1a","created_at":"2026-10-13T20:00:00Z"}
{"id":"andon-76c","title":"Census","status":"closed","created_at":"2026-10-13T20:00:00Z","closed_at":"2026-10-13T21:00:00Z","close_reason":"done"}"###;

    #[test]
    fn an_export_yields_create_claim_close_comment_and_verdict() {
        let events = from_export(EXPORT, &Config::default(), &bc(), Path::new("."))
            .unwrap_or_else(|e| panic!("{e}"));
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
        assert_eq!(events[0].attr_str("appetite"), Some("about 8 agent-hours"));
        assert_eq!(events[1].actor.as_deref(), Some("andon-1b"));
        let verdict = &events[3];
        assert_eq!(verdict.outcome, Some(Outcome::Return));
        assert_eq!(verdict.ids.verdict.as_deref(), Some("c2"));
        assert_eq!(verdict.ids.change.as_deref(), Some("wnon"));
        assert_eq!(verdict.ids.commit.as_deref(), Some("abc"));
        assert_eq!(verdict.actor, None);
        assert_eq!(verdict.attr_str("reviewer"), Some("andon-1a"));
        assert_eq!(events[6].attr_str("reason"), Some("done"));
    }
}
