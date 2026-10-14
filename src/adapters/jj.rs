//! vcs: jj. Reads the trunk's first-parent history, unlanded commits, and
//! the operation log (founding design §5).
//!
//! Every jj call runs with `--ignore-working-copy --at-op @`: no snapshot of
//! any workspace, and no merge of divergent operation heads, so reading
//! records no operation. andon writes nothing into the repository.
//!
//! Emits:
//! - `land` for each trunk commit, timed by the operation that first pointed
//!   the trunk at it or a descendant (a batch lands at one instant), else by
//!   its committer time; the actor is the seat trailer, else the workspace
//!   trailer, else the operation's workspace;
//! - `commit` for each commit, timed by author time (the change's start);
//! - `other:desk_create` and `other:desk_remove` for workspace operations.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use jiff::{SignedDuration, Timestamp};
use regex::Regex;
use serde::Deserialize;

use crate::adapters::run_tool;
use crate::config::{Config, JjConfig, Trailers};
use crate::event::{Event, Kind, Outcome};
use crate::time::parse_instant;

#[derive(Debug, Deserialize)]
pub struct JjCommit {
    pub commit_id: String,
    pub change_id: String,
    pub description: String,
    pub author: Signature,
    pub committer: Signature,
}

#[derive(Debug, Deserialize)]
pub struct Signature {
    pub timestamp: String,
}

#[derive(Debug, Deserialize)]
pub struct JjOp {
    pub time: OpTime,
    pub description: String,
    #[serde(default)]
    pub workspace_name: Option<String>,
    #[serde(default)]
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct OpTime {
    pub start: String,
}

/// Read the repository.
///
/// # Errors
/// When jj cannot run or its output cannot be parsed.
pub fn collect(config: &Config, jc: &JjConfig) -> Result<Vec<Event>> {
    let repo = config.path(&jc.repo);
    let op = operation(&repo)?;
    let dir = Repo {
        dir: &repo,
        op: &op,
    };
    // the filter is a UTC instant before `since`, never jj's reading of a
    // date in local time; the events keep only what lies after `since`
    let date = jc
        .since
        .as_deref()
        .map(parse_instant)
        .transpose()?
        .map(|s| format!(" & committer_date(after:{:?})", crate::time::read_from(s)))
        .unwrap_or_default();
    // the root commit is jj's empty origin, not a landing
    let landed = log(
        &dir,
        &format!("first_ancestors({}) ~ root(){date}", jc.trunk),
    )?;
    let unlanded = log(
        &dir,
        &format!("~::{} & ~empty() & mutable(){date}", jc.trunk),
    )?;
    let since_t = jc.since.as_deref().map(parse_instant).transpose()?;
    let ops = op_log(&dir, since_t)?;
    let goal = config.goal_regex()?;
    let source = format!("jj -R {}", repo.display());
    let since = jc.since.as_deref().map(parse_instant).transpose()?;
    let mut out = events(
        &landed,
        &unlanded,
        &ops,
        &jc.trunk,
        &jc.trailers,
        &goal,
        &source,
        since,
    )?;
    // what this read covers: the trunk's landings and change starts from
    // `since` (or all history) to now, the instant of capture
    out.push(crate::event::coverage(
        since.unwrap_or(crate::event::ALL_HISTORY),
        Some(config.capture()),
        &[
            "land",
            "commit",
            "freeze",
            "other:desk_create",
            "other:desk_remove",
        ],
        source,
        None,
    ));
    Ok(out)
}

/// One commit, for `check commit`: its id, title and the seat its trailers
/// name (the seat trailer, else the workspace trailer).
///
/// # Errors
/// When jj cannot resolve the revision to exactly one commit.
pub fn describe(config: &Config, jc: &JjConfig, rev: &str) -> Result<crate::check::Commit> {
    let repo = config.path(&jc.repo);
    let op = operation(&repo)?;
    let commits = log(
        &Repo {
            dir: &repo,
            op: &op,
        },
        rev,
    )?;
    let [commit] = commits.as_slice() else {
        anyhow::bail!("{rev} names {} commits; name exactly one", commits.len());
    };
    let parsed = Parsed::new(&commit.description, &jc.trailers, &config.goal_regex()?);
    Ok(crate::check::Commit {
        id: commit.commit_id.clone(),
        title: parsed.title,
        message: commit.description.clone(),
        seat: parsed.seat.or(parsed.workspace),
    })
}

/// A repository read at one operation, so every call sees one snapshot.
struct Repo<'a> {
    dir: &'a Path,
    op: &'a str,
}

