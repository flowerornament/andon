//! The goal schema (founding design §3): why, a done-when that can fail, an
//! budget, a pool, an owner. A goal's description carries the first four as
//! sections; the owner is set by a claim, never by hand.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use regex::Regex;

use crate::config::Config;

/// The first goal id in `text`. A pattern with a capture group yields the
/// group (`(?:Closes|Fixes) #(\d+)` yields `12`), else the whole match.
#[must_use]
pub fn find(pattern: &Regex, text: &str) -> Option<String> {
    pattern.captures(text).and_then(|c| capture(&c))
}

/// Every goal id in `text`, sorted and distinct.
#[must_use]
pub fn find_all(pattern: &Regex, text: &str) -> Vec<String> {
    let mut ids: Vec<String> = pattern
        .captures_iter(text)
        .filter_map(|c| capture(&c))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn capture(c: &regex::Captures<'_>) -> Option<String> {
    c.iter()
        .skip(1)
        .flatten()
        .next()
        .or_else(|| c.get(0))
        .map(|m| m.as_str().to_owned())
}

/// The goal a commit cites. By convention the subject ends with it
/// (`area: subject (goal-id)`), so the subject's last match wins; with
/// `project.goal_in = "message"`, the message's first match (`Closes #12`)
/// stands in when the subject has none.
#[must_use]
pub fn cited(pattern: &Regex, subject: &str, message: &str, config: &Config) -> Option<String> {
    candidates(pattern, subject, message, config)
        .into_iter()
        .next()
}

/// Every goal id a commit could be citing, most likely first: the subject's
/// matches from last to first, then (by configuration) the message's.
#[must_use]
pub fn candidates(pattern: &Regex, subject: &str, message: &str, config: &Config) -> Vec<String> {
    let mut out: Vec<String> = pattern
        .captures_iter(subject)
        .filter_map(|c| capture(&c))
        .collect();
    out.reverse();
    if config.project.goal_in == "message" {
        for c in pattern.captures_iter(message) {
            if let Some(id) = capture(&c)
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
    }
    out
}

/// A goal's text with the sections config keeps in tracker fields
/// (`[goal.fields]`) added, each where the description lacks it. A field
/// value that begins with its own label (`Done when: ...`) loses it.
#[must_use]
pub fn with_fields(
    description: &str,
    record: &serde_json::Value,
    fields: &BTreeMap<String, String>,
) -> String {
    let have = sections(description);
    let mut text = description.to_owned();
    for (name, field) in fields {
        if section(&have, name).is_some() {
            continue;
        }
        let Some(value) = record[field.as_str()]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        let value = value
            .strip_prefix(name.as_str())
            .and_then(|v| v.strip_prefix(':'))
            .map_or(value, str::trim);
        let _ = write!(text, "\n\n## {name}\n\n{value}\n");
    }
    text
}

/// A description's sections, keyed by heading, lowercased. A section starts at
/// a `## Heading` line or a `Heading:` line and runs to the next.
#[must_use]
pub fn sections(description: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in description.lines() {
        let trimmed = line.trim();
        let heading = trimmed
            .strip_prefix('#')
            .map(|h| h.trim_start_matches('#').trim().to_owned())
            .filter(|h| !h.is_empty())
            .or_else(|| {
                // `Done when: …` on one line: a heading with inline text
                let (head, _) = trimmed.split_once(':')?;
                (head.len() <= 24
                    && head.chars().next()?.is_ascii_uppercase()
                    && head
                        .chars()
                        .all(|c| c.is_ascii_alphabetic() || c == ' ' || c == '-'))
                .then(|| head.to_owned())
            });
        if let Some(head) = heading {
            let key = head.to_lowercase();
            let inline = trimmed
                .split_once(':')
                .filter(|_| !trimmed.starts_with('#'))
                .map(|(_, rest)| rest.trim().to_owned())
                .unwrap_or_default();
            out.insert(key.clone(), inline);
            current = Some(key);
            continue;
        }
        if let Some(key) = &current {
            let body = out.entry(key.clone()).or_default();
            if !body.is_empty() || !trimmed.is_empty() {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(trimmed);
            }
        }
    }
    for body in out.values_mut() {
        *body = body.trim().to_owned();
    }
    out
}

/// The section a config names, if present and not empty.
#[must_use]
pub fn section<'a>(sections: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    sections
        .get(&name.to_lowercase())
        .map(String::as_str)
        .filter(|s| !s.is_empty())
}

/// Why a goal is not a goal, each with its fix; empty when it is one.
#[must_use]
pub fn problems(description: &str, config: &Config) -> Vec<String> {
    let found = sections(description);
    let mut problems = Vec::new();
    for name in &config.goal.sections {
        if section(&found, name).is_none() {
            problems.push(format!("add a \"## {name}\" section"));
        }
    }
    if let Some(budget) = section(&found, &config.goal.budget)
        && crate::fold::board::parse_budget(budget).is_none()
    {
        problems.push(format!(
            "state the {} as a number of agent-hours, hours, days or runs",
            config.goal.budget
        ));
    }
    if config.goal.require_pool {
        match section(&found, "pool") {
            None => problems.push("add a \"## Pool\" section naming a configured pool".into()),
            Some(pool) if !config.pools.contains_key(pool.trim()) => problems.push(format!(
                "pool {:?} is not configured; use one of: {}",
                pool.trim(),
                config.pools.keys().cloned().collect::<Vec<_>>().join(", ")
            )),
            Some(_) => {}
        }
    }
    problems
}

/// Advice, not refusals: a done-when that names no runnable check, number or
/// limit may not be able to fail. A heuristic cannot be the reason a goal is
/// refused, so it warns.
#[must_use]
pub fn warnings(description: &str, config: &Config) -> Vec<String> {
    let found = sections(description);
    match section(&found, &config.goal.done_when) {
        Some(done) if cannot_fail(done, &config.goal.checkable) => vec![format!(
            "the {} section names no command, number or limit a check could fail on: \"{}\"",
            config.goal.done_when,
            first_words(done)
        )],
        _ => Vec::new(),
    }
}

/// A done-when can fail only when it names what a check observes: a command
/// in backticks, a measured value (a standalone number), or a check outcome
/// from `checkable`, as a whole word. Anything else ("the board is good",
/// "the team understands it", "landing a cleaner design") cannot fail. A
/// heuristic, so it is a floor, not a proof.
/// Limits: they count when a quantity follows them.
const LIMITS: &[&str] = &[
    "under",
    "below",
    "above",
    "within",
    "exceed",
    "exceeds",
    "more than",
    "fewer than",
    "at most",
    "at least",
    "no more than",
    "less than",
];
/// Words that are a quantity after a limit.
const QUANTITY: &[&str] = &[
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "a",
    "an", "half",
];

fn cannot_fail(done: &str, checkable: &[String]) -> bool {
    if done.contains('`') {
        return false;
    }
    let lower = done.to_lowercase();
    let words: Vec<String> = lower
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '(' | ')' | '"' | '\''))
        .map(|w| w.trim_end_matches(['.', '!', '?']).to_owned())
        .filter(|w| !w.is_empty())
        .collect();
    // a measured value: -0.5, 1e-6, 7.8, 95%, 117/117, 2s; not v2
    let measured = |w: &str| {
        let w = w
            .strip_prefix(['-', '+', '<', '>', '≤', '≥', '~'])
            .unwrap_or(w);
        let mut chars = w.chars().peekable();
        let starts = chars.peek().is_some_and(char::is_ascii_digit);
        let body: String = w
            .chars()
            .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '/'))
            .collect();
        let rest = &w[body.len()..];
        let exponent = rest.strip_prefix('e').map_or(rest, |e| {
            e.trim_start_matches('-')
                .trim_start_matches(|c: char| c.is_ascii_digit())
        });
        starts
            && (exponent.is_empty()
                || exponent == "%"
                || exponent.chars().all(|c| c.is_ascii_alphabetic()))
    };
    // a check word with its endings: pass, passes, passed, passing
    let inflected = |w: &str, stem: &str| {
        w == stem
            || w.strip_prefix(stem).is_some_and(|rest| {
                ["s", "es", "ed", "d", "ing", "e", "able", "ible"].contains(&rest)
            })
    };
    // a limit names a quantity: "under 2 s", "within one second", "at most
    // 1e-6"; "under consideration" names none
    let quantity_at = |i: usize| {
        words
            .get(i)
            .is_some_and(|w| measured(w) || QUANTITY.contains(&w.as_str()))
    };
    let limit = LIMITS.iter().any(|phrase| {
        let parts: Vec<&str> = phrase.split(' ').collect();
        words
            .windows(parts.len())
            .enumerate()
            .any(|(i, w)| w.iter().zip(&parts).all(|(a, b)| a == b) && quantity_at(i + parts.len()))
    });
    let padded = format!(" {} ", words.join(" "));
    let observes = limit
        || words.iter().any(|w| measured(w))
        || checkable
            .iter()
            .filter(|stem| !LIMITS.contains(&stem.as_str()))
            .any(|stem| {
                if stem.contains(' ') {
                    padded.contains(&format!(" {stem} "))
                } else {
                    words.iter().any(|w| inflected(w, stem))
                }
            });
    !observes
}

