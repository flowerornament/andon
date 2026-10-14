//! messenger: hail. Reads the state directory (`~/.local/state/hail`).
//!
//! A message is a file `seats/<label>/{new,cur,owed}/<id>.<state>.md`: header
//! lines (`from`, `to`, `kind`, `id`, `time`, `re`, `ask` or `headline`), a
//! blank line, then the body. The directory it sits in names a recipient;
//! `pending/` holds the sender's copy and names none. Goal ids are found in
//! the headline and body with the project's goal pattern.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use regex::Regex;
use serde_json::Value;

use crate::adapters::Reads;
use crate::config::{Config, HailConfig, wildcard};
use crate::event::{Event, Kind};
use crate::time::parse_instant;

#[derive(Debug, Default)]
struct Message {
    headers: BTreeMap<String, String>,
    body: String,
    recipients: Vec<String>,
    source: String,
    /// The file's modification time, for the year a bare id leaves out.
    modified: Option<jiff::Timestamp>,
}

/// Read every message whose sender or a recipient matches the labels.
///
/// # Errors
/// When the goal pattern does not compile.
pub fn collect(config: &Config, hc: &HailConfig) -> Result<Vec<Event>> {
    let root = config.path(&hc.state).join("seats");
    let since = hc.since.as_deref().map(parse_instant).transpose()?;
    let goal = config.goal_regex()?;
    let keep = |l: &str| hc.labels.is_empty() || hc.labels.iter().any(|p| wildcard(p, l));
    let mut messages: BTreeMap<String, Message> = BTreeMap::new();
    let now = config.capture();
    // every listing is recorded: a directory that cannot be listed held
    // messages to and from unknown seats, a gap for every seat
    let mut reads = Reads::default();
    if !root.try_exists().unwrap_or(true) {
        // no seats directory: nothing was read, so nothing is covered
        return Ok(Vec::new());
    }
    for seat_dir in reads.list(&root) {
        let label = label(&seat_dir.file_name().unwrap_or_default().to_string_lossy());
        for state in ["new", "cur", "owed", "pending"] {
            let files = reads.list(&seat_dir.join(state));
            let seat_kept = keep(&label);
            for file in files {
                let name = file
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let Some(id) = message_id(&name) else {
                    continue;
                };
                if let Some(since) = since {
                    let modified = std::fs::metadata(&file)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| jiff::Timestamp::try_from(t).ok());
                    if modified.is_some_and(|m| m < since) {
                        continue;
                    }
                }
                // in another project's seat, a message matters only if one of
                // ours sent it: read its headers before the rest
                if !seat_kept && !messages.contains_key(id) {
                    let from = header_from(&file);
                    if !from.as_deref().is_some_and(keep) {
                        continue;
                    }
                }
                let m = messages.entry(id.to_owned()).or_default();
                if m.source.is_empty() || state != "pending" {
                    if let Err(e) = read_copy(&file, state == "pending", &label, m) {
                        reads.unreadable.push((file.clone(), e));
                    }
                    if m.source.is_empty() {
                        m.source = file.display().to_string();
                        m.modified = std::fs::metadata(&file)
                            .and_then(|md| md.modified())
                            .ok()
                            .and_then(|t| jiff::Timestamp::try_from(t).ok());
                    }
                }
                if state != "pending" && !m.recipients.contains(&label) {
                    m.recipients.push(label.clone());
                }
            }
        }
    }
    let mut events = Vec::new();
    for (id, m) in messages {
        // a file named like a message whose copies all lack a sender is not
        // a message andon can count; it is a gap, not a message
        if !m.headers.contains_key("from") && !m.headers.contains_key("issuer") {
            if !reads
                .unreadable
                .iter()
                .any(|(p, _)| p.display().to_string() == m.source)
            {
                reads.unreadable.push((
                    m.source.clone().into(),
                    "not a hail message (no sender)".to_owned(),
                ));
            }
            continue;
        }
        let Some(e) = message_event(&id, &m, &goal)? else {
            continue;
        };
        let sender_kept = e.actor.as_deref().is_some_and(keep);
        let recipient_kept = m.recipients.iter().any(|r| keep(r));
        if (sender_kept || recipient_kept) && since.is_none_or(|s| e.t >= s) {
            events.push(e);
        }
    }
    events.extend(reads.gaps_since(0, now, &["message"], None));
    // every message under the state directory from `since` to now
    events.push(crate::event::coverage(
        since.unwrap_or(crate::event::ALL_HISTORY),
        Some(now),
        &["message"],
        root.display().to_string(),
        None,
    ));
    Ok(events)
}