const FIX: &str = "install jj, or remove [adapters.jj] from andon.toml";

fn jj(repo: &Repo<'_>, args: &[&str]) -> Result<String> {
    let dir = repo.dir.to_string_lossy();
    let mut full = vec![
        "--ignore-working-copy",
        "--at-op",
        repo.op,
        "--no-pager",
        "-R",
        &dir,
    ];
    full.extend_from_slice(args);
    run_tool("jj", &full, FIX)
}

/// The operation to read at: `@`, or when concurrent writers left several
/// heads (jj would merge them, recording an operation), the newest head.
fn operation(dir: &Path) -> Result<String> {
    let path = dir.to_string_lossy().into_owned();
    let read = |op: &str, args: &[&str]| -> Result<String> {
        let mut full = vec![
            "--ignore-working-copy",
            "--at-op",
            op,
            "--no-pager",
            "-R",
            path.as_str(),
        ];
        full.extend_from_slice(args);
        run_tool("jj", &full, FIX)
    };
    match read("@", &["op", "log", "-n1", "--no-graph", "-T", "id"]) {
        Ok(id) => Ok(id.trim().to_owned()),
        Err(error) if error.to_string().contains("more than one operation") => {
            let mut newest: Option<(Timestamp, String)> = None;
            for head in op_heads(dir)? {
                let json = read(
                    &head,
                    &["op", "log", "-n1", "--no-graph", "-T", "json(self)"],
                )?;
                let op: JjOp = serde_json::from_str(json.trim())
                    .context("jj op log printed something other than an operation")?;
                let t = parse_instant(&op.time.start)?;
                if newest.as_ref().is_none_or(|(n, _)| t > *n) {
                    newest = Some((t, head));
                }
            }
            newest.map(|(_, id)| id).ok_or_else(|| {
                anyhow::anyhow!("jj reports divergent operations but lists no heads")
            })
        }
        Err(error) => Err(error),
    }
}

/// The operation heads, from the repository's store (a secondary
/// workspace's `.jj/repo` is a file naming the main repository's).
fn op_heads(dir: &Path) -> Result<Vec<String>> {
    let root = dir
        .ancestors()
        .find(|d| d.join(".jj").exists())
        .ok_or_else(|| anyhow::anyhow!("{} is not in a jj repository", dir.display()))?;
    let mut store = root.join(".jj").join("repo");
    if store.is_file() {
        let target = std::fs::read_to_string(&store)?;
        let target = Path::new(target.trim());
        store = if target.is_absolute() {
            target.to_path_buf()
        } else {
            root.join(".jj").join(target)
        };
    }
    let mut heads: Vec<String> = std::fs::read_dir(store.join("op_heads").join("heads"))
        .with_context(|| format!("cannot list operation heads in {}", store.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    heads.sort();
    Ok(heads)
}

fn log(repo: &Repo<'_>, revset: &str) -> Result<Vec<JjCommit>> {
    let out = jj(
        repo,
        &[
            "log",
            "--no-graph",
            "-r",
            revset,
            "-T",
            "json(self) ++ \"\\n\"",
        ],
    )?;
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).context("jj log printed something other than a commit"))
        .collect()
}

/// The operation log, newest first. With `since`, reading stops two days
/// before it: older operations cannot time a landing in the window, and the
/// two days keep the trunk move before it, which the batch rule needs.
fn op_log(repo: &Repo<'_>, since: Option<Timestamp>) -> Result<Vec<JjOp>> {
    use std::io::BufRead;
    let dir = repo.dir.to_string_lossy();
    let mut child = std::process::Command::new("jj")
        .args([
            "--ignore-working-copy",
            "--at-op",
            repo.op,
            "--no-pager",
            "-R",
            &dir,
        ])
        .args(["op", "log", "--no-graph", "-T", "json(self) ++ \"\\n\""])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("cannot run jj ({e}); {FIX}"))?;
    let cutoff = since.and_then(|s| s.checked_sub(SignedDuration::from_hours(48)).ok());
    let mut ops = Vec::new();
    let mut stopped = false;
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let op: JjOp = serde_json::from_str(&line)
                .context("jj op log printed something other than an operation")?;
            let old = cutoff.is_some_and(|c| parse_instant(&op.time.start).is_ok_and(|t| t < c));
            if old {
                stopped = true;
                break;
            }
            ops.push(op);
        }
    }
    if stopped {
        // the rest is older than anything the window needs
        let _ = child.kill();
    }
    let status = child.wait()?;
    if !stopped && !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
        }
        let first = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no message");
        anyhow::bail!(
            "jj op log failed in {dir}: {}",
            first.trim_start_matches("Error: ")
        );
    }
    Ok(ops)
}

