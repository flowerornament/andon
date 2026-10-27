//! The command line. Exit codes are part of the interface:
//!
//! | code | meaning                                                      |
//! |------|--------------------------------------------------------------|
//! | 0    | a surface rendered, or a check passed                        |
//! | 1    | a check refused; stdout names the fix in one line            |
//! | 2    | andon could not decide (no config, unreadable source); stdout |
//! |      | names what to repair in one line                             |

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::config::Config;
use crate::ledger::Ledger;
use crate::time::{Window, endpoint_in};
use crate::{adapters, fold, render};

/// The signal board for many agents on one repository, derived from the
/// artifacts they already write.
#[derive(Debug, Parser)]
#[command(
    name = "andon",
    version,
    about,
    after_help = "\
With no command, andon prints the board.

Examples:
  andon                        the board, now
  andon asof yesterday         the board at the end of yesterday
  andon scorecard 7d           the four gauges over the last seven days
  andon check commit @         may this commit land? (exit 1 names the fix)
  andon init > andon.toml      a starter config

Exit codes: 0 rendered or passed; 1 a check refused; 2 andon could not
decide (the line says what to repair). Times print in the local zone;
TZ=UTC andon ... prints UTC."
)]
pub struct Cli {
    /// The config file (default: andon.toml here or in the nearest ancestor).
    #[arg(
        long,
        global = true,
        value_name = "FILE",
        help_heading = "Global options"
    )]
    pub config: Option<PathBuf>,
    /// Read a kept ledger (from `andon ledger`) instead of the sources.
    #[arg(
        long,
        global = true,
        value_name = "FILE",
        help_heading = "Global options"
    )]
    pub ledger: Option<PathBuf>,
    /// Color: auto (on a terminal, unless `NO_COLOR` is set), always or never.
    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "WHEN",
        default_value_t = Color::Auto,
        help_heading = "Global options"
    )]
    pub color: Color,
    /// After the output, print each source's read time and event count to
    /// stderr.
    #[arg(long, global = true, help_heading = "Global options")]
    pub timings: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// When text output is painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Color {
    Auto,
    Always,
    Never,
}