/// Merge one copy of a message into `m`. The sender's outbox copy
/// (`pending/`) names no sender: the seat directory it sits in is the sender.
fn read_copy(file: &Path, pending: bool, label: &str, m: &mut Message) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let (mut headers, body) = parse(&text);
    if pending && !headers.contains_key("from") && !headers.contains_key("issuer") {
        headers.insert("from".to_owned(), label.to_owned());
    }
    for (k, v) in headers {
        m.headers.entry(k).or_insert(v);
    }
    if body.len() > m.body.len() {
        m.body = body;
    }
    Ok(())
}

/// The sender label, from the header lines alone.
fn header_from(file: &Path) -> Option<String> {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(std::fs::File::open(file).ok()?);
    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            break;
        }
        if let Some(v) = line
            .strip_prefix("from:")
            .or_else(|| line.strip_prefix("issuer:"))
        {
            return Some(label(v.trim()));
        }
    }
    None
}

/// `x-2a@%6` → `x-2a`; `y@%405` → `ferry`.
fn label(raw: &str) -> String {
    raw.split('/')
        .next()
        .unwrap_or(raw)
        .split('@')
        .next()
        .unwrap_or(raw)
        .to_owned()
}

/// `1013T231056-5aeb.inline.md` → `1013T231056-5aeb`.
fn message_id(name: &str) -> Option<&str> {
    let id = name.get(..16)?;
    let b = id.as_bytes();
    let ok = b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'T'
        && b[5..11].iter().all(u8::is_ascii_digit)
        && b[11] == b'-'
        && b[12..].iter().all(u8::is_ascii_hexdigit);
    ok.then_some(id)
}

fn parse(text: &str) -> (BTreeMap<String, String>, String) {
    let mut headers = BTreeMap::new();
    let mut lines = text.lines();
    for line in lines.by_ref() {
        if line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            if key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c == '-')
            {
                headers
                    .entry(key.to_owned())
                    .or_insert_with(|| v.trim().to_owned());
            }
        }
    }
    let body: Vec<&str> = lines.collect();
    (headers, body.join("\n"))
}

fn message_event(id: &str, m: &Message, goal: &Regex) -> Result<Option<Event>> {
    let h = &m.headers;
    let t = match h.get("time") {
        Some(t) => parse_instant(t)?,
        None => match m.modified.and_then(|mt| id_time(id, mt)) {
            Some(t) => t,
            None => return Ok(None),
        },
    };
    let mut e = Event::new(t, Kind::Message, m.source.clone());
    e.actor = h.get("from").or_else(|| h.get("issuer")).map(|f| label(f));
    e.ids.message = Some(id.to_owned());
    let headline = h
        .get("ask")
        .or_else(|| h.get("headline"))
        .cloned()
        .unwrap_or_default();
    let text = format!("{headline}\n{}", m.body);
    let goals = crate::goal::find_all(goal, &text);
    // the subject is the goal named first, the headline's if it names one
    e.subject = crate::goal::find(goal, &text);
    e.ids.goal.clone_from(&e.subject);
    e.set_attr("goals", Value::from(goals));
    e.set_attr("message_kind", h.get("kind").cloned().unwrap_or_default());
    let mut to: Vec<String> = m.recipients.clone();
    if let Some(header) = h.get("to") {
        for r in header
            .split(',')
            .map(|r| label(r.trim()))
            .filter(|r| !r.is_empty())
        {
            if !to.contains(&r) {
                to.push(r);
            }
        }
    }
    to.sort();
    e.set_attr("to", Value::from(to));
    e.set_attr("re", h.get("re").cloned().unwrap_or_default());
    e.set_attr("headline", headline);
    Ok(Some(e))
}