/// Turn jj's commits and operations into events. Pure; tested without jj.
///
/// `landed` is the trunk's first-parent line, newest first, as jj prints it.
///
/// # Errors
/// When a timestamp cannot be parsed.
#[allow(clippy::too_many_arguments)]
pub fn events(
    landed: &[JjCommit],
    unlanded: &[JjCommit],
    ops: &[JjOp],
    trunk: &str,
    trailers: &Trailers,
    goal: &Regex,
    source: &str,
    since: Option<Timestamp>,
) -> Result<Vec<Event>> {
    // the earliest operation pointing the trunk at each commit
    let pointer = format!("point bookmark {trunk} to commit ");
    let mut first_op: HashMap<&str, (Timestamp, Option<&str>)> = HashMap::new();
    let mut out = Vec::new();
    for op in ops {
        let t = parse_instant(&op.time.start)?;
        if let Some(commit) = op.description.strip_prefix(&pointer) {
            let entry = first_op
                .entry(commit.trim())
                .or_insert((t, op.workspace_name.as_deref()));
            if t < entry.0 {
                *entry = (t, op.workspace_name.as_deref());
            }
        }
        let args = op.attributes.get("args").map_or("", String::as_str);
        let desk = if op.description.starts_with("add workspace") || args.contains(" workspace add")
        {
            Some("other:desk_create")
        } else if op.description.starts_with("forget workspace")
            || args.contains(" workspace forget")
        {
            Some("other:desk_remove")
        } else {
            None
        };
        if let Some(kind) = desk
            && since.is_none_or(|s| t >= s)
        {
            let mut e = Event::new(t, Kind::Other(kind.to_owned()), format!("{source} op log"));
            // an added workspace's op names no workspace; its description does
            e.actor = op
                .workspace_name
                .clone()
                .or_else(|| quoted(&op.description).map(str::to_owned));
            e.set_attr("description", op.description.clone());
            out.push(e);
        }
    }

    // Newest first. A commit with no operation of its own landed in the batch
    // of its nearest descendant's operation, if it was committed after the
    // trunk move before that one; otherwise it predates what the op log shows
    // and its committer time stands in.
    let mut moves: Vec<Timestamp> = first_op.values().map(|(t, _)| *t).collect();
    moves.sort();
    let mut best: Option<(Timestamp, Option<&str>)> = None;
    for commit in landed {
        let own = first_op.get(commit.commit_id.as_str()).copied();
        if let Some(o) = own
            && best.is_none_or(|b| o.0 < b.0)
        {
            best = Some(o);
        }
        let committed = parse_instant(&commit.committer.timestamp)?;
        let batch = own.or_else(|| {
            let (t, w) = best?;
            let previous = moves.iter().rev().find(|m| **m < t);
            previous.is_none_or(|p| committed > *p).then_some((t, w))
        });
        let (t, workspace) = batch.unwrap_or((committed, None));
        let parsed = Parsed::new(&commit.description, trailers, goal);
        let mut land = Event::new(
            t,
            Kind::Land,
            format!("{source} log first_ancestors({trunk})"),
        );
        land.outcome = Some(Outcome::Pass);
        fill(&mut land, commit, &parsed);
        if land.actor.is_none() {
            land.actor = workspace.map(str::to_owned);
        }
        land.set_attr("workspace", workspace.unwrap_or_default());
        land.set_attr(
            "land_time",
            if batch.is_some() { "op" } else { "committer" },
        );
        out.push(land);
        out.push(start(commit, &parsed, source, true)?);
    }
    for commit in unlanded {
        let parsed = Parsed::new(&commit.description, trailers, goal);
        out.push(start(commit, &parsed, source, false)?);
    }
    Ok(out)
}