/// How this invocation's text is shown: the reader's zone and terminal,
/// and `--color`.
fn display(cli: &Cli) -> render::Display {
    let auto = render::Display::local();
    render::Display {
        color: match cli.color {
            Color::Auto => auto.color,
            Color::Always => true,
            Color::Never => false,
        },
        ..auto
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// One row per seat now: goal, budget, lands, runs, verdict, flags.
    #[command(
        long_about = render::board_help(),
        after_help = "Examples:\n  andon board\n  andon board --seat proj-1a\n  andon board --all --json | jq '.seats[].flags'"
    )]
    Board {
        /// Only this seat's row.
        #[arg(long)]
        seat: Option<String>,
        /// Every seat, the quiet ones too.
        #[arg(long)]
        all: bool,
        /// Print the board as JSON (board schema 4, every seat), not text.
        #[arg(long)]
        json: bool,
    },
    /// Who holds which goal: budget spent and time since the claim.
    ///
    /// The goals someone holds, who holds each, budget spent of allowed,
    /// and how long since the claim; then how many open goals no one holds.
    #[command(
        after_help = "Examples:\n  andon goals\n  andon goals --open\n  andon goals --seat proj-1a\n  andon goals --all --json | jq '.goals[].id'"
    )]
    Goals {
        /// Only the goals this seat (or identity) holds.
        #[arg(long)]
        seat: Option<String>,
        /// The open goals no one holds too.
        #[arg(long)]
        open: bool,
        /// Open and closed goals: every goal.
        #[arg(long)]
        all: bool,
        /// The list as JSON (schema 2).
        #[arg(long)]
        json: bool,
    },
    /// The roster: each seat's names, workspace, session and joins.
    ///
    /// Each configured seat with the names that join to it, where it works,
    /// its live session and what joined to it in the window; then the names
    /// the sources recorded that join no seat, with the fix.
    #[command(after_help = "Examples:\n  andon seats\n  andon seats --json")]
    Seats {
        /// The roster as JSON (schema 1).
        #[arg(long)]
        json: bool,
    },
    /// The board as it stood at an instant, replayed from the ledger.
    #[command(
        after_help = "Examples:\n  andon asof yesterday         the end of yesterday\n  andon asof 2026-10-24        the end of that day\n  andon asof 2026-10-24T09:30  local time\n  andon asof -6h               six hours ago"
    )]
    Asof {
        /// The instant: a date (its end), a time (local, or with Z or an
        /// offset), now, today, yesterday, -6h or -2d.
        #[arg(allow_hyphen_values = true)]
        when: String,
        /// Only this seat's row.
        #[arg(long)]
        seat: Option<String>,
        /// Every seat, the quiet ones too.
        #[arg(long)]
        all: bool,
        /// Print the board as JSON (board schema 4, every seat), not text.
        #[arg(long)]
        json: bool,
    },
    /// The four laws' gauges per seat over a window, today by default.
    #[command(
        long_about = render::scorecard_help(),
        after_help = "Examples:\n  andon scorecard                      today so far\n  andon scorecard yesterday\n  andon scorecard 7d                   the last seven days\n  andon scorecard 2026-10-11..2026-10-12"
    )]
    Scorecard {
        /// The window: a day (2026-10-24, today, yesterday), FROM..TO,
        /// FROM.., or the last 6h or 2d. Dates are local days.
        #[arg(default_value = "today", allow_hyphen_values = true)]
        window: String,
        /// Print the scorecard as JSON (scorecard schema 2), not text.
        #[arg(long)]
        json: bool,
    },
    /// A refusal at a choke point: exit 1 naming the fix, or exit 0.
    #[command(
        after_help = "Examples:\n  andon check commit @                in the land verb, before it moves trunk\n  andon check goal proj-abc           when a goal is filed\n  andon check message ask --goal proj-abc"
    )]
    Check {
        #[command(subcommand)]
        what: CheckCommand,
    },
    /// The seat's row and what changed since its last turn, for a hook.
    ///
    /// Reads the hook's JSON payload on stdin and never fails a session: a
    /// problem is one line in the agent's context, and exit 0.
    #[command(
        after_help = "Wire it as a Claude Code UserPromptSubmit or SessionStart hook, or a Codex\nhook, with the command `andon hook`. Check the wiring with:\n  echo '<payload>' | andon hook --check"
    )]
    Hook {
        /// Check the wiring instead: report the config, seat, workspace and
        /// transcript this payload resolves to, and exit 1 naming the fix
        /// when one is missing.
        #[arg(long)]
        check: bool,
    },
    /// Print the event ledger as JSONL, for a project to keep and replay.
    #[command(
        after_help = "Examples:\n  andon ledger > ledger.jsonl\n  andon --ledger ledger.jsonl asof 2026-10-12\n  andon ledger --since -1d"
    )]
    Ledger {
        /// Only events at or after this instant or date (local), or -6h.
        #[arg(long, allow_hyphen_values = true)]
        since: Option<String>,
    },
    /// What each source read, and the hook's wiring; exit 1 names the fix.
    ///
    /// For each configured source: its events, the span it covers, its gaps
    /// and its time, or its whole error. Then the hook's wiring, as `hook
    /// --check` reports it. Exits 1, naming the fix, when a source cannot be
    /// read.
    #[command(after_help = "Example:\n  andon doctor")]
    Doctor,
    /// Print an andon.toml for this repository, from what it finds here.
    ///
    /// It detects the name, jj or git and the trunk, bd's prefix, the jj
    /// workspaces as seats, and the session, presence and hail sources.
    /// It writes nothing: redirect it into place.
    #[command(after_help = "Example:\n  andon init > andon.toml")]
    Init,
}

#[derive(Debug, Subcommand)]
pub enum CheckCommand {
    /// The commit cites a goal its seat has claimed, within the seat's WIP.
    Commit {
        /// A revision: a jj revset naming one commit (`@`, a change id), or
        /// a git revision with [adapters.git].
        rev: String,
        /// The committing seat, when the commit's trailers do not name it.
        #[arg(long)]
        seat: Option<String>,
    },
    /// The goal has its sections, a done-when that can fail, a finite
    /// budget, and room in its pool.
    Goal {
        /// The goal's tracker id, or `-` to read a description on stdin.
        id: String,
    },
    /// A decision kind names the goal it serves.
    Message {
        /// The message's kind (`ask`); `[messages] decision_kinds` must name
        /// a goal.
        kind: String,
        /// The goal the message serves.
        #[arg(long)]
        goal: Option<String>,
        /// The message's headline and body, searched for a goal id.
        #[arg(long, default_value = "")]
        text: String,
    },
}

