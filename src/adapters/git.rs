//! vcs: git. Reads the trunk's first-parent history with `git log`, which
//! writes nothing.
//!
//! git keeps no shared record of when the trunk moved (the reflog is local),
//! so a landing is timed by its trunk commit's committer time: for a merge or
//! squash on GitHub, the merge. A merged branch's change starts at its
//! earliest commit's author time.
//!
//! Emits `land` for each trunk commit and `commit` (change start) for each.
//! The actor is the seat trailer, else the merged branch through
//! `branch_seats`, else the author's name. A pull-request number found in the
//! subject (`(#12)`, `Merge pull request #12`) becomes `ids.change = "pr#12"`,
//! which is how review verdicts from the github adapter join the landing.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use anyhow::Result;
use jiff::Timestamp;
use regex::Regex;

use crate::adapters::run_tool;
use crate::config::{Config, GitConfig};
use crate::event::{Event, Kind, Outcome};
use crate::time::parse_instant;

/// One commit as `git log` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommit {
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_time: String,
    pub commit_time: String,
    pub message: String,
}

const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%aI%x1f%cI%x1f%B%x1e";

/// The configured path must be the top level of its own git repository.
/// git searches upward, so a path without its own `.git` (a jj-only
/// workspace under a home directory that has one) would otherwise be
/// answered for by an enclosing repository, silently.
fn top_level(dir: &std::path::Path) -> Result<()> {
    let fix = "point [adapters.git] repo at the repository's root";
    let shown = dir.display();
    let out = run_tool(
        "git",
        &["-C", &dir.to_string_lossy(), "rev-parse", "--show-toplevel"],
        "install git, or remove [adapters.git]",
    )
    .map_err(|e| {
        anyhow::anyhow!("{shown} is not the top level of a git repository ({e}); {fix}")
    })?;
    let top = std::path::Path::new(out.trim());
    let same = top
        .canonicalize()
        .ok()
        .zip(dir.canonicalize().ok())
        .is_some_and(|(a, b)| a == b);
    if !same {
        anyhow::bail!(
            "{shown} is not the top level of a git repository (git answers for {}); {fix}",
            top.display()
        );
    }
    Ok(())
}

/// Read the repository.
///
/// # Errors
/// When git cannot run or a timestamp cannot be parsed.
pub fn collect(config: &Config, gc: &GitConfig) -> Result<Vec<Event>> {
    let dir = config.path(&gc.repo);
    top_level(&dir)?;
    let dir = dir.to_string_lossy();
    let mut args = vec!["-C", &dir, "log", FORMAT];
    // a UTC instant before `since`: git's approxidate reads a bare date in
    // local time at the current time of day, which would decide what is missed
    let from = gc.since.as_deref().map(parse_instant).transpose()?;
    let since = from.map(|s| format!("--since={}", crate::time::read_from(s)));
    if let Some(since) = &since {
        args.push(since);
    }
    let first_parent: Vec<&str> =
        [args.as_slice(), &["--first-parent", gc.trunk.as_str()]].concat();
    let trunk = parse_log(&run_tool(
        "git",
        &first_parent,
        "install git, or remove [adapters.git]",
    )?);
    let all_args: Vec<&str> = [args.as_slice(), &[gc.trunk.as_str()]].concat();
    let all = parse_log(&run_tool(
        "git",
        &all_args,
        "install git, or remove [adapters.git]",
    )?);
    let source = format!("git -C {dir}");
    let mut out = events(&trunk, &all, config, gc, &source)?;
    // what this read covers: the trunk from `since` (or all history) to now
    out.push(crate::event::coverage(
        from.unwrap_or(crate::event::ALL_HISTORY),
        Some(config.capture()),
        &["land", "commit"],
        source,
        None,
    ));
    Ok(out)
}

/// One commit, for `check commit`: its id, subject, message, and the seat
/// its trailer names.
///
/// # Errors
/// When git cannot resolve the revision.
pub fn describe(config: &Config, gc: &GitConfig, rev: &str) -> Result<crate::check::Commit> {
    let dir = config.path(&gc.repo);
    top_level(&dir)?;
    let dir = dir.to_string_lossy();
    let out = run_tool(
        "git",
        &["-C", &dir, "log", "-1", FORMAT, rev],
        "install git",
    )?;
    let commit = parse_log(&out)
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("git names no commit {rev}"))?;
    let parsed = super::jj::Parsed::new(&commit.message, &gc.trailers, &config.goal_regex()?);
    Ok(crate::check::Commit {
        id: commit.id,
        title: parsed.title,
        message: commit.message,
        seat: parsed.seat,
    })
}