/// The change-start event for a commit.
fn start(commit: &JjCommit, parsed: &Parsed, source: &str, landed: bool) -> Result<Event> {
    let mut e = Event::new(
        parse_instant(&commit.author.timestamp)?,
        Kind::Commit,
        format!("{source} log (author time)"),
    );
    fill(&mut e, commit, parsed);
    if landed {
        e.set_attr("landed", true);
    }
    Ok(e)
}

fn fill(e: &mut Event, commit: &JjCommit, parsed: &Parsed) {
    e.ids.commit = Some(commit.commit_id.clone());
    e.ids.change = Some(commit.change_id.clone());
    e.ids.session.clone_from(&parsed.session);
    e.ids.goal.clone_from(&parsed.goal);
    e.ids.cites.clone_from(&parsed.cites);
    e.subject.clone_from(&parsed.goal);
    e.actor = parsed.seat.clone().or_else(|| parsed.workspace.clone());
    e.set_attr("title", parsed.title.clone());
    for (key, values) in &parsed.kept {
        e.set_attr(key, values.clone());
    }
}

/// The first `'quoted'` name in a description: `add workspace 'andon-1c'`.
fn quoted(text: &str) -> Option<&str> {
    let start = text.find('\'')? + 1;
    let len = text[start..].find('\'')?;
    Some(&text[start..start + len])
}

/// What a commit description says, by the project's trailer names.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub title: String,
    pub goal: Option<String>,
    pub session: Option<String>,
    pub seat: Option<String>,
    pub workspace: Option<String>,
    /// Verdict ids from review trailers.
    pub cites: Vec<String>,
    /// Kept trailers (`trailers.keep`), by lowercased name.
    pub kept: std::collections::BTreeMap<String, Vec<String>>,
}

impl Parsed {
    #[must_use]
    pub fn new(description: &str, trailers: &Trailers, goal: &Regex) -> Self {
        let title = description.lines().next().unwrap_or_default().to_owned();
        // the subject's last match: the convention ends it with the goal
        let last = goal
            .captures_iter(&title)
            .filter_map(|c| c.iter().skip(1).flatten().next().or_else(|| c.get(0)))
            .last()
            .map(|m| m.as_str().to_owned());
        let mut parsed = Self {
            goal: last,
            title,
            ..Self::default()
        };
        for (key, value) in trailer_lines(description) {
            if key == trailers.session {
                parsed.session.get_or_insert_with(|| value.to_owned());
            } else if trailers.seat.as_deref() == Some(key) {
                parsed.seat.get_or_insert_with(|| value.to_owned());
            } else if key == trailers.workspace {
                parsed.workspace.get_or_insert_with(|| value.to_owned());
            } else if trailers.keep.iter().any(|k| k == key) {
                parsed
                    .kept
                    .entry(key.to_lowercase())
                    .or_default()
                    .push(value.to_owned());
            } else if trailers.critic.as_deref() == Some(key) {
                // the configured word is the verdict id; a one-word value is it
                let words: Vec<&str> = value.split_whitespace().collect();
                let word = trailers.critic_id_word.saturating_sub(1);
                if let Some(id) = words.get(word).or_else(|| words.first()) {
                    parsed.cites.push((*id).to_owned());
                }
            }
        }
        parsed
    }
}

