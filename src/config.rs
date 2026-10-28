//! `andon.toml`: every project convention andon knows lives here, never in the
//! fold (founding design §8). A trailer name, a goal-id pattern, a bot account,
//! a seat's labels in each source: all configuration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;

/// The file andon looks for, walking up from the working directory.
pub const FILE_NAME: &str = "andon.toml";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub project: Project,
    /// Seat name → how that seat is known in each source.
    #[serde(default)]
    pub seats: BTreeMap<String, SeatConfig>,
    #[serde(default)]
    pub pools: BTreeMap<String, Pool>,
    #[serde(default)]
    pub goal: GoalRules,
    #[serde(default)]
    pub messages: MessageRules,
    #[serde(default)]
    pub board: BoardRules,
    #[serde(default)]
    pub adapters: Adapters,
    /// The directory the config was read from; relative paths resolve here.
    #[serde(skip)]
    pub root: PathBuf,
    /// The capture instant: the clock read once, before the sources, that
    /// every adapter stamps as the end of what it read (and a current-state
    /// source as the one instant it vouches for).
    #[serde(skip)]
    pub capture: Option<jiff::Timestamp>,
    /// Whether the file declares `[goal]`: without it, no goal schema
    /// applies (`.design/2026-10-19-goal-schema-as-config.md`).
    #[serde(skip)]
    pub goal_declared: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    #[serde(default)]
    pub name: String,
    /// A goal id, as a regular expression. Used to find goal ids in commit
    /// subjects, messages and receipts.
    #[serde(default = "default_goal_pattern")]
    pub goal_pattern: String,
    /// Where a commit cites its goal: `subject` (the first line) or
    /// `message` (anywhere, as GitHub's `Closes #12`).
    #[serde(default = "default_goal_in")]
    pub goal_in: String,
    /// Keep only goal ids the tracker has (when a tracker is configured), so
    /// a word shaped like an id (`x-review`) is not a goal.
    #[serde(default = "yes")]
    pub known_goals_only: bool,
    /// The gate recipe that is the project's land verb: its refusals are
    /// "last land refused", its passing runs time the land gate.
    #[serde(default = "default_land_recipe")]
    pub land_recipe: String,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            name: String::new(),
            goal_pattern: default_goal_pattern(),
            goal_in: default_goal_in(),
            known_goals_only: true,
            land_recipe: default_land_recipe(),
        }
    }
}

fn default_land_recipe() -> String {
    "land".to_owned()
}

fn default_goal_in() -> String {
    "subject".to_owned()
}

/// A goal id in parentheses, as `area: subject (goal-id)` writes it. Bare
/// words like `read-only` do not match; a project names its own pattern.
fn default_goal_pattern() -> String {
    r"\(([a-z][a-z0-9]*-[a-z0-9]{3,8}(?:\.[0-9]+)*)\)".to_owned()
}

/// How one seat is known. A label is matched exactly; a prefix claims any
/// label that starts with it (a seat's extra workspaces, `x-1a-par`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeatConfig {
    /// Exact labels: trailer values, tracker actors, message labels.
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub prefixes: Vec<String>,
    /// Working directories whose sessions belong to this seat.
    #[serde(default)]
    pub cwd: Vec<String>,
    /// Goals in progress this seat may hold; overrides `goal.wip_per_seat`.
    #[serde(default)]
    pub wip: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pool {
    /// Open goals the pool may hold; `check goal` refuses past it.
    #[serde(default)]
    pub cap: Option<usize>,
    #[serde(default)]
    pub seats: Vec<String>,
}

/// A tracker field that stands in for a goal section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum GoalField {
    /// The field's name.
    Name(String),
    /// A field holding a number, and the unit it is in.
    Number(NumberField),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberField {
    pub field: String,
    pub unit: String,
}

impl GoalField {
    /// The tracker field's name.
    #[must_use]
    pub fn field(&self) -> &str {
        match self {
            Self::Name(f) => f,
            Self::Number(n) => &n.field,
        }
    }