/// Parse `git log` output in [`FORMAT`].
#[must_use]
pub fn parse_log(out: &str) -> Vec<GitCommit> {
    out.split('\x1e')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let mut f = record.splitn(6, '\x1f');
            let id = f.next()?.trim().to_owned();
            if id.is_empty() {
                return None;
            }
            Some(GitCommit {
                id,
                parents: f.next()?.split_whitespace().map(str::to_owned).collect(),
                author: f.next()?.to_owned(),
                author_time: f.next()?.to_owned(),
                commit_time: f.next()?.to_owned(),
                message: f.next().unwrap_or_default().trim_end().to_owned(),
            })
        })
        .collect()
}

static MERGED_BRANCH: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^Merge pull request #\d+ from [^/\s]+/(\S+)").ok());

/// Turn trunk commits into events. `all` is every commit reachable from the
/// trunk, for finding when a merged branch started. Pure; tested without git.
///
/// # Errors
/// When a timestamp cannot be parsed.
pub fn events(
    trunk: &[GitCommit],
    all: &[GitCommit],
    config: &Config,
    gc: &GitConfig,
    source: &str,
) -> Result<Vec<Event>> {
    let pattern = config.goal_regex()?;
    let pr = Regex::new(&gc.pr_pattern)?;
    let on_trunk: HashSet<&str> = trunk.iter().map(|c| c.id.as_str()).collect();
    let by_id: HashMap<&str, &GitCommit> = all.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut out = Vec::new();
    for commit in trunk {
        let subject = commit.message.lines().next().unwrap_or_default();
        let trailers = super::jj::Parsed::new(&commit.message, &gc.trailers, &pattern);
        let goal = crate::goal::cited(&pattern, subject, &commit.message, config);
        let change = crate::goal::find(&pr, subject).map(|n| format!("pr#{n}"));
        let branch = MERGED_BRANCH
            .as_ref()
            .and_then(|re| re.captures(subject))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned());
        let actor = trailers
            .seat
            .clone()
            .or_else(|| branch.as_deref().and_then(|b| branch_seat(b, gc)))
            .unwrap_or_else(|| commit.author.clone());
        let fill = |e: &mut Event| {
            e.actor = Some(actor.clone());
            e.subject.clone_from(&goal);
            e.ids.goal.clone_from(&goal);
            e.ids.commit = Some(commit.id.clone());
            e.ids.change.clone_from(&change);
            e.ids.session.clone_from(&trailers.session);
            e.ids.cites.clone_from(&trailers.cites);
            e.set_attr("title", subject.to_owned());
            e.set_attr("author", commit.author.clone());
            e.set_attr("branch", branch.clone().unwrap_or_default());
        };
        let mut land = Event::new(
            parse_instant(&commit.commit_time)?,
            Kind::Land,
            format!("{source} log --first-parent {}", gc.trunk),
        );
        land.outcome = Some(Outcome::Pass);
        fill(&mut land);
        out.push(land);
        let start = branch_start(commit, &on_trunk, &by_id)?;
        let mut begun = Event::new(start, Kind::Commit, format!("{source} log (author time)"));
        fill(&mut begun);
        begun.set_attr("landed", true);
        out.push(begun);
    }
    Ok(out)
}

/// The seat a merged branch belongs to, by the longest configured prefix.
fn branch_seat(branch: &str, gc: &GitConfig) -> Option<String> {
    gc.branch_seats
        .iter()
        .filter(|(prefix, _)| branch.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, seat)| seat.clone())
}