/// `Key: value` lines in a description (after its title).
fn trailer_lines(description: &str) -> impl Iterator<Item = (&str, &str)> {
    description.lines().skip(1).filter_map(|line| {
        let (key, value) = line.trim().split_once(": ")?;
        let ok = key.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        (ok && !value.trim().is_empty()).then(|| (key, value.trim()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal() -> Regex {
        Regex::new(r"\bandon-[a-z0-9]{3}\b").unwrap_or_else(|e| panic!("{e}"))
    }

    fn commit(id: &str, author: &str, committer: &str, description: &str) -> JjCommit {
        JjCommit {
            commit_id: id.to_owned(),
            change_id: format!("change-{id}"),
            description: description.to_owned(),
            author: Signature {
                timestamp: author.to_owned(),
            },
            committer: Signature {
                timestamp: committer.to_owned(),
            },
        }
    }

    fn op(start: &str, description: &str, workspace: &str) -> JjOp {
        JjOp {
            time: OpTime {
                start: start.to_owned(),
            },
            description: description.to_owned(),
            workspace_name: Some(workspace.to_owned()),
            attributes: HashMap::new(),
        }
    }

    #[test]
    fn descriptions_yield_goal_session_and_citations() {
        let p = Parsed::new(
            "board: rows (andon-xa4)\n\nbody\n\nReviewed-By: andon-xa4 0f0e-uuid abcd\nJj-Workspace: andon-1b\nSession: s-1\n",
            &Trailers {
                critic: Some("Reviewed-By".into()),
                critic_id_word: 2,
                ..Trailers::default()
            },
            &goal(),
        );
        assert_eq!(p.goal.as_deref(), Some("andon-xa4"));
        assert_eq!(p.session.as_deref(), Some("s-1"));
        assert_eq!(p.workspace.as_deref(), Some("andon-1b"));
        assert_eq!(p.cites, vec!["0f0e-uuid".to_owned()]);
        assert_eq!(p.seat, None);
        let keep = Trailers {
            keep: vec!["Purchase".into()],
            ..Trailers::default()
        };
        let p = Parsed::new("x (andon-xa4)\n\nPurchase: andon-76c\n", &keep, &goal());
        assert_eq!(p.kept.get("purchase"), Some(&vec!["andon-76c".to_owned()]));
    }

    #[test]
    fn a_batch_lands_at_the_operation_that_moved_the_trunk() {
        // newest first: c3 was pointed to by one op; c2 lands with it
        let landed = vec![
            commit(
                "c3",
                "2026-10-13T10:00:00Z",
                "2026-10-13T10:05:00Z",
                "c: three (andon-xa4)\n\nSeat: andon-1b\n",
            ),
            commit(
                "c2",
                "2026-10-13T09:00:00Z",
                "2026-10-13T09:05:00Z",
                "c: two (andon-xa4)\n",
            ),
            commit(
                "c1",
                "2026-10-12T09:00:00Z",
                "2026-10-12T09:05:00Z",
                "c: one\n",
            ),
        ];
        let ops = vec![
            op(
                "2026-10-12T12:00:00Z",
                "point bookmark main to commit c0",
                "andon-1a",
            ),
            op(
                "2026-10-13T11:00:00Z",
                "point bookmark main to commit c3",
                "andon-1b",
            ),
            op(
                "2026-10-13T11:00:05Z",
                "push bookmark main to git remote origin",
                "andon-1b",
            ),
        ];
        let events = events(
            &landed,
            &[],
            &ops,
            "main",
            &Trailers::default(),
            &goal(),
            "jj",
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let lands: Vec<&Event> = events.iter().filter(|e| e.kind == Kind::Land).collect();
        assert_eq!(lands.len(), 3);
        let t = |s: &str| parse_instant(s).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(lands[0].t, t("2026-10-13T11:00:00Z"));
        assert_eq!(lands[0].actor.as_deref(), Some("andon-1b"));
        assert_eq!(lands[1].t, t("2026-10-13T11:00:00Z"));
        // the workspace that moved the trunk stands in for a missing trailer
        assert_eq!(lands[1].actor.as_deref(), Some("andon-1b"));
        // c1 was committed before the previous trunk move: committer time
        assert_eq!(lands[2].t, t("2026-10-12T09:05:00Z"));
        assert_eq!(lands[2].attr_str("land_time"), Some("committer"));
        let starts: Vec<&Event> = events.iter().filter(|e| e.kind == Kind::Commit).collect();
        assert_eq!(starts[0].t, t("2026-10-13T10:00:00Z"));
        assert_eq!(starts[0].subject.as_deref(), Some("andon-xa4"));
    }

    #[test]
    fn workspace_operations_become_desk_events() {
        let ops = vec![op(
            "2026-10-13T11:00:00Z",
            "add workspace 'andon-1c'",
            "andon-1c",
        )];
        let events = events(
            &[],
            &[],
            &ops,
            "main",
            &Trailers::default(),
            &goal(),
            "jj",
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, Kind::Other("other:desk_create".to_owned()));
        assert_eq!(quoted("add workspace 'andon-1c'"), Some("andon-1c"));
    }
}
