//! tracker and review: GitHub. Reads issues, pull requests and Actions runs
//! through `gh` (GET requests only), or from a snapshot directory holding the
//! same JSON (`issues.json`, `prs.json`, `runs.json`, `events.json`).
//!
//! Emits:
//! - per issue: `create`, `claim` (an assignee; timed by its `assigned` event
//!   when the events page holds one, else the issue's creation), `close`, and
//!   each comment as `other:comment`; goal ids are `#<number>`;
//! - per pull request: `other:pr` (its author, branch, and the goals it
//!   closes) and each review as a `verdict`: approved is GO, changes
//!   requested is RETURN. Both carry `ids.change = "pr#<n>"`, the key the git
//!   adapter puts on the landing;
//! - per Actions run: `run`, by its head commit. GitHub keeps run logs, so a
//!   failure's reason is kept (its URL).

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use crate::adapters::run_tool;
use crate::config::{Config, GithubConfig};
use crate::event::attr;
use crate::event::{Event, Kind, Outcome};
use crate::time::parse_instant;

/// What the adapter reads, in `gh`'s JSON shapes.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub issues: Vec<Value>,
    pub prs: Vec<Value>,
    pub runs: Vec<Value>,
    /// Issue events (`assigned`, `unassigned`), newest first, as the REST API
    /// returns them. Optional: without them a claim is timed by the issue.
    pub events: Vec<Value>,
    /// Lists a snapshot directory does not hold, or holds unreadable, by
    /// file (`issues.json`, ...) and why: their kinds are a gap, never an
    /// empty list (Ferry's policy: one unreadable file, one gap).
    pub absent: Vec<(String, String)>,
}

const ISSUE_FIELDS: &str =
    "number,title,body,author,createdAt,closedAt,state,assignees,labels,comments";
const PR_FIELDS: &str = "number,title,author,createdAt,mergedAt,mergeCommit,headRefName,reviews,closingIssuesReferences,state";
const RUN_FIELDS: &str = "databaseId,headSha,headBranch,conclusion,status,createdAt,updatedAt,name,workflowName,event,url";

/// Read the repository's tracker, reviews and runs.
///
/// # Errors
/// When `gh` cannot run (or a snapshot file is unreadable).
pub fn collect(config: &Config, gc: &GithubConfig) -> Result<Vec<Event>> {
    let snapshot = match &gc.snapshot {
        Some(dir) => read_snapshot(&config.path(dir)),
        None => fetch(gc)?,
    };
    events(
        &snapshot,
        gc,
        &format!("github {}", gc.repo),
        config.capture()?,
        &config.goal.fields,
    )
}

fn fetch(gc: &GithubConfig) -> Result<Snapshot> {
    let fix = "install gh and run `gh auth login`, or set [adapters.github] snapshot";
    let limit = gc.limit.to_string();
    let list = |what: &str, fields: &str| -> Result<Vec<Value>> {
        let mut args = vec![
            what,
            "list",
            "-R",
            gc.repo.as_str(),
            "--limit",
            limit.as_str(),
            "--json",
            fields,
        ];
        if what != "run" {
            args.extend(["--state", "all"]);
        }
        values(&run_tool("gh", &args, fix)?)
    };
    let issues = list("issue", ISSUE_FIELDS)?;
    let prs = list("pr", PR_FIELDS)?;
    let runs = if gc.runs {
        list("run", RUN_FIELDS)?
    } else {
        Vec::new()
    };
    let request = events_request(&gc.repo);
    let args: Vec<&str> = request.iter().map(String::as_str).collect();
    let events = values(&run_tool("gh", &args, fix)?)?;
    Ok(Snapshot {
        issues,
        prs,
        runs,
        events,
        absent: Vec::new(),
    })
}

/// The issue events, every page: a claim whose `assigned` event is on a
/// later page would otherwise be timed by the issue's creation.
fn events_request(repo: &str) -> Vec<String> {
    ["api", "-X", "GET", "--paginate"]
        .into_iter()
        .map(str::to_owned)
        .chain([format!("repos/{repo}/issues/events?per_page=100")])
        .collect()
}