/// Run the command line; never panics, always returns an exit code.
#[must_use]
pub fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // help and version print, for the hook as for any command
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error.exit()
        }
        // a hook must never fail a session, not even on a bad command line
        Err(_) if std::env::args().nth(1).as_deref() == Some("hook") => return ExitCode::SUCCESS,
        Err(error) => error.exit(),
    };
    // progress and timings go to stderr only, and the line only to a
    // terminal (never into a hook's output, whose stderr a harness may show)
    let hook = matches!(cli.command, Some(Command::Hook { .. }));
    let line = !hook
        && (io::IsTerminal::is_terminal(&io::stderr())
            || std::env::var_os("ANDON_PROGRESS").is_some_and(|v| v == "1"));
    adapters::progress::configure(adapters::progress::Options {
        line,
        timings: cli.timings,
    });
    let mut stdout = io::stdout().lock();
    let code = match run(&cli, &mut stdout) {
        Ok(code) => code,
        Err(error) => {
            // one line on stdout (stdout is the API), the whole chain on it,
            // so the cause that names the fix is not lost
            let _ = writeln!(stdout, "andon: cannot decide: {error:#}");
            ExitCode::from(2)
        }
    };
    if cli.timings {
        let _ = stdout.flush();
        eprint!(
            "{}",
            adapters::progress::timings_text(&adapters::progress::take_timings())
        );
    }
    code
}