/// When the change behind a trunk commit started: its own author time, or
/// for a merge, the earliest author time on the merged side (walking back
/// until the trunk, at most 1,000 commits).
fn branch_start(
    commit: &GitCommit,
    on_trunk: &HashSet<&str>,
    by_id: &HashMap<&str, &GitCommit>,
) -> Result<Timestamp> {
    let mut earliest = parse_instant(&commit.author_time)?;
    let mut stack: Vec<&str> = commit.parents.iter().skip(1).map(String::as_str).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    while let Some(id) = stack.pop() {
        if on_trunk.contains(id) || !seen.insert(id) || seen.len() > 1_000 {
            continue;
        }
        let Some(c) = by_id.get(id) else { continue };
        let t = parse_instant(&c.author_time)?;
        if t < earliest {
            earliest = t;
        }
        stack.extend(c.parents.iter().map(String::as_str));
    }
    Ok(earliest)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn commit(
        id: &str,
        parents: &[&str],
        author_time: &str,
        commit_time: &str,
        message: &str,
    ) -> GitCommit {
        GitCommit {
            id: id.into(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            author: "octo".into(),
            author_time: author_time.into(),
            commit_time: commit_time.into(),
            message: message.into(),
        }
    }

    fn config() -> (Config, GitConfig) {
        let config = Config::parse(
            r#"
[project]
goal_pattern = '(?:[Cc]loses|[Ff]ixes) (#\d+)'
goal_in = "message"
[seats.bot-1]
[adapters.git]
branch_seats = { "bot-1/" = "bot-1" }
"#,
            Path::new("/"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let gc = config.adapters.git.clone().unwrap_or_else(|| panic!("git"));
        (config, gc)
    }

    #[test]
    fn git_log_records_parse() {
        let out = "abc\x1fp1 p2\x1focto\x1f2026-10-13T10:00:00+02:00\x1f2026-10-13T11:00:00Z\x1fMerge pull request #7 from o/bot-1/x\n\nCloses #3\n\x1e\ndef\x1f\x1focto\x1f2026-10-12T10:00:00Z\x1f2026-10-12T10:00:00Z\x1finit\n\x1e\n";
        let commits = parse_log(out);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, vec!["p1", "p2"]);
        assert_eq!(
            commits[0].message,
            "Merge pull request #7 from o/bot-1/x\n\nCloses #3"
        );
        assert_eq!(commits[1].parents, [] as [std::string::String; 0]);
    }

    #[test]
    fn a_merged_pull_request_lands_with_its_goal_seat_and_branch_start() {
        let (config, gc) = config();
        let trunk = vec![
            commit(
                "m",
                &["base", "b2"],
                "2026-10-13T12:00:00Z",
                "2026-10-13T12:00:00Z",
                "Merge pull request #7 from octo/bot-1/fix\n\nCloses #3\n",
            ),
            commit(
                "base",
                &[],
                "2026-10-12T00:00:00Z",
                "2026-10-12T00:00:00Z",
                "init",
            ),
        ];
        let mut all = trunk.clone();
        all.push(commit(
            "b2",
            &["b1"],
            "2026-10-13T11:00:00Z",
            "2026-10-13T11:00:00Z",
            "two",
        ));
        all.push(commit(
            "b1",
            &["base"],
            "2026-10-13T09:00:00Z",
            "2026-10-13T09:00:00Z",
            "one",
        ));
        let events = events(&trunk, &all, &config, &gc, "git").unwrap_or_else(|e| panic!("{e}"));
        let land = &events[0];
        assert_eq!(land.kind, Kind::Land);
        assert_eq!(land.subject.as_deref(), Some("#3"));
        assert_eq!(land.ids.change.as_deref(), Some("pr#7"));
        assert_eq!(land.actor.as_deref(), Some("bot-1"));
        let start = &events[1];
        assert_eq!(start.kind, Kind::Commit);
        assert_eq!(
            start.t,
            parse_instant("2026-10-13T09:00:00Z").unwrap_or_else(|e| panic!("{e}"))
        );
    }

    #[test]
    fn a_squash_merge_names_its_pull_request() {
        let (config, gc) = config();
        let trunk = vec![commit(
            "s",
            &["base"],
            "2026-10-13T09:00:00Z",
            "2026-10-13T12:00:00Z",
            "Fix the board (#9)\n\nFixes #4\n",
        )];
        let events = events(&trunk, &trunk, &config, &gc, "git").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(events[0].ids.change.as_deref(), Some("pr#9"));
        assert_eq!(events[0].subject.as_deref(), Some("#4"));
        assert_eq!(events[0].actor.as_deref(), Some("octo"));
    }
}