/// Read a snapshot directory. A list that is missing, unreadable or not
/// JSON is recorded as absent, with why (its kinds become a gap);
/// `events.json` is optional.
#[must_use]
pub fn read_snapshot(dir: &Path) -> Snapshot {
    let mut absent = Vec::new();
    let mut read = |name: &str| -> Vec<Value> {
        let path = dir.join(name);
        let got = std::fs::read_to_string(&path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "the snapshot holds none".to_owned()
                } else {
                    e.to_string()
                }
            })
            .and_then(|text| values(&text).map_err(|e| format!("not JSON ({e})")));
        got.unwrap_or_else(|why| {
            if name != "events.json" || !why.starts_with("the snapshot holds none") {
                absent.push((name.to_owned(), why));
            }
            Vec::new()
        })
    };
    let issues = read("issues.json");
    let prs = read("prs.json");
    let runs = read("runs.json");
    let events = read("events.json");
    Snapshot {
        issues,
        prs,
        runs,
        events,
        absent,
    }
}

/// One or more JSON arrays (gh pages print one after another), flattened.
fn values(text: &str) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for value in serde_json::Deserializer::from_str(text).into_iter::<Value>() {
        match value? {
            Value::Array(items) => out.extend(items),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Turn a snapshot into events. Pure; tested from fixtures.
///
/// # Errors
/// When a timestamp cannot be parsed.
pub fn events(
    s: &Snapshot,
    gc: &GithubConfig,
    source: &str,
    now: jiff::Timestamp,
    fields: &std::collections::BTreeMap<String, crate::config::GoalField>,
) -> Result<Vec<Event>> {
    let who = |login: Option<&str>| -> Option<String> {
        login.map(|l| gc.accounts.get(l).cloned().unwrap_or_else(|| l.to_owned()))
    };
    // the latest `assigned` event per (issue, assignee): events are newest first
    let mut assigned: HashMap<(i64, String), String> = HashMap::new();
    for e in &s.events {
        if e["event"] == "assigned"
            && let (Some(n), Some(login), Some(t)) = (
                e["issue"]["number"].as_i64(),
                e["assignee"]["login"].as_str(),
                e["created_at"].as_str(),
            )
        {
            assigned
                .entry((n, login.to_owned()))
                .or_insert_with(|| t.to_owned());
        }
    }
    let mut out = Vec::new();
    for issue in &s.issues {
        out.extend(issue_events(
            issue, gc, &assigned, &who, source, fields, now,
        )?);
    }
    for pr in &s.prs {
        out.extend(pr_events(pr, &who, source, now)?);
    }
    for run in &s.runs {
        out.push(run_event(run, gc, source, now)?);
    }
    // each list is complete unless it came back full (`limit` items): then
    // it covers only from its oldest item
    let from = |items: &[Value], key: &str| -> Result<jiff::Timestamp> {
        if items.len() < gc.limit {
            return Ok(crate::event::ALL_HISTORY);
        }
        let oldest = items.iter().filter_map(|i| text(i, key)).min();
        oldest.map_or(Ok(crate::event::ALL_HISTORY), parse_instant)
    };
    // (snapshot file, its items, the kinds it holds, its name, whether read)
    let lists = [
        (
            "issues.json",
            &s.issues,
            &[
                "create",
                "claim",
                "close",
                "reopen",
                "assign",
                "other:comment",
            ][..],
            "issues",
            true,
        ),
        (
            "prs.json",
            &s.prs,
            &["verdict", "other:pr"][..],
            "pull requests",
            true,
        ),
        ("runs.json", &s.runs, &["run"][..], "runs", gc.runs),
    ];
    // the issue events time claims: unreadable ones leave claims untimed
    if let Some((file, why)) = s.absent.iter().find(|(a, _)| a == "events.json") {
        out.push(crate::event::gap(
            now,
            &["claim"],
            format!("{source} issue events"),
            None,
            format!("{source}: {file}: {why}"),
        ));
    }
    for (file, items, kinds, what, read) in lists {
        if !read {
            continue;
        }
        if let Some((_, why)) = s.absent.iter().find(|(a, _)| a == file) {
            out.push(crate::event::gap(
                now,
                kinds,
                format!("{source} {what}"),
                None,
                format!("{source}: {file}: {why}"),
            ));
        } else {
            out.push(crate::event::coverage(
                from(items, "createdAt")?,
                Some(now),
                kinds,
                format!("{source} {what}"),
                None,
            ));
        }
    }
    Ok(out)
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// What an issue yields.
const ISSUE: &[&str] = &[
    "create",
    "claim",
    "close",
    "reopen",
    "assign",
    "other:comment",
];

fn issue_events(
    issue: &Value,
    gc: &GithubConfig,
    assigned: &HashMap<(i64, String), String>,
    who: &dyn Fn(Option<&str>) -> Option<String>,
    source: &str,
    fields: &std::collections::BTreeMap<String, crate::config::GoalField>,
    now: jiff::Timestamp,
) -> Result<Vec<Event>> {
    let mut out = Vec::new();
    let Some(n) = issue["number"].as_i64() else {
        out.push(unplaced(now, ISSUE, source, "an issue has no number"));
        return Ok(out);
    };
    if let Some(label) = &gc.goal_label {
        let labels: Vec<&str> = issue["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l["name"].as_str())
            .collect();
        if !labels.contains(&label.as_str()) {
            return Ok(out);
        }
    }
    let id = format!("#{n}");
    let item = |t: &str, kind: Kind| -> Result<Event> {
        let mut e = Event::new(parse_instant(t)?, kind, format!("{source} issue {id}"));
        e.subject = Some(id.clone());
        e.ids.goal = Some(id.clone());
        Ok(e)
    };
    let Some(created) = text(issue, "createdAt") else {
        out.push(unplaced(
            now,
            ISSUE,
            source,
            &format!("issue {id} has no creation time"),
        ));
        return Ok(out);
    };
    let mut create = item(created, Kind::Create)?;
    create.actor = who(issue["author"]["login"].as_str());
    create.set_attr(
        attr::TITLE,
        text(issue, "title").unwrap_or_default().to_owned(),
    );
    let body = crate::goal::with_fields(text(issue, "body").unwrap_or_default(), issue, fields);
    let sections = crate::goal::sections(&body);
    if let Some(budget) = crate::goal::section(&sections, "budget") {
        create.set_attr(attr::BUDGET, budget.to_owned());
    }
    if let Some(pool) = crate::goal::section(&sections, "pool") {
        create.set_attr(attr::POOL, pool.to_owned());
    }
    out.push(create);
    let closed = text(issue, "state").is_some_and(|s| s.eq_ignore_ascii_case("closed"));
    for a in issue["assignees"].as_array().into_iter().flatten().take(1) {
        let Some(login) = a["login"].as_str() else {
            out.push(unplaced(
                now,
                &["claim"],
                source,
                &format!("an assignee of issue {id} has no login"),
            ));
            continue;
        };
        let t = assigned
            .get(&(n, login.to_owned()))
            .map_or(created, String::as_str);
        let mut claim = item(t, Kind::Claim)?;
        claim.actor = who(Some(login));
        claim.set_attr(attr::ASSIGNEE, login.to_owned());
        out.push(claim);
    }
    if closed {
        match text(issue, "closedAt") {
            Some(t) => out.push(item(t, Kind::Close)?),
            None => out.push(unplaced(
                now,
                &["close"],
                source,
                &format!("closed issue {id} has no close time"),
            )),
        }
    }
    for c in issue["comments"].as_array().into_iter().flatten() {
        let Some(t) = text(c, "createdAt") else {
            out.push(unplaced(
                now,
                &["other:comment"],
                source,
                &format!("a comment on issue {id} has no time"),
            ));
            continue;
        };
        let mut e = item(t, Kind::Other("other:comment".into()))?;
        e.actor = who(c["author"]["login"].as_str());
        e.set_attr(attr::COMMENT, text(c, "id").unwrap_or_default().to_owned());
        out.push(e);
    }
    Ok(out)
}

fn pr_events(
    pr: &Value,
    who: &dyn Fn(Option<&str>) -> Option<String>,
    source: &str,
    now: jiff::Timestamp,
) -> Result<Vec<Event>> {
    let mut out = Vec::new();
    let (Some(n), Some(created)) = (pr["number"].as_i64(), text(pr, "createdAt")) else {
        out.push(unplaced(
            now,
            &["verdict", "other:pr"],
            source,
            "a pull request has no number or creation time",
        ));
        return Ok(out);
    };
    let change = format!("pr#{n}");
    let goals: Vec<String> = pr["closingIssuesReferences"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["number"].as_i64())
        .map(|i| format!("#{i}"))
        .collect();
    let author = who(pr["author"]["login"].as_str());
    let mut opened = Event::new(
        parse_instant(created)?,
        Kind::Other("other:pr".into()),
        format!("{source} pr {n}"),
    );
    opened.actor.clone_from(&author);
    opened.ids.change = Some(change.clone());
    opened.ids.commit = pr["mergeCommit"]["oid"].as_str().map(str::to_owned);
    opened.subject = goals.first().cloned();
    opened.ids.goal.clone_from(&opened.subject);
    opened.set_attr(attr::GOALS, goals.clone());
    opened.set_attr(
        attr::BRANCH,
        text(pr, "headRefName").unwrap_or_default().to_owned(),
    );
    opened.set_attr(
        attr::TITLE,
        text(pr, "title").unwrap_or_default().to_owned(),
    );
    opened.set_attr(
        attr::MERGED_AT,
        text(pr, "mergedAt").unwrap_or_default().to_owned(),
    );
    out.push(opened);
    for r in pr["reviews"].as_array().into_iter().flatten() {
        // a review not yet submitted (pending) has no time and no verdict
        let Some(t) = text(r, "submittedAt") else {
            continue;
        };
        let outcome = match text(r, "state").unwrap_or_default() {
            "APPROVED" => Outcome::Go,
            "CHANGES_REQUESTED" => Outcome::Return,
            // a comment-only review decides nothing
            _ => continue,
        };
        let mut v = Event::new(
            parse_instant(t)?,
            Kind::Verdict,
            format!("{source} pr {n} review"),
        );
        v.outcome = Some(outcome);
        v.ids.change = Some(change.clone());
        v.ids.verdict = text(r, "id").map(str::to_owned);
        v.ids.commit = r["commit"]["oid"].as_str().map(str::to_owned);
        v.subject = goals.first().cloned();
        // the verdict is on the author's change; the reviewer is who gave it
        v.actor.clone_from(&author);
        v.set_attr(
            attr::REVIEWER,
            who(r["author"]["login"].as_str()).unwrap_or_default(),
        );
        // a review joins its landing by the pull request, not by a trailer
        v.set_attr(attr::JOIN, "change");
        out.push(v);
    }
    Ok(out)
}

/// An item the adapter cannot place in time: a gap in its kinds, never a
/// quietly shorter list.
fn unplaced(now: jiff::Timestamp, kinds: &[&str], source: &str, what: &str) -> Event {
    crate::event::gap(
        now,
        kinds,
        source.to_owned(),
        None,
        format!("{source}: {what}"),
    )
}

fn run_event(run: &Value, gc: &GithubConfig, source: &str, now: jiff::Timestamp) -> Result<Event> {
    let Some(t) = text(run, "createdAt") else {
        return Ok(unplaced(
            now,
            &["run"],
            source,
            "a run has no creation time",
        ));
    };
    let mut e = Event::new(parse_instant(t)?, Kind::Run, format!("{source} run"));
    if text(run, "status") == Some("completed")
        && let Some(end) = text(run, "updatedAt")
    {
        e.t_end = Some(parse_instant(end)?);
    }
    e.ids.commit = text(run, "headSha").map(str::to_owned);
    e.ids.receipt = run["databaseId"].as_i64().map(|i| i.to_string());
    e.outcome = Some(match text(run, "conclusion").unwrap_or_default() {
        "success" => Outcome::Pass,
        "failure" | "timed_out" | "startup_failure" => Outcome::Fail,
        "cancelled" => Outcome::Stopped,
        _ => Outcome::Unknown,
    });
    let branch = text(run, "headBranch").unwrap_or_default();
    e.actor = gc
        .branch_seats
        .iter()
        .filter(|(prefix, _)| branch.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, seat)| seat.clone());
    e.set_attr(
        attr::RECIPE,
        text(run, "workflowName")
            .or_else(|| text(run, "name"))
            .unwrap_or_default()
            .to_owned(),
    );
    e.set_attr(attr::MACHINE, "github-actions");
    e.set_attr(attr::BRANCH, branch.to_owned());
    if e.outcome == Some(Outcome::Fail) {
        // GitHub keeps the run's logs: the reason is kept, at this address
        e.set_attr(
            attr::LOGS,
            text(run, "url").unwrap_or("logs kept by GitHub").to_owned(),
        );
    }
    Ok(e)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, reason = "a fixture captured now")]
mod tests {
    use super::*;

    fn gc() -> GithubConfig {
        GithubConfig {
            repo: "octo/demo".into(),
            goal_label: None,
            accounts: std::iter::once(("octo-bot".to_owned(), "bot-1".to_owned())).collect(),
            branch_seats: std::iter::once(("bot-1/".to_owned(), "bot-1".to_owned())).collect(),
            since: None,
            snapshot: None,
            limit: 100,
            runs: true,
        }
    }

    fn snapshot() -> Snapshot {
        let v = |s: &str| values(s).unwrap_or_else(|e| panic!("{e}"));
        Snapshot {
            issues: v(
                r###"[{"number":3,"title":"Board shows seats","body":"## Why\nx\n## Budget\n4 agent-hours","author":{"login":"lead"},"createdAt":"2026-10-12T00:00:00Z","state":"CLOSED","closedAt":"2026-10-13T13:00:00Z","assignees":[{"login":"octo-bot"}],"labels":[],"comments":[{"id":"c1","author":{"login":"lead"},"body":"finding","createdAt":"2026-10-12T05:00:00Z"}]}]"###,
            ),
            prs: v(
                r#"[{"number":7,"title":"Fix","author":{"login":"octo-bot"},"createdAt":"2026-10-13T09:30:00Z","mergedAt":"2026-10-13T12:00:00Z","mergeCommit":{"oid":"m"},"headRefName":"bot-1/fix","state":"MERGED","closingIssuesReferences":[{"number":3}],"reviews":[{"id":"r1","author":{"login":"lead"},"state":"CHANGES_REQUESTED","submittedAt":"2026-10-13T10:00:00Z","commit":{"oid":"b1"}},{"id":"r2","author":{"login":"lead"},"state":"COMMENTED","submittedAt":"2026-10-13T10:30:00Z"},{"id":"r3","author":{"login":"lead"},"state":"APPROVED","submittedAt":"2026-10-13T11:30:00Z","commit":{"oid":"b2"}}]}]"#,
            ),
            runs: v(
                r#"[{"databaseId":99,"headSha":"b2","headBranch":"bot-1/fix","conclusion":"failure","status":"completed","createdAt":"2026-10-13T11:00:00Z","updatedAt":"2026-10-13T11:05:00Z","workflowName":"CI","url":"https://github.com/octo/demo/actions/runs/99"}]"#,
            ),
            events: v(
                r#"[{"event":"assigned","issue":{"number":3},"assignee":{"login":"octo-bot"},"created_at":"2026-10-13T09:00:00Z"}]"#,
            ),
            absent: Vec::new(),
        }
    }

    #[test]
    fn issues_become_goals_claimed_by_their_assignee() {
        let events = events(
            &snapshot(),
            &gc(),
            "github",
            jiff::Timestamp::now(),
            &std::collections::BTreeMap::new(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        // and each list states its coverage
        let covered = events.iter().filter(|e| e.kind == Kind::Coverage).count();
        assert_eq!(covered, 3);
        let kinds: Vec<String> = events
            .iter()
            .filter(|e| e.kind != Kind::Coverage)
            .map(|e| e.kind.to_string())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "create",
                "claim",
                "close",
                "other:comment",
                "other:pr",
                "verdict",
                "verdict",
                "run"
            ]
        );
        let claim = &events[1];
        assert_eq!(claim.subject.as_deref(), Some("#3"));
        assert_eq!(claim.actor.as_deref(), Some("bot-1"));
        assert_eq!(
            claim.t,
            parse_instant("2026-10-13T09:00:00Z").unwrap_or_else(|e| panic!("{e}"))
        );
        assert_eq!(events[0].attr_str(attr::BUDGET), Some("4 agent-hours"));
    }

    #[test]
    fn reviews_are_verdicts_on_the_pull_request() {
        let events = events(
            &snapshot(),
            &gc(),
            "github",
            jiff::Timestamp::now(),
            &std::collections::BTreeMap::new(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let verdicts: Vec<&Event> = events.iter().filter(|e| e.kind == Kind::Verdict).collect();
        assert_eq!(verdicts.len(), 2, "a comment-only review is not a verdict");
        assert_eq!(verdicts[0].outcome, Some(Outcome::Return));
        assert_eq!(verdicts[0].ids.change.as_deref(), Some("pr#7"));
        assert_eq!(verdicts[0].actor.as_deref(), Some("bot-1"));
        assert_eq!(verdicts[0].attr_str(attr::REVIEWER), Some("lead"));
        assert_eq!(verdicts[1].outcome, Some(Outcome::Go));
    }

    #[test]
    fn a_failed_run_keeps_its_reason_and_finds_its_seat_by_branch() {
        let events = events(
            &snapshot(),
            &gc(),
            "github",
            jiff::Timestamp::now(),
            &std::collections::BTreeMap::new(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let run = events
            .iter()
            .find(|e| e.kind == Kind::Run)
            .unwrap_or_else(|| panic!("a run"));
        assert_eq!(run.outcome, Some(Outcome::Fail));
        assert_eq!(run.actor.as_deref(), Some("bot-1"));
        assert!(
            run.attr_str(attr::LOGS)
                .is_some_and(|r| r.ends_with("/runs/99"))
        );
    }

    #[test]
    fn paged_output_flattens() {
        assert_eq!(
            values("[1,2][3]").unwrap_or_else(|e| panic!("{e}")).len(),
            3
        );
    }

    #[test]
    fn issue_events_are_read_to_the_last_page() {
        assert!(events_request("o/r").iter().any(|a| a == "--paginate"));
    }
}
