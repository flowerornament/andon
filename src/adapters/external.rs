//! External adapters: the boundary a project's own bridge meets.
//!
//! andon's core adapters read general tools (jj, git, bd, GitHub, Ferry,
//! hail, harness transcripts). Anything particular to one project (its own
//! evidence store, its review protocol, its desk layout) is bridged by an
//! external adapter: a program, or a set of files, that the project owns and
//! that speaks andon's ledger format. andon defines the contract and checks
//! every line; it knows nothing else about the program.
//!
//! The contract, version 1:
//!
//! - Output is JSONL, one event per line, in the ledger's own schema (the
//!   same lines `andon ledger` prints; see `.design/` for the schema).
//! - `t` is required and is UTC (RFC 3339). `kind` is one of the ledger's
//!   kinds or `other:<name>`. `source` names the artifact the fact came from.
//! - An adapter states who acted (`actor`) and never resolves seats:
//!   a `seat` it sets is discarded, because seat identity is andon's join.
//! - The program reads; it must not write to the stores it reads.
//! - Exit 0. A non-zero exit, or a line that is not an event, is an error
//!   that names the adapter, so `board` and `check` exit 2.
//!
//! Every event an external adapter emits is marked: `attrs.adapter` is its
//! configured name and `source` is prefixed with it, so a reader of the
//! ledger or the board can always tell a core fact from a bridged one.

use anyhow::{Context, Result, bail};

use crate::adapters::{glob_all, run_tool_in};
use crate::config::{Config, ExternalConfig};
use crate::event::Event;
use crate::event::attr;

/// The version of the contract this andon reads.
pub const CONTRACT: u32 = 1;

/// Run one external adapter, or read its files.
///
/// # Errors
/// When the program fails, a file cannot be read, or a line is not an event.
pub fn collect(config: &Config, ec: &ExternalConfig) -> Result<Vec<Event>> {
    if ec.contract != CONTRACT {
        bail!(
            "external adapter {} uses contract {}, but this andon supports contract {CONTRACT}; upgrade one of them",
            ec.name,
            ec.contract
        );
    }
    let mut events = Vec::new();
    if let Some((program, args)) = ec.command.split_first() {
        let dir = config.path(ec.dir.as_deref().unwrap_or("."));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let fix = format!(
            "install {program}, or remove the external adapter {} from andon.toml",
            ec.name
        );
        let out = run_tool_in(&dir, program, &args, &fix)?;
        events.extend(parse(&out, &ec.name, program)?);
    }
    for pattern in &ec.files {
        let found = glob_all(&config.path(pattern))
            .with_context(|| format!("external adapter {}", ec.name))?;
        for path in found {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            events.extend(parse(&text, &ec.name, &path.display().to_string())?);
        }
    }
    if ec.command.is_empty() && ec.files.is_empty() {
        bail!(
            "external adapter {} has no command or files; set command or files",
            ec.name
        );
    }
    Ok(events)
}

/// Parse an adapter's output and mark every event as the adapter's.
///
/// # Errors
/// When a line is not an event; the error names the adapter and the line.
pub fn parse(text: &str, name: &str, origin: &str) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut event: Event = serde_json::from_str(line).with_context(|| {
            format!(
                "external adapter {name}: line {} of {origin} is not an andon event",
                i + 1
            )
        })?;
        event.upgrade();
        if let Some(a) = event.misshapen() {
            bail!(
                "external adapter {name}: line {} of {origin} has attrs.{} that is not {}; fix the adapter",
                i + 1,
                a.key,
                a.shape.as_str()
            );
        }
        // seat identity is andon's join, never an adapter's assertion
        event.seat = None;
        event.set_attr(attr::ADAPTER, name.to_owned());
        event.source = format!("adapter {name}: {}", event.source);
        events.push(event);
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Kind;

    #[test]
    fn events_are_marked_as_the_adapters_and_seats_are_not_trusted() {
        let out = concat!(
            r#"{"t":"2026-10-13T10:00:00Z","kind":"other:boundary_fix","actor":"x-1a","seat":"x-9z","source":"review census"}"#,
            "\n\n"
        );
        let events = parse(out, "x-review", "x-adapter").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.kind, Kind::Other("other:boundary_fix".into()));
        assert_eq!(e.seat, None);
        assert_eq!(e.actor.as_deref(), Some("x-1a"));
        assert_eq!(e.attr_str(attr::ADAPTER), Some("x-review"));
        assert_eq!(e.source, "adapter x-review: review census");
    }

    #[test]
    fn a_line_that_is_not_an_event_names_the_adapter() {
        let err = parse("{\"kind\":\"land\"}\n", "x-review", "x-adapter")
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            err.as_deref(),
            Some("external adapter x-review: line 1 of x-adapter is not an andon event")
        );
    }

    #[test]
    fn a_declared_key_of_the_wrong_shape_is_refused() {
        let line =
            r#"{"t":"2026-10-13T10:00:00Z","kind":"run","source":"s","attrs":{"recipe":["land"]}}"#;
        let err = parse(line, "x-review", "x-adapter")
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            err.as_deref(),
            Some(
                "external adapter x-review: line 1 of x-adapter has attrs.recipe that is not text; fix the adapter"
            )
        );
    }
}