fn first_words(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().take(6).collect();
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const GOAL: &str = "## Why\n\nBecause.\n\n## Done when\n\n`cargo test` passes the study pins.\n\n## Budget\n\nTwo sessions (about 8 agent-hours).\n";

    #[test]
    fn a_capture_group_is_the_goal_id() {
        let closes = Regex::new(r"(?:[Cc]loses|[Ff]ixes) #(\d+)").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(find(&closes, "body\n\nCloses #12").as_deref(), Some("12"));
        let plain = Regex::new(r"\bandon-[a-z0-9]{3}\b").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(find(&plain, "x (andon-xa4)").as_deref(), Some("andon-xa4"));
        assert_eq!(
            find_all(&closes, "Fixes #3, closes #1, Fixes #3"),
            vec!["1", "3"]
        );
    }

    #[test]
    fn the_subjects_last_match_is_the_citation() {
        let pattern = Regex::new(r"\bandon-[a-z0-9]{3,6}\b").unwrap_or_else(|e| panic!("{e}"));
        let c = Config::default();
        assert_eq!(
            cited(&pattern, "andon-hook: wire it (andon-8ig)", "", &c).as_deref(),
            Some("andon-8ig")
        );
        assert_eq!(
            candidates(&pattern, "andon-hook: wire it (andon-8ig)", "", &c),
            vec!["andon-8ig".to_owned(), "andon-hook".to_owned()]
        );
    }

    #[test]
    fn sections_read_markdown_and_inline_headings() {
        let s = sections(GOAL);
        assert_eq!(section(&s, "Why"), Some("Because."));
        assert_eq!(
            section(&s, "budget"),
            Some("Two sessions (about 8 agent-hours).")
        );
        let inline = sections("Why: speed\nDone when: `just check` passes\nBudget: 2 agent-hours");
        assert_eq!(section(&inline, "done when"), Some("`just check` passes"));
    }

    #[test]
    fn a_complete_goal_has_no_problems() {
        assert_eq!(
            problems(GOAL, &Config::default()),
            [] as [std::string::String; 0]
        );
    }

    #[test]
    fn each_missing_part_names_its_fix() {
        let p = problems("## Why\n\nBecause.\n", &Config::default());
        assert_eq!(
            p,
            vec![
                "add a \"## Done when\" section".to_owned(),
                "add a \"## Budget\" section".to_owned()
            ]
        );
    }

    #[test]
    fn a_done_when_that_cannot_fail_is_advised_not_refused() {
        let goal = GOAL.replace(
            "`cargo test` passes the study pins.",
            "Improve the board layout",
        );
        assert_eq!(
            problems(&goal, &Config::default()),
            [] as [std::string::String; 0]
        );
        assert_eq!(
            warnings(&goal, &Config::default()),
            vec!["the Done when section names no command, number or limit a check could fail on: \"Improve the board layout\"".to_owned()]
        );
    }

    #[test]
    fn a_done_when_that_observes_nothing_draws_a_warning() {
        let vague = [
            "the board is good",
            "it works well",
            "works",
            "ensure correctness",
            "the scorecard is accurate",
            "the team understands it",
            "under consideration",
            "the latest design feels right",
            "it builds character",
            "ensure it runs smoothly",
            "everything is in the list of nice things",
            "the architecture is elegant and closer to ideal",
            "the design is better than v2",
            "the code is green and lovely",
            "the team is in the red mood",
        ];
        for done in vague {
            let goal = GOAL.replace("`cargo test` passes the study pins.", done);
            // advice, not a refusal
            assert!(
                problems(&goal, &Config::default()).is_empty(),
                "{done:?} refused"
            );
            assert!(
                !warnings(&goal, &Config::default()).is_empty(),
                "{done:?} passed silently"
            );
        }
    }

    #[test]
    fn a_done_when_naming_a_check_command_number_or_limit_passes_quietly() {
        let checkable = [
            "the board shows a row per seat",
            "p95 under 2 s",
            "a census exists with one row per site",
            "`just check` is green",
            "the scorecard reproduces 117 landings",
            "a commit citing no goal is refused",
            "117/117 landings join their session",
            "the hook prints at most 280 bytes",
            // the third acceptance round's false refusals
            "error is at most 1e-6",
            "drift stays below 1e-6",
            "the residual is -0.5",
            "the probe gives -0.1836 within 1e-6",
            "all tests are passing",
            "the build is reproducible",
            "the hook stays under one second",
            "bench admits a perf run",
            "no seat claims more than one goal",
            "the parser accepts every example in the spec",
            "zero clippy warnings",
            "no warnings",
        ];
        for done in checkable {
            let goal = GOAL.replace("`cargo test` passes the study pins.", done);
            assert!(
                warnings(&goal, &Config::default()).is_empty(),
                "{done:?} warned"
            );
        }
    }

    #[test]
    fn an_unbounded_budget_is_refused() {
        let goal = GOAL.replace("Two sessions (about 8 agent-hours).", "as long as it takes");
        assert_eq!(
            problems(&goal, &Config::default()),
            vec!["state the Budget as a number of agent-hours, hours, days or runs".to_owned()]
        );
    }

    #[test]
    fn pools_are_checked_when_required() {
        let config = Config::parse(
            "[goal]\nrequire_pool = true\n[seats.a]\n[pools.compiler]\nseats = [\"a\"]\n",
            Path::new("/"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            problems(&format!("{GOAL}\n## Pool\n\nruntime\n"), &config),
            vec!["pool \"runtime\" is not configured; use one of: compiler".to_owned()]
        );
        assert_eq!(
            problems(&format!("{GOAL}\n## Pool\n\ncompiler\n"), &config),
            [] as [std::string::String; 0]
        );
    }
}
