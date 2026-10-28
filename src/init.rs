//! `andon init`: a config for the repository it runs in, from what the
//! directory shows: its name, jj or git and the trunk, bd and its issue
//! prefix, jj's workspaces as seats, Claude Code's session files and
//! hail's store. It prints to stdout and writes nothing.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What `init` found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each is one source init found or did not"
)]
pub struct Found {
    pub name: String,
    /// The repository's root (its main workspace), relative paths from it.
    pub root: PathBuf,
    pub vcs: Option<Vcs>,
    pub trunk: Option<String>,
    /// bd's issue prefix, when the repository has a `.beads` directory.
    pub bd_prefix: Option<String>,
    pub bd: bool,
    /// jj workspaces (each a seat), the default one first.
    pub workspaces: Vec<String>,
    /// Claude Code's session files (`~/.claude/sessions`) exist.
    pub presence: bool,
    /// hail's store exists.
    pub hail: bool,
    /// Ferry receipts in the repository.
    pub ferry: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vcs {
    Jj,
    Git,
}

/// Look at `cwd`: the nearest directory holding `.jj` or `.git`, and what
/// its tools say. Every read is a read; nothing is written.
#[must_use]
pub fn detect(cwd: &Path, home: Option<&Path>) -> Found {
    let mut found = Found::default();
    let top = cwd
        .ancestors()
        .find(|d| d.join(".jj").exists() || d.join(".git").exists())
        .unwrap_or(cwd);
    // a secondary jj workspace names its repository in `.jj/repo`: the
    // project is that repository's directory
    let root = jj_store_root(top).unwrap_or_else(|| top.to_path_buf());
    found.name = root.file_name().map_or_else(
        || "project".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    found.root.clone_from(&root);
    if top.join(".jj").exists() {
        found.vcs = Some(Vcs::Jj);
        let bookmarks = tool(
            top,
            "jj",
            &[
                "--ignore-working-copy",
                "bookmark",
                "list",
                "-T",
                "name ++ \"\\n\"",
            ],
        );
        found.trunk = pick_trunk(bookmarks.lines());
        let listed = tool(
            top,
            "jj",
            &[
                "--ignore-working-copy",
                "workspace",
                "list",
                "-T",
                "name ++ \"\\n\"",
            ],
        );
        let mut names: Vec<String> = listed
            .lines()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect();
        names.sort_by_key(|n| (n != "default", n.clone()));
        found.workspaces = names;
    } else if top.join(".git").exists() {
        found.vcs = Some(Vcs::Git);
        let branches = tool(top, "git", &["branch", "--format=%(refname:short)"]);
        found.trunk = pick_trunk(branches.lines());
    }
    let beads = root.join(".beads");
    if beads.exists() {
        found.bd = true;
        found.bd_prefix = std::fs::read_to_string(beads.join("config.yaml"))
            .ok()
            .and_then(|text| prefix_of(&text))
            .or_else(|| {
                // no prefix in the config: the first issue id bd prints
                let out = tool(&root, "bd", &["list", "--json", "--limit", "1"]);
                serde_json::from_str::<serde_json::Value>(&out)
                    .ok()
                    .and_then(|v| v.get(0)?.get("id")?.as_str().map(str::to_owned))
                    .and_then(|id| id.rsplit_once('-').map(|(p, _)| p.to_owned()))
            });
    }
    found.ferry = root.join(".ferry").exists();
    if let Some(home) = home {
        found.presence = home.join(".claude/sessions").exists();
        found.hail = home.join(".local/state/hail").exists();
    }
    found
}

/// The repository directory a jj workspace at `top` belongs to.
fn jj_store_root(top: &Path) -> Option<PathBuf> {
    let repo = top.join(".jj/repo");
    if repo.is_dir() {
        return None;
    }
    let named = std::fs::read_to_string(&repo).ok()?;
    let store = top.join(".jj").join(named.trim());
    // <root>/.jj/repo
    let store = std::fs::canonicalize(store).ok()?;
    store.parent()?.parent().map(Path::to_path_buf)
}

/// The trunk among branch or bookmark names: main, then master, then trunk.
fn pick_trunk<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    let names: Vec<&str> = names.map(str::trim).collect();
    ["main", "master", "trunk"]
        .into_iter()
        .find(|t| names.contains(t))
        .map(str::to_owned)
}

/// `issue-prefix: x` (or `issue_prefix`) in bd's config.
fn prefix_of(config: &str) -> Option<String> {
    config.lines().find_map(|line| {
        let line = line.trim();
        let rest = line
            .strip_prefix("issue-prefix:")
            .or_else(|| line.strip_prefix("issue_prefix:"))?;
        let p = rest.trim().trim_matches(['"', '\'']);
        (!p.is_empty()).then(|| p.to_owned())
    })
}

/// A tool's stdout, or nothing when it cannot run: detection guesses, it
/// never fails.
fn tool(dir: &Path, program: &str, args: &[&str]) -> String {
    crate::adapters::run_tool_in(dir, program, args, "").unwrap_or_default()
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The config for what was found, with a comment on each guess.
#[must_use]
pub fn config(found: &Found) -> String {
    let mut out = String::new();
    let name = &found.name;
    let _ = writeln!(
        out,
        "# andon.toml for {name}, from what `andon init` found here. andon reads\n# the adapters below and writes to none of them; see `andon --help`.\n"
    );
    let _ = writeln!(out, "[project]\nname = {}", toml_str(name));
    let prefix = found.bd_prefix.as_deref().unwrap_or(name);
    let _ = writeln!(
        out,
        "# A goal id as it appears in commit subjects and messages{}.",
        if found.bd_prefix.is_some() {
            " (bd's prefix)"
        } else {
            ""
        }
    );
    let _ = writeln!(
        out,
        "goal_pattern = '\\b{}-[a-z0-9]{{3,6}}(\\.[0-9]+)*\\b'",
        regex::escape(prefix)
    );
    let _ = writeln!(
        out,
        "# The gate recipe that lands a change: a failed run shows as \"last land failed\".\nland_recipe = \"land\"\n"
    );
    let _ = writeln!(
        out,
        "# Identity first: every event matches a seat by the name its tool recorded.\n# Each agent acts under its seat's name: with bd, BEADS_ACTOR=<seat>."
    );
    let seats: Vec<String> = if found.workspaces.is_empty() {
        vec![format!("{name}-1a")]
    } else {
        found
            .workspaces
            .iter()
            .map(|w| {
                if w == "default" {
                    name.clone()
                } else {
                    w.clone()
                }
            })
            .collect()
    };
    if found.workspaces.is_empty() {
        let _ = writeln!(
            out,
            "# One table per seat; no jj workspaces were found, so this one is a guess."
        );
    } else {
        let _ = writeln!(out, "# One seat per jj workspace (`jj workspace list`).");
    }
    for (seat, workspace) in seats.iter().zip(
        found
            .workspaces
            .iter()
            .map(Some)
            .chain(std::iter::repeat(None)),
    ) {
        let _ = writeln!(out, "[seats.{}]", toml_key(seat));
        if workspace.is_some_and(|w| w == "default") {
            let _ = writeln!(
                out,
                "# the main checkout: jj calls its workspace default\naliases = [\"default\"]"
            );
        } else {
            // a seat's extra workspaces (subagents' desks) start with its name
            let _ = writeln!(out, "prefixes = [{}]", toml_str(&format!("{seat}-")));
        }
    }
    let _ = writeln!(
        out,
        "\n# The goal schema: `check commit` and `check goal` check work against it, and the\n# board shows goals and budgets. Remove [goal] for a board without them.\n[goal]\nsections = [\"Why\", \"Done when\", \"Budget\"]\nwip_per_seat = 1\n"
    );
    let _ = writeln!(
        out,
        "# Messages that ask for a decision (hail 0.7: ask, fyi, announce).\n[messages]\ndecision_kinds = [\"ask\"]"
    );
    adapters(found, &mut out);
    out
}

/// The adapters for what was found.
fn adapters(found: &Found, out: &mut String) {
    let name = &found.name;
    match found.vcs {
        Some(Vcs::Jj) => {
            let _ = writeln!(
                out,
                "\n[adapters.jj]\nrepo = \".\"\ntrunk = {}",
                toml_str(found.trunk.as_deref().unwrap_or("main"))
            );
        }
        Some(Vcs::Git) => {
            let _ = writeln!(
                out,
                "\n[adapters.git]\nrepo = \".\"\ntrunk = {}",
                toml_str(found.trunk.as_deref().unwrap_or("main"))
            );
        }
        None => {
            let _ = writeln!(
                out,
                "\n# no .jj or .git here: add [adapters.jj] or [adapters.git]"
            );
        }
    }
    if found.bd {
        let _ = writeln!(out, "\n[adapters.bd]\ndir = \".\"");
    }
    if found.ferry {
        let _ = writeln!(
            out,
            "\n[adapters.ferry]\nreceipts = [\"{name}*/.ferry/receipts\"]"
        );
    }
    let dashed = found.root.display().to_string().replace(['/', '.'], "-");
    let _ = writeln!(
        out,
        "\n# Transcripts of sessions that worked here.\n[adapters.sessions]\nclaude = [\"~/.claude*/projects/{dashed}*\"]\ncodex = [\"~/.codex/sessions\"]\ncwd = [{}]",
        toml_str(&format!("{}*", found.root.display()))
    );
    if found.hail {
        let _ = writeln!(
            out,
            "\n[adapters.hail]\nlabels = [{}]",
            toml_str(&format!("{name}*"))
        );
    }
    if found.presence {
        let _ = writeln!(
            out,
            "\n# Who is running now, from Claude Code's session files.\n[adapters.presence]"
        );
    }
}

/// A seat name as a TOML table key.
fn toml_key(name: &str) -> String {
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        name.to_owned()
    } else {
        toml_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trunk_and_prefix_are_read_from_what_tools_print() {
        assert_eq!(
            pick_trunk(["dev", "master"].into_iter()).as_deref(),
            Some("master")
        );
        assert_eq!(
            pick_trunk(["main", "master"].into_iter()).as_deref(),
            Some("main")
        );
        assert_eq!(pick_trunk(["dev"].into_iter()), None);
        assert_eq!(
            prefix_of("# x\nissue-prefix: demo\n").as_deref(),
            Some("demo")
        );
        assert_eq!(prefix_of("issue_prefix: \"x-y\"\n").as_deref(), Some("x-y"));
    }

    #[test]
    fn what_init_prints_loads_as_a_config() {
        let found = Found {
            name: "demo".into(),
            root: PathBuf::from("/code/demo"),
            vcs: Some(Vcs::Jj),
            trunk: Some("master".into()),
            bd_prefix: Some("dm".into()),
            bd: true,
            workspaces: vec!["default".into(), "demo-1b".into()],
            presence: true,
            hail: true,
            ferry: true,
        };
        let text = config(&found);
        let parsed = crate::config::Config::parse(&text, Path::new("/code/demo"))
            .unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(parsed.seats.len(), 2);
        assert_eq!(parsed.messages.decision_kinds, vec!["ask".to_owned()]);
        assert!(
            regex::Regex::new(&parsed.project.goal_pattern)
                .is_ok_and(|r| r.is_match("dm-a1b") && r.is_match("dm-a1b.2"))
        );
        // with nothing found, still a config
        let bare = config(&Found {
            name: "x".into(),
            ..Found::default()
        });
        crate::config::Config::parse(&bare, Path::new("/x"))
            .unwrap_or_else(|e| panic!("{e}\n{bare}"));
    }
}