/// A hail id's time: `MMDDTHHMMSS` in UTC. The id carries no year; it is the
/// year of the file's modification, or the year before when the id would
/// otherwise fall after the file was written.
fn id_time(id: &str, modified: jiff::Timestamp) -> Option<jiff::Timestamp> {
    let year = modified.to_zoned(jiff::tz::TimeZone::UTC).year();
    let at = |year: i16| {
        let text = format!(
            "{year}-{}-{}T{}:{}:{}Z",
            id.get(0..2)?,
            id.get(2..4)?,
            id.get(5..7)?,
            id.get(7..9)?,
            id.get(9..11)?
        );
        parse_instant(&text).ok()
    };
    let t = at(year)?;
    if t > modified { at(year - 1) } else { Some(t) }
}

/// For tests: a state directory's messages.
///
/// # Errors
/// As [`collect`].
pub fn from_dir(state: &Path, labels: &[&str], goal: &str) -> Result<Vec<Event>> {
    let config = Config::parse(
        &format!("[project]\ngoal_pattern = '{goal}'\n"),
        Path::new("/"),
    )?;
    let hc = HailConfig {
        state: state.display().to_string(),
        labels: labels.iter().map(|s| (*s).to_owned()).collect(),
        since: None,
    };
    collect(&config, &hc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{e}"));
        }
        std::fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn messages_are_read_with_their_recipients_and_goals() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let seats = dir.path().join("seats");
        let msg = "from: andon-1b/%51\nkind: ask\nid: 1013T231056-5aeb\ntime: 2026-10-13T23:10:56Z\nask: review andon-xa4\n\nbody naming andon-76c\n";
        write(&seats.join("andon/cur/1013T231056-5aeb.injected.md"), msg);
        write(&seats.join("andon-1b/pending/1013T231056-5aeb"), msg);
        let other = "from: y-1a\nkind: fyi\nid: 1013T100000-0001\ntime: 2026-10-13T10:00:00Z\nask: unrelated\n\n";
        write(&seats.join("y-2b/cur/1013T100000-0001.read.md"), other);
        let events = from_dir(dir.path(), &["andon*"], r"\bandon-[a-z0-9]{3}\b")
            .unwrap_or_else(|e| panic!("{e}"));
        let (covered, events): (Vec<_>, Vec<_>) =
            events.into_iter().partition(|e| e.kind == Kind::Coverage);
        assert_eq!(covered.len(), 1, "the state directory states its coverage");
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.actor.as_deref(), Some("andon-1b"));
        assert_eq!(e.attr_strs("to"), vec!["andon"]);
        assert_eq!(e.attr_strs("goals"), vec!["andon-76c", "andon-xa4"]);
        assert_eq!(e.attr_str("message_kind"), Some("ask"));
    }

    #[test]
    fn ids_are_recognised_and_dated() {
        assert_eq!(
            message_id("1013T231056-5aeb.inline.md"),
            Some("1013T231056-5aeb")
        );
        assert_eq!(message_id("hooked"), None);
        let written = parse_instant("2026-10-14T00:00:00Z").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            id_time("1013T231056-5aeb", written),
            parse_instant("2026-10-13T23:10:56Z").ok()
        );
        // a December id in a file written in January is last year's
        let january = parse_instant("2027-01-02T00:00:00Z").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            id_time("1231T120000-0001", january),
            parse_instant("2026-12-31T12:00:00Z").ok()
        );
    }
}