    /// The unit a number in the field is in: stated, or the field name's
    /// own ending (`estimated_minutes`).
    #[must_use]
    pub fn unit(&self) -> Option<&str> {
        match self {
            Self::Number(n) => Some(&n.unit),
            Self::Name(f) if f.ends_with("_minutes") => Some("minutes"),
            Self::Name(f) if f.ends_with("_hours") => Some("agent-hours"),
            Self::Name(_) => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalRules {
    /// Sections a goal's description must carry, as `## Heading` or `Heading:`.
    #[serde(default = "default_sections")]
    pub sections: Vec<String>,
    /// Which section holds the done-when; it must name a check that can fail.
    #[serde(default = "default_done_when")]
    pub done_when: String,
    /// Which section holds the budget; it must be finite.
    #[serde(default = "default_budget")]
    pub budget: String,
    /// Stems of check outcomes and limits. A done-when that names none of
    /// these (endings matched: "pass" covers "passing"), no command in
    /// backticks and no number (`-0.5`, `1e-6`, `95%`) draws a warning from
    /// `check goal`: it may not be able to fail. Advice: a heuristic never
    /// refuses a goal.
    #[serde(default = "default_checkable")]
    pub checkable: Vec<String>,
    /// Goals in progress per seat.
    #[serde(default = "default_wip")]
    pub wip_per_seat: usize,
    /// Whether `check goal` requires a pool section naming a configured pool.
    #[serde(default)]
    pub require_pool: bool,
    /// Sections kept in a tracker field rather than the description:
    /// section name to field (`"Done when" = "acceptance_criteria"`). The bd
    /// and GitHub adapters read the field as that section where the
    /// description has none. A field holding a number names its unit:
    /// `"Budget" = { field = "estimated", unit = "minutes" }`, or by a
    /// `_minutes` or `_hours` ending (`"Budget" = "estimated_minutes"`).
    #[serde(default)]
    pub fields: BTreeMap<String, GoalField>,
    /// Tracker item types that count toward WIP; others (an epic a lane's
    /// owner holds) do not. An item of no stated type counts.
    #[serde(default = "default_types")]
    pub types: Vec<String>,
}

impl Default for GoalRules {
    fn default() -> Self {
        Self {
            sections: default_sections(),
            done_when: default_done_when(),
            budget: default_budget(),
            checkable: default_checkable(),
            wip_per_seat: default_wip(),
            require_pool: false,
            fields: BTreeMap::new(),
            types: default_types(),
        }
    }
}

fn default_sections() -> Vec<String> {
    vec!["Why".into(), "Done when".into(), "Budget".into()]
}
fn default_types() -> Vec<String> {
    vec!["feature".into(), "bug".into(), "task".into()]
}
fn default_done_when() -> String {
    "Done when".into()
}
fn default_budget() -> String {
    "Budget".into()
}
fn default_checkable() -> Vec<String> {
    // stems (endings are matched: "pass" covers passes, passed, passing) and
    // limits (matched as whole words or phrases)
    [
        "pass",
        "fail",
        "refus",
        "exit",
        "print",
        "return",
        "reproduc",
        "assert",
        "equal",
        "match",
        "land",
        "merge",
        "close",
        "exist",
        "show",
        "fire",
        "contain",
        "succeed",
        "admit",
        "accept",
        "reject",
        "exceed",
        "report",
        "warning",
        "error",
        "zero",
        "under",
        "below",
        "above",
        "within",
        "at most",
        "at least",
        "no more than",
        "fewer than",
        "more than",
        "no seat",
        "no warnings",
        "every",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}
const fn default_wip() -> usize {
    1
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageRules {
    /// Message kinds that carry a decision and so must name a goal.
    #[serde(default = "default_decisions")]
    pub decision_kinds: Vec<String>,
    /// Every kind the messenger defines; `check message` refuses others, so
    /// a typo cannot pass as a non-decision. Kinds match without case.
    #[serde(default = "default_kinds")]
    pub kinds: Vec<String>,
    /// Seats whose inbound volume is the single-server gauge. Empty: the seat
    /// with the most inbound messages in the window.
    #[serde(default)]
    pub hubs: Vec<String>,
}

impl Default for MessageRules {
    fn default() -> Self {
        Self {
            decision_kinds: default_decisions(),
            kinds: default_kinds(),
            hubs: Vec::new(),
        }
    }
}

/// hail's kinds.
fn default_kinds() -> Vec<String> {
    [
        "ask", "go", "nogo", "ruling", "block", "stop", "fyi", "done", "announce", "hold",
        "release",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

fn default_decisions() -> Vec<String> {
    ["ask", "go", "nogo", "ruling", "block", "stop"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardRules {
    /// Flag a seat with no landing for this many days while it holds a goal.
    #[serde(default = "default_stale_days")]
    pub no_land_days: f64,
    /// How a budget of `N agent-hours` is measured: `agent-hours` counts
    /// session activity, `wall-clock` counts hours since the claim.
    #[serde(default = "default_budget_unit")]
    pub budget_unit: String,
    /// Counts in a row cover this many hours before the board's instant.
    #[serde(default = "default_window_hours")]
    pub window_hours: f64,
    /// Flag a seat with this many runs since its last landing.
    #[serde(default = "default_runs_without_land")]
    pub runs_without_land: usize,
    /// Flag a seat whose live session has been idle this many minutes while
    /// it holds a goal.
    #[serde(default = "default_idle_minutes")]
    pub idle_minutes: f64,
}

impl Default for BoardRules {
    fn default() -> Self {
        Self {
            no_land_days: default_stale_days(),
            budget_unit: default_budget_unit(),
            window_hours: default_window_hours(),
            runs_without_land: default_runs_without_land(),
            idle_minutes: default_idle_minutes(),
        }
    }
}

const fn default_window_hours() -> f64 {
    24.0
}
const fn default_idle_minutes() -> f64 {
    60.0
}
const fn default_runs_without_land() -> usize {
    20
}

const fn default_stale_days() -> f64 {
    1.0
}
fn default_budget_unit() -> String {
    "agent-hours".into()
}

/// Which adapters run, each with its own conventions.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adapters {
    pub jj: Option<JjConfig>,
    pub git: Option<GitConfig>,
    pub bd: Option<BdConfig>,
    pub github: Option<GithubConfig>,
    pub ferry: Option<FerryConfig>,
    pub sessions: Option<SessionsConfig>,
    pub hail: Option<HailConfig>,
    pub presence: Option<PresenceConfig>,
    /// Project-owned adapters: programs or files that speak andon's ledger
    /// format (see `adapters::external`). Each is named, and every event it
    /// emits carries its name.
    #[serde(default)]
    pub external: Vec<ExternalConfig>,
}

/// One external adapter.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalConfig {
    /// Its name, shown on every event it emits (`x-review` style:
    /// project, then what it reads).
    pub name: String,
    /// A program and its arguments; it prints events as JSONL on stdout.
    #[serde(default)]
    pub command: Vec<String>,
    /// Or files of events, `*` matching within a path component.
    #[serde(default)]
    pub files: Vec<String>,
    /// Where the command runs (default: the config's directory).
    #[serde(default)]
    pub dir: Option<String>,
    /// The contract version the adapter speaks.
    #[serde(default = "contract_one")]
    pub contract: u32,
}

const fn contract_one() -> u32 {
    1
}

/// Commit trailers, by role: the trailer names a project uses. The defaults
/// are andon's own (`scripts/jj-identity` writes `Jj-Workspace` and
/// `Session`); a seat trailer and a review-citation trailer are a project's
/// conventions, so they are read only when configured.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trailers {
    #[serde(default = "trailer_session")]
    pub session: String,
    #[serde(default = "trailer_workspace")]
    pub workspace: String,
    /// A trailer naming the committing seat.
    #[serde(default)]
    pub seat: Option<String>,
    /// A trailer citing a verdict, one per verdict.
    #[serde(default)]
    pub critic: Option<String>,
    /// Which word of the citation trailer's value is the verdict id
    /// (1-based); a value of one word is the id.
    #[serde(default = "first_word")]
    pub critic_id_word: usize,
    /// Other trailers kept on landings as attributes, by name.
    #[serde(default)]
    pub keep: Vec<String>,
}

impl Default for Trailers {
    fn default() -> Self {
        Self {
            session: trailer_session(),
            workspace: trailer_workspace(),
            seat: None,
            critic: None,
            critic_id_word: first_word(),
            keep: Vec::new(),
        }
    }
}

fn trailer_session() -> String {
    "Session".into()
}
fn trailer_workspace() -> String {
    "Jj-Workspace".into()
}
const fn first_word() -> usize {
    1
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JjConfig {
    /// A directory inside the repository (any workspace).
    #[serde(default = "dot")]
    pub repo: String,
    #[serde(default = "trunk")]
    pub trunk: String,
    #[serde(default)]
    pub trailers: Trailers,
    /// Only history after this instant is read; keeps live reads bounded.
    #[serde(default)]
    pub since: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitConfig {
    #[serde(default = "dot")]
    pub repo: String,
    #[serde(default = "trunk")]
    pub trunk: String,
    #[serde(default)]
    pub trailers: Trailers,
    /// Branch name prefix → seat, for landings that carry no seat trailer.
    #[serde(default)]
    pub branch_seats: BTreeMap<String, String>,
    /// A pull-request number in a trunk subject; its capture group is the
    /// number. Joins a landing to its review verdicts.
    #[serde(default = "default_pr_pattern")]
    pub pr_pattern: String,
    #[serde(default)]
    pub since: Option<String>,
}

fn default_pr_pattern() -> String {
    r"\(#(\d+)\)\s*$|^Merge pull request #(\d+)".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BdConfig {
    /// A directory where `bd` resolves the project's database.
    #[serde(default = "dot")]
    pub dir: String,
    /// How a review verdict is written as a comment, when the project
    /// writes them in its tracker. Absent: comments are findings only.
    #[serde(default)]
    pub verdicts: Option<VerdictFormat>,
    #[serde(default)]
    pub since: Option<String>,
    /// Read claims, closes, reopens and assignments as history from bd's
    /// events table (`bd sql`), where bd can serve it. Without it, or where
    /// bd cannot (an embedded database), they are the export's current state.
    #[serde(default = "yes")]
    pub events: bool,
    /// The zone bd wrote local times in, for rows stored as local time
    /// labelled UTC: an IANA name or a POSIX TZ string. Default: this
    /// machine's zone.
    #[serde(default)]
    pub events_zone: Option<String>,
    /// The first instant (as stored) from which every row is true UTC, and
    /// before which every row is local time. Absent: each row is resolved
    /// by calibrating against the issues' own timestamps.
    #[serde(default)]
    pub events_utc_from: Option<String>,
}

/// A verdict comment: a prefix, then a JSON object. Each field names where
/// in that object a fact is, as a dotted path (`critic.session`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerdictFormat {
    pub prefix: String,
    /// The verdict itself: go, approve or pass is GO; return, reject or
    /// fail is RETURN (any case).
    pub outcome: String,
    /// The reviewed commit.
    #[serde(default)]
    pub commit: Option<String>,
    /// The reviewed change ids: a string, a list, or a list of lists whose
    /// first items are the ids.
    #[serde(default)]
    pub change: Option<String>,
    /// The reviewer's session.
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubConfig {
    /// `owner/name`.
    pub repo: String,
    /// A label that marks an issue as a goal; empty: every issue.
    #[serde(default)]
    pub goal_label: Option<String>,
    /// Bot or account login → seat.
    #[serde(default)]
    pub accounts: BTreeMap<String, String>,
    /// Branch name prefix → seat, for Actions runs.
    #[serde(default)]
    pub branch_seats: BTreeMap<String, String>,
    #[serde(default)]
    pub since: Option<String>,
    /// Read JSON from this directory instead of calling `gh` (tests, offline).
    #[serde(default)]
    pub snapshot: Option<String>,
    /// How many issues, pull requests and runs to read.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Whether to read Actions runs.
    #[serde(default = "yes")]
    pub runs: bool,
}

const fn default_limit() -> usize {
    200
}
const fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FerryConfig {
    /// Receipt directories (`<workspace>/.ferry/receipts`); `*` matches
    /// within one path component. The directories are the scope: receipts
    /// name no repository.
    pub receipts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionsConfig {
    /// Claude Code project directories.
    #[serde(default)]
    pub claude: Vec<String>,
    /// Codex session directories.
    #[serde(default)]
    pub codex: Vec<String>,
    /// Keep sessions whose working directory matches (`*` wildcard). Empty
    /// keeps every session under the directories above.
    #[serde(default)]
    pub cwd: Vec<String>,
    /// Gap that ends an activity span, in minutes.
    #[serde(default = "default_gap")]
    pub gap_minutes: u32,
    #[serde(default)]
    pub since: Option<String>,
}

const fn default_gap() -> u32 {
    10
}

/// Harness session files: who is running now.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceConfig {
    /// Claude Code session directories (`<config dir>/sessions`), `*`
    /// matching within a path component.
    #[serde(default = "claude_sessions")]
    pub claude: Vec<String>,
}

fn claude_sessions() -> Vec<String> {
    vec!["~/.claude/sessions".into()]
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HailConfig {
    #[serde(default = "hail_state")]
    pub state: String,
    /// Keep messages whose sender or a recipient matches (`*` wildcard).
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub since: Option<String>,
}

fn hail_state() -> String {
    "~/.local/state/hail".into()
}
fn dot() -> String {
    ".".into()
}
fn trunk() -> String {
    "main".into()
}

impl Config {
    /// The capture instant. Every caller sets one before reading sources:
    /// the command line from its one clock read, a library caller itself.
    ///
    /// # Errors
    /// When none was set.
    pub fn capture(&self) -> Result<jiff::Timestamp> {
        self.capture.ok_or_else(|| {
            anyhow::anyhow!("config.capture is not set; set it before reading adapters")
        })
    }

    /// Whether a goal schema applies: `[goal]` is declared, even empty.
    #[must_use]
    pub const fn goal_schema(&self) -> bool {
        self.goal_declared
    }

    /// Parse a config. `root` is where relative paths resolve.
    ///
    /// # Errors
    /// When the TOML is invalid or a pattern does not compile.
    pub fn parse(text: &str, root: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(text).context("andon.toml is not valid")?;
        config.root = root.to_path_buf();
        config.goal_declared =
            toml::from_str::<toml::Table>(text).is_ok_and(|t| t.contains_key("goal"));
        if !matches!(config.project.goal_in.as_str(), "subject" | "message") {
            bail!(
                "project.goal_in is {:?}; use \"subject\" or \"message\"",
                config.project.goal_in
            );
        }
        Regex::new(&config.project.goal_pattern).with_context(|| {
            format!(
                "project.goal_pattern {:?} is not a valid regex",
                config.project.goal_pattern
            )
        })?;
        for (name, pool) in &config.pools {
            for seat in &pool.seats {
                if !config.seats.contains_key(seat) {
                    bail!(
                        "pool {name} lists seat {seat}, which is not in [seats]; add [seats.{seat}] or remove it from the pool"
                    );
                }
            }
        }
        Ok(config)
    }

    /// Read a config file.
    ///
    /// # Errors
    /// When the file cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let root = path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self::parse(&text, &root).with_context(|| format!("in {}", path.display()))
    }

    /// Find `andon.toml` in `start` or the nearest ancestor.
    #[must_use]
    pub fn find(start: &Path) -> Option<PathBuf> {
        start
            .ancestors()
            .map(|dir| dir.join(FILE_NAME))
            .find(|p| p.is_file())
    }

    /// The goal-id pattern, compiled.
    ///
    /// # Errors
    /// When the pattern does not compile (`parse` already checked it).
    pub fn goal_regex(&self) -> Result<Regex> {
        Regex::new(&self.project.goal_pattern).context("project.goal_pattern")
    }

    /// A configured path: `~` expands to the home directory, a relative path
    /// resolves against the config's directory.
    #[must_use]
    pub fn path(&self, raw: &str) -> PathBuf {
        expand(raw, &self.root)
    }

    /// The WIP limit for a seat.
    #[must_use]
    pub fn wip_for(&self, seat: &str) -> usize {
        self.seats
            .get(seat)
            .and_then(|s| s.wip)
            .unwrap_or(self.goal.wip_per_seat)
    }
}

/// Expand `~` and resolve relative paths against `root`.
#[must_use]
pub fn expand(raw: &str, root: &Path) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

/// Whether `label` matches `pattern`, where `*` matches any run of characters.
#[must_use]
pub fn wildcard(pattern: &str, label: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == label;
    }
    let mut rest = label;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if let Some(pos) = rest.find(part) {
            rest = &rest[pos + part.len()..];
        } else {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_match_prefixes_and_infixes() {
        assert!(wildcard("x*", "x-1a"));
        assert!(wildcard("x*", "x"));
        assert!(!wildcard("x*", "y-1a"));
        assert!(wildcard("*-1a", "x-1a"));
        assert!(wildcard("x*-1*", "x-1a"));
        assert!(wildcard("exact", "exact"));
        assert!(!wildcard("exact", "exactly"));
    }

    #[test]
    fn a_pool_naming_an_undefined_seat_is_refused() {
        let err = Config::parse("[pools.p]\nseats = [\"ghost\"]\n", Path::new("/"))
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            err.as_deref(),
            Some(
                "pool p lists seat ghost, which is not in [seats]; add [seats.ghost] or remove it from the pool"
            )
        );
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert!(Config::parse("[project]\nnmae = \"x\"\n", Path::new("/")).is_err());
    }
}