/// Run a parsed command, writing its output to `out`.
///
/// # Errors
/// When andon cannot decide: missing config, unreadable source, bad input.
pub fn run(cli: &Cli, out: &mut dyn Write) -> Result<ExitCode> {
    let shown = |all: bool| render::Display {
        all,
        ..display(cli)
    };
    // no command is the board, now
    let board = Command::Board {
        seat: None,
        all: false,
        json: false,
    };
    match cli.command.as_ref().unwrap_or(&board) {
        Command::Hook { check } => hook(cli, *check, out),
        Command::Check { what } => {
            let now = clock();
            let config = load_config(cli, now)?;
            let ledger = load_ledger(cli, &config)?;
            let answer = check(what, &config, &ledger, now)?;
            writeln!(out, "{}", answer.line())?;
            Ok(if answer.refused() {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            })
        }
        Command::Init => init(out),
        goals_ @ Command::Goals { .. } => goals(cli, out, goals_),
        Command::Seats { json } => seats(cli, out, *json),
        Command::Doctor => doctor(cli, out),
        Command::Ledger { since } => {
            let config = load_config(cli, clock())?;
            let ledger = load_ledger(cli, &config)?;
            let ledger = match since {
                Some(since) => {
                    let from = endpoint_in(since, clock(), &TimeZone::system(), false)?;
                    Ledger::new(
                        ledger
                            .events()
                            .iter()
                            .filter(|e| e.t >= from)
                            .cloned()
                            .collect(),
                    )
                }
                None => ledger,
            };
            ledger.write(out)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Board { seat, all, json } => {
            // the board's instant is read before the sources, so every source
            // read after it covers it; the clock is read nowhere in the fold
            let now = clock();
            let config = load_config(cli, now)?;
            let ledger = load_ledger(cli, &config)?;
            show_board(
                out,
                &ledger,
                &config,
                now,
                seat.as_deref(),
                &shown(*all),
                *json,
            )
        }
        Command::Asof {
            when,
            seat,
            all,
            json,
        } => {
            let now = clock();
            let config = load_config(cli, now)?;
            let ledger = load_ledger(cli, &config)?;
            // a date is its end; a moment not yet come is now
            let asof = endpoint_in(when, now, &TimeZone::system(), true)?.min(now);
            show_board(
                out,
                &ledger,
                &config,
                asof,
                seat.as_deref(),
                &shown(*all),
                *json,
            )
        }
        Command::Scorecard { window, json } => {
            // `now` closes an open window; read before the sources, which then
            // cover it; the only clock read
            let now = clock();
            let config = load_config(cli, now)?;
            let ledger = load_ledger(cli, &config)?;
            let window = Window::parse_in(window, now, &TimeZone::system())?;
            let card = fold::scorecard::scorecard(&ledger, &config, window);
            if *json {
                serde_json::to_writer_pretty(&mut *out, &card)?;
                out.write_all(b"\n")?;
            } else {
                out.write_all(render::scorecard_in(&card, &display(cli)).as_bytes())?;
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// `andon hook`: the seat's row and what changed, or with `--check` the
/// wiring; a hook never fails the session.
fn hook(cli: &Cli, check: bool, out: &mut dyn Write) -> Result<ExitCode> {
    let mut input = String::new();
    let _ = io::Read::read_to_string(&mut io::stdin(), &mut input);
    if check {
        let (report, ok) = crate::hook::check(&sources(cli), &input, clock());
        out.write_all(report.as_bytes())?;
        return Ok(if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        });
    }
    match crate::hook::run(&sources(cli), &input, clock()) {
        // a hook must never break the session: a write error, like any
        // other, is explained on stderr and the hook exits 0
        Ok(text) => {
            if let Err(error) = out.write_all(text.as_bytes()) {
                eprintln!("andon hook: {error}");
            }
        }
        // say so, in one line the agent sees, but never fail the
        // session (an exit of 2 would block the prompt)
        Err(error) => {
            let _ = writeln!(out, "andon: hook cannot decide: {error:#}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn check(
    what: &CheckCommand,
    config: &Config,
    ledger: &Ledger,
    now: Timestamp,
) -> Result<crate::check::Answer> {
    use crate::check;
    // a refusal needs the facts it refuses on: without the tracker covered,
    // a check that reads goals cannot decide (exit 2), it does not guess
    let tracker = || {
        crate::fold::coverage::Coverage::new(ledger)
            .check_state(crate::fold::gauge::TRACKER, None, now)
            .map_or(Ok(()), |why| {
                Err(anyhow!(
                    "the tracker is not covered ({why}); configure a tracker adapter"
                ))
            })
    };
    // without a goal schema, commits and goals have nothing to be checked
    // against: the check passes and says so, without reading a source
    if !config.goal_schema() {
        let empty = crate::commit::Commit::default();
        match what {
            CheckCommand::Commit { rev, .. } => {
                let commit = crate::commit::Commit {
                    id: rev.clone(),
                    ..empty
                };
                return Ok(check::commit(&commit, ledger, config, now));
            }
            CheckCommand::Goal { id } => return Ok(check::goal(id, "", ledger, config, now)),
            CheckCommand::Message { .. } => {}
        }
    }
    if matches!(what, CheckCommand::Commit { .. })
        || matches!(what, CheckCommand::Message { goal: Some(_), .. })
    {
        tracker()?;
    }
    Ok(match what {
        CheckCommand::Commit { rev, seat } => {
            let mut commit = match (&config.adapters.jj, &config.adapters.git) {
                (Some(jc), _) => {
                    adapters::jj::describe(config, jc, rev, &std::env::current_dir()?)?
                }
                (None, Some(gc)) => adapters::git::describe(config, gc, rev)?,
                (None, None) => {
                    return Err(anyhow!(
                        "check commit reads the commit through [adapters.jj] or [adapters.git]; configure one"
                    ));
                }
            };
            if seat.is_some() {
                commit.seat.clone_from(seat);
            }
            if commit.seat.is_none() {
                let here = std::env::current_dir()?;
                commit.seat = crate::hook::seat_of(&here, config);
            }
            commit.seat = commit
                .seat
                .map(|s| crate::seat::Resolver::new(config).label(&s).unwrap_or(s));
            check::commit(&commit, ledger, config, now)
        }
        CheckCommand::Goal { id } => {
            let description = if id == "-" {
                let mut text = String::new();
                io::Read::read_to_string(&mut io::stdin(), &mut text)?;
                text
            } else {
                let bc = config
                    .adapters
                    .bd
                    .as_ref()
                    .ok_or_else(|| anyhow!("check goal reads goals through [adapters.bd]; configure it, or pipe the description to: andon check goal -"))?;
                adapters::bd::description(config, bc, id)?
                    .ok_or_else(|| anyhow!("the tracker has no goal {id}"))?
            };
            let name = if id == "-" {
                "the description"
            } else {
                id.as_str()
            };
            check::goal(name, &description, ledger, config, now)
        }
        CheckCommand::Message { kind, goal, text } => {
            check::message(kind, goal.as_deref(), text, ledger, config, now)
        }
    })
}

fn show_board(
    out: &mut dyn Write,
    ledger: &Ledger,
    config: &Config,
    asof: Timestamp,
    seat: Option<&str>,
    display: &render::Display,
    json: bool,
) -> Result<ExitCode> {
    let mut board = fold::board::board(ledger, config, asof);
    if let Some(seat) = seat {
        board.seats.retain(|r| r.seat == seat);
        if board.seats.is_empty() {
            return Err(anyhow!(
                "no seat {seat} on the board; add it under [seats] in andon.toml"
            ));
        }
    }
    if json {
        serde_json::to_writer_pretty(&mut *out, &board)?;
        out.write_all(b"\n")?;
    } else {
        let display = render::Display {
            // a seat asked for by name is shown, quiet or not
            all: display.all || seat.is_some(),
            ..display.clone()
        };
        out.write_all(render::board_in(&board, &display).as_bytes())?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Where the hook reads: `--config` and `--ledger`.
fn sources(cli: &Cli) -> crate::hook::Sources<'_> {
    crate::hook::Sources {
        config: cli.config.as_deref(),
        ledger: cli.ledger.as_deref(),
    }
}

/// The clock, read where a command begins: the capture instant every
/// adapter reads to, and every surface renders at.
#[allow(
    clippy::disallowed_methods,
    reason = "time enters here, at capture, and nowhere in a fold"
)]
fn clock() -> Timestamp {
    Timestamp::now()
}

/// The config named by `--config`, or the nearest `andon.toml`.
///
/// # Errors
/// When no config exists, naming the fix.
pub fn load_config(cli: &Cli, capture: Timestamp) -> Result<Config> {
    let path = if let Some(path) = &cli.config {
        path.clone()
    } else {
        let here = std::env::current_dir().context("no working directory")?;
        Config::find(&here).ok_or_else(|| {
            anyhow!("no andon.toml here or above; create one: andon init > andon.toml")
        })?
    };
    let mut config = Config::load(&path)?;
    config.capture = Some(capture);
    Ok(config)
}

/// `andon init`: a config for the repository here.
fn init(out: &mut dyn Write) -> Result<ExitCode> {
    let here = std::env::current_dir().context("no working directory")?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let found = crate::init::detect(&here, home.as_deref());
    out.write_all(crate::init::config(&found).as_bytes())?;
    Ok(ExitCode::SUCCESS)
}

/// `andon goals`.
fn goals(cli: &Cli, out: &mut dyn Write, command: &Command) -> Result<ExitCode> {
    let Command::Goals {
        seat,
        open,
        all,
        json,
    } = command
    else {
        unreachable!("goals runs only for `andon goals`")
    };
    let (open, all, json) = (*open, *all, *json);
    let ask = &fold::goals::Ask {
        open,
        all,
        seat: seat.as_deref(),
    };
    let now = clock();
    let config = load_config(cli, now)?;
    let ledger = load_ledger(cli, &config)?;
    let list = fold::goals::goals(&ledger, &config, now, ask);
    if json {
        writeln!(out, "{}", serde_json::to_string(&list)?)?;
    } else {
        out.write_all(crate::listing::goals(&list, ask, &display(cli)).as_bytes())?;
    }
    Ok(ExitCode::SUCCESS)
}

/// `andon seats`.
fn seats(cli: &Cli, out: &mut dyn Write, json: bool) -> Result<ExitCode> {
    let now = clock();
    let config = load_config(cli, now)?;
    let ledger = load_ledger(cli, &config)?;
    let roster = fold::seats::roster(&ledger, &config, now);
    if json {
        writeln!(out, "{}", serde_json::to_string(&roster)?)?;
    } else {
        out.write_all(crate::listing::seats(&roster, &display(cli)).as_bytes())?;
    }
    Ok(ExitCode::SUCCESS)
}

/// `andon doctor`.
fn doctor(cli: &Cli, out: &mut dyn Write) -> Result<ExitCode> {
    let now = clock();
    let here = std::env::current_dir().context("no working directory")?;
    let path = match &cli.config {
        Some(p) => p.clone(),
        None => Config::find(&here).ok_or_else(|| {
            anyhow!("no andon.toml here or above; create one: andon init > andon.toml")
        })?,
    };
    let config = load_config(cli, now)?;
    let (report, ok) = crate::doctor::doctor(&path, &config, &here, now, &display(cli));
    out.write_all(report.as_bytes())?;
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// The ledger: read from `--ledger`, else collected from the sources.
///
/// # Errors
/// When the kept ledger or a source cannot be read.
pub fn load_ledger(cli: &Cli, config: &Config) -> Result<Ledger> {
    match &cli.ledger {
        Some(path) => Ledger::open(path),
        None => adapters::collect(config),
    }
}
