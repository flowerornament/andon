//! The command line. Exit codes are part of the interface:
//!
//! | code | meaning                                                      |
//! |------|--------------------------------------------------------------|
//! | 0    | a surface rendered, or a check passed                        |
//! | 1    | a check refused; stdout names the fix in one line            |
//! | 2    | andon could not decide (no config, unreadable source); stdout |
//! |      | names what to repair in one line                             |

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use jiff::Timestamp;

use crate::config::{self, Config};
use crate::ledger::Ledger;
use crate::time::{Window, parse_instant};
use crate::{adapters, fold, render};

/// The signal board for many agents on one repository, derived from the
/// artifacts they already write.
#[derive(Debug, Parser)]
#[command(name = "andon", version, about)]
pub struct Cli {
    /// The config file (default: andon.toml here or in the nearest ancestor).
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Read a kept ledger (from `andon ledger`) instead of the sources.
    #[arg(long, global = true, value_name = "FILE")]
    pub ledger: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// The four gauges per seat over a window: DAY, FROM..TO or FROM..
    Scorecard {
        window: String,
        #[arg(long)]
        json: bool,
    },
    /// One row per seat: goal, budget spent, landings, runs, verdict, flags.
    ///
    /// Budget: the finite effort a goal's `## Budget` section allows (agent-hours, hours, days or runs); the board shows it as spent/allowed and flags a goal over it.
    Board {
        /// Only this seat's row.
        #[arg(long)]
        seat: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// The board as it stood at an instant (a date means its end).
    Asof {
        when: String,
        #[arg(long)]
        seat: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print the ledger as JSONL, for a project to keep.
    Ledger {
        /// Only events at or after this instant or date.
        #[arg(long)]
        since: Option<String>,
    },
    /// Read a harness hook's JSON on stdin; print the seat's row and what
    /// changed since its last turn. Never fails a session.
    Hook {
        /// Check the wiring instead: report the config, seat, workspace and
        /// transcript this payload resolves to, and exit 1 naming the fix
        /// when one is missing.
        #[arg(long)]
        check: bool,
    },
    /// A refusal for a choke point: exit 1 and name the fix, or exit 0.
    Check {
        #[command(subcommand)]
        what: CheckCommand,
    },
    /// Print a starter andon.toml (redirect it into place).
    Init,
}

#[derive(Debug, Subcommand)]
pub enum CheckCommand {
    /// The commit cites a goal its seat has claimed, within the seat's WIP.
    Commit {
        /// A revision (jj revset naming one commit).
        rev: String,
        /// The committing seat, when the commit's trailers do not name it.
        #[arg(long)]
        seat: Option<String>,
    },
    /// The goal has its sections, a done-when that can fail, a finite
    /// budget, and room in its pool. `-` reads a description on stdin.
    Goal { id: String },
    /// A decision kind names the goal it serves.
    Message {
        kind: String,
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
        // a hook must never fail a session, not even on a bad command line
        Err(_) if std::env::args().nth(1).as_deref() == Some("hook") => return ExitCode::SUCCESS,
        Err(error) => error.exit(),
    };
    let mut stdout = io::stdout().lock();
    match run(&cli, &mut stdout) {
        Ok(code) => code,
        Err(error) => {
            // one line on stdout (stdout is the API), the whole chain on it,
            // so the cause that names the fix is not lost
            let _ = writeln!(stdout, "andon: cannot decide: {error:#}");
            ExitCode::from(2)
        }
    }
}

/// Run a parsed command, writing its output to `out`.
///
/// # Errors
/// When andon cannot decide: missing config, unreadable source, bad input.
pub fn run(cli: &Cli, out: &mut dyn Write) -> Result<ExitCode> {
    match &cli.command {
        Command::Hook { check } => {
            let mut input = String::new();
            let _ = io::Read::read_to_string(&mut io::stdin(), &mut input);
            if *check {
                let (report, ok) = hook_check(cli, &input, Timestamp::now());
                out.write_all(report.as_bytes())?;
                return Ok(if ok {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                });
            }
            match hook(cli, &input, Timestamp::now()) {
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
        Command::Check { what } => {
            let now = Timestamp::now();
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
        Command::Init => {
            out.write_all(config::STARTER.as_bytes())?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Ledger { since } => {
            let config = load_config(cli, Timestamp::now())?;
            let ledger = load_ledger(cli, &config)?;
            let ledger = match since {
                Some(since) => {
                    let from = Window::parse(&format!("{since}.."), Timestamp::MAX)?.from;
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
        Command::Board { seat, json } => {
            // the board's instant is read before the sources, so every source
            // read after it covers it; the clock is read nowhere in the fold
            let now = Timestamp::now();
            let config = load_config(cli, now)?;
            let ledger = load_ledger(cli, &config)?;
            show_board(out, &ledger, &config, now, seat.as_deref(), *json)
        }
        Command::Asof { when, seat, json } => {
            let config = load_config(cli, Timestamp::now())?;
            let ledger = load_ledger(cli, &config)?;
            let asof = instant_or_day_end(when)?;
            show_board(out, &ledger, &config, asof, seat.as_deref(), *json)
        }
        Command::Scorecard { window, json } => {
            // `now` closes an open window; read before the sources, which then
            // cover it; the only clock read
            let now = Timestamp::now();
            let config = load_config(cli, now)?;
            let ledger = load_ledger(cli, &config)?;
            let window = Window::parse(window, now)?;
            let card = fold::scorecard::scorecard(&ledger, &config, window);
            if *json {
                serde_json::to_writer_pretty(&mut *out, &card)?;
                out.write_all(b"\n")?;
            } else {
                out.write_all(render::scorecard(&card).as_bytes())?;
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// `andon hook --check`: what a payload resolves to, line by line, and
/// whether the hook can work with it.
fn hook_check(cli: &Cli, input: &str, now: Timestamp) -> (String, bool) {
    let mut report = String::new();
    let mut line = |ok: bool, what: &str, detail: String| {
        let _ = writeln!(
            report,
            "{} {what}: {detail}",
            if ok { "ok     " } else { "refused" }
        );
        ok
    };
    let payload = crate::hook::Payload::parse(input);
    if let Some(problem) = &payload.problem {
        line(
            false,
            "payload",
            format!("{problem}; pipe the harness's hook JSON to andon hook --check"),
        );
        return (report, false);
    }
    line(true, "payload", format!("{} event", payload.event));
    let cwd = payload
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let path = cli.config.clone().or_else(|| Config::find(&cwd));
    let Some(path) = path else {
        line(
            false,
            "config",
            format!(
                "no andon.toml in {} or above; add one, or pass --config",
                cwd.display()
            ),
        );
        return (report, false);
    };
    let config = match Config::load(&path) {
        Ok(mut c) => {
            c.capture = Some(now);
            line(true, "config", path.display().to_string());
            c
        }
        Err(e) => {
            line(false, "config", format!("{e:#}"));
            return (report, false);
        }
    };
    let Some(seat) = crate::hook::seat_of(&cwd, &config) else {
        line(
            false,
            "seat",
            format!(
                "{} belongs to no seat; add it under [seats] (cwd or prefixes)",
                cwd.display()
            ),
        );
        return (report, false);
    };
    let root = crate::hook::seat_root(&cwd, &seat);
    line(
        true,
        "seat",
        format!("{seat} (workspace {})", root.display()),
    );
    let ok = check_transcript_and_row(cli, &payload, &config, &seat, &root, now, &mut line);
    (report, ok)
}

/// The rest of `hook --check`: the transcript, then the row itself.
fn check_transcript_and_row(
    cli: &Cli,
    payload: &crate::hook::Payload,
    config: &Config,
    seat: &str,
    root: &Path,
    now: Timestamp,
    line: &mut dyn FnMut(bool, &str, String) -> bool,
) -> bool {
    let mut ok = true;
    match (
        &payload.transcript,
        payload
            .transcript
            .as_deref()
            .and_then(crate::hook::last_turn),
    ) {
        (Some(p), Some(t)) => {
            line(
                true,
                "transcript",
                format!("{} (last turn {})", p.display(), render::short_time(t)),
            );
        }
        (Some(p), None) => {
            ok &= line(
                false,
                "transcript",
                format!(
                    "cannot read a last turn from {}; the hook cannot tell what changed",
                    p.display()
                ),
            );
        }
        (None, _) if payload.event == "SessionStart" => {
            line(
                true,
                "transcript",
                "none needed at session start".to_owned(),
            );
        }
        (None, _) => {
            ok &= line(
                false,
                "transcript",
                "the payload names no transcript_path; the hook cannot tell what changed"
                    .to_owned(),
            );
        }
    }
    let ledger = match &cli.ledger {
        Some(p) => read_ledger(p),
        None => adapters::collect_for_seat(config, seat, root, now),
    };
    match ledger {
        Ok(ledger) => {
            let board = fold::board::board(&ledger, config, now);
            let row = board.seats.iter().find(|r| r.seat == seat);
            match row {
                Some(r) => {
                    line(true, "row", crate::hook::line(r, now));
                }
                None => {
                    ok &= line(
                        false,
                        "row",
                        format!("{seat} has no row; add it under [seats]"),
                    );
                }
            }
        }
        Err(e) => {
            ok &= line(false, "sources", format!("{e:#}"));
        }
    }
    ok
}

fn hook(cli: &Cli, input: &str, now: Timestamp) -> Result<String> {
    let payload = crate::hook::Payload::parse(input);
    if let Some(problem) = &payload.problem {
        return Err(anyhow!(
            "{problem}; the harness hook should pipe its JSON payload to `andon hook`"
        ));
    }
    let cwd = match &payload.cwd {
        Some(cwd) => cwd.clone(),
        None => std::env::current_dir()?,
    };
    let path = match &cli.config {
        Some(p) => p.clone(),
        None => match Config::find(&cwd) {
            Some(p) => p,
            None => return Ok(String::new()),
        },
    };
    let mut config = Config::load(&path)?;
    config.capture = Some(now);
    let Some(seat) = crate::hook::seat_of(&cwd, &config) else {
        return Ok(String::new());
    };
    // a kept ledger when given; else only what this seat's row needs
    let ledger = if let Some(path) = &cli.ledger {
        read_ledger(path)?
    } else {
        let root = crate::hook::seat_root(&cwd, &seat);
        adapters::collect_for_seat(&config, &seat, &root, now)?
    };
    let since = payload
        .transcript
        .as_deref()
        .and_then(crate::hook::last_turn);
    let mut text = crate::hook::render(&ledger, &config, &seat, &payload.event, since, now);
    // on a prompt, a transcript andon cannot read means it cannot tell what
    // changed: say so rather than look like "nothing changed"
    if payload.event != "SessionStart" && since.is_none() {
        let why = match &payload.transcript {
            None => "the payload names no transcript_path".to_owned(),
            Some(p) => format!("cannot read a last turn from {}", p.display()),
        };
        let _ = writeln!(
            text,
            "andon: hook cannot tell what changed since the last turn: {why}; check the wiring: andon hook --check"
        );
    }
    Ok(text)
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
            .check_state(&["create", "claim", "close"], None, now)
            .map_or(Ok(()), |why| {
                Err(anyhow!(
                    "the tracker is not covered ({why}); configure a tracker adapter"
                ))
            })
    };
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
        out.write_all(render::board(&board).as_bytes())?;
    }
    Ok(ExitCode::SUCCESS)
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

/// The ledger: read from `--ledger`, else collected from the sources.
///
/// # Errors
/// When the kept ledger or a source cannot be read.
pub fn load_ledger(cli: &Cli, config: &Config) -> Result<Ledger> {
    match &cli.ledger {
        Some(path) => read_ledger(path),
        None => adapters::collect(config),
    }
}

fn read_ledger(path: &Path) -> Result<Ledger> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("cannot read ledger {}", path.display()))?;
    Ledger::read(io::BufReader::new(file))
}

/// Parse an instant or a date (a date means its end, for `asof`).
///
/// # Errors
/// When the text is neither.
pub fn instant_or_day_end(text: &str) -> Result<Timestamp> {
    if let Ok(t) = parse_instant(text) {
        return Ok(t);
    }
    Ok(Window::parse(text, Timestamp::MAX)?.to)
}
