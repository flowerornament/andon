//! messenger: hail. Reads the state directory (`~/.local/state/hail`).
//!
//! A message is a file `seats/<label>/{new,cur,owed}/<id>.<state>.md`: header
//! lines (`from`, `to`, `kind`, `id`, `time`, `re`, `ask` or `headline`), a
//! blank line, then the body. The directory it sits in names a recipient;
//! `pending/` holds the sender's copy and names none. Goal ids are found in
//! the headline and body with the project's goal pattern.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use regex::Regex;
use serde_json::Value;

use crate::adapters::Reads;
use crate::config::{Config, HailConfig, wildcard};
use crate::event::attr;
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
    let now = config.capture()?;
    // every listing is recorded: a directory that cannot be listed held
    // messages to and from unknown seats, a gap for every seat
    let mut reads = Reads::default();
    if !root.try_exists().unwrap_or(true) {
        // no seats directory: nothing was read, so nothing is covered
        return Ok(Vec::new());
    }
    // every copy, in listing order; then each read at once, then merged in
    // that order, so the merge is the same as reading them one by one
    let copies = copies(&mut reads, &root);
    // what each copy says: its whole text in a seat of ours; in another
    // project's seat only its sender first (it matters only if one of ours
    // sent it)
    let read: Vec<Copy> = parallel(&copies, |c: &(String, &'static str, PathBuf, String)| {
        let (label, file) = (&c.0, &c.2);
        let when = modified(file);
        if since.is_some_and(|s| when.is_some_and(|m| m < s)) {
            return Copy::Old;
        }
        if keep(label) {
            Copy::Text(
                std::fs::read_to_string(file).map_err(|e| e.to_string()),
                when,
            )
        } else {
            Copy::Sender(header_from(file), when)
        }
    });
    for ((label, state, file, id), copy) in copies.iter().zip(read) {
        let (text, when) = match copy {
            Copy::Old => continue,
            Copy::Text(text, when) => (Some(text), when),
            Copy::Sender(from, when) => {
                if !messages.contains_key(id) {
                    match from {
                        Ok(from) if from.as_deref().is_some_and(keep) => {}
                        Ok(_) => continue,
                        // whose message it is cannot be told: a gap
                        Err(why) => {
                            reads.unreadable.push((file.clone(), why));
                            continue;
                        }
                    }
                }
                (None, when)
            }
        };
        let m = messages.entry(id.clone()).or_default();
        if m.source.is_empty() || *state != "pending" {
            let text =
                text.unwrap_or_else(|| std::fs::read_to_string(file).map_err(|e| e.to_string()));
            if let Err(e) = text.map(|t| merge_copy(&t, *state == "pending", label, m)) {
                reads.unreadable.push((file.clone(), e));
            }
            if m.source.is_empty() {
                m.source = file.display().to_string();
                m.modified = when;
            }
        }
        if *state != "pending" && !m.recipients.contains(label) {
            m.recipients.push(label.clone());
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
            // no time header, and an id whose year the file cannot date
            reads
                .unreadable
                .push((m.source.clone().into(), "a message with no time".to_owned()));
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

/// Every copy of a message under `root`, in listing order: the seat it
/// sits with, its state directory, the file and the message's id.
fn copies(reads: &mut Reads, root: &Path) -> Vec<(String, &'static str, PathBuf, String)> {
    let mut out = Vec::new();
    for seat_dir in reads.list(root) {
        let label = label(&seat_dir.file_name().unwrap_or_default().to_string_lossy());
        for state in ["new", "cur", "owed", "pending"] {
            for file in reads.list(&seat_dir.join(state)) {
                let name = file
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                if let Some(id) = message_id(&name) {
                    let id = id.to_owned();
                    out.push((label.clone(), state, file, id));
                }
            }
        }
    }
    out
}

/// When a file was last written, if that can be read.
fn modified(file: &Path) -> Option<jiff::Timestamp> {
    std::fs::metadata(file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| jiff::Timestamp::try_from(t).ok())
}

/// One copy, as read.
enum Copy {
    /// Older than `since`: not read.
    Old,
    /// Its text, and when it was last written.
    Text(Result<String, String>, Option<jiff::Timestamp>),
    /// Its sender alone, and when it was last written.
    Sender(Result<Option<String>, String>, Option<jiff::Timestamp>),
}

/// `f` over `items`, on every core at once, the results in order.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let chunk = items.len().div_ceil(workers.max(1)).max(1);
    std::thread::scope(|s| {
        let f = &f;
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| s.spawn(move || part.iter().map(f).collect::<Vec<R>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

/// Merge one copy of a message into `m`. The sender's outbox copy
/// (`pending/`) names no sender: the seat directory it sits in is the sender.
fn merge_copy(text: &str, pending: bool, label: &str, m: &mut Message) {
    let (mut headers, body) = parse(text);
    if pending && !headers.contains_key("from") && !headers.contains_key("issuer") {
        headers.insert("from".to_owned(), label.to_owned());
    }
    for (k, v) in headers {
        m.headers.entry(k).or_insert(v);
    }
    if body.len() > m.body.len() {
        m.body = body;
    }
}

/// The sender label, from the header lines alone.
fn header_from(file: &Path) -> Result<Option<String>, String> {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(std::fs::File::open(file).map_err(|e| e.to_string())?);
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            break;
        }
        if let Some(v) = line
            .strip_prefix("from:")
            .or_else(|| line.strip_prefix("issuer:"))
        {
            return Ok(Some(label(v.trim())));
        }
    }
    Ok(None)
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
    e.set_attr(attr::GOALS, Value::from(goals));
    e.set_attr(
        attr::MESSAGE_KIND,
        h.get("kind").cloned().unwrap_or_default(),
    );
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
    e.set_attr(attr::TO, Value::from(to));
    e.set_attr(attr::RE, h.get("re").cloned().unwrap_or_default());
    e.set_attr(attr::HEADLINE, headline);
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
pub fn from_dir(
    state: &Path,
    labels: &[&str],
    goal: &str,
    capture: jiff::Timestamp,
) -> Result<Vec<Event>> {
    let mut config = Config::parse(
        &format!("[project]\ngoal_pattern = '{goal}'\n"),
        Path::new("/"),
    )?;
    config.capture = Some(capture);
    let hc = HailConfig {
        state: state.display().to_string(),
        labels: labels.iter().map(|s| (*s).to_owned()).collect(),
        since: None,
    };
    collect(&config, &hc)
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "fixtures write the messages they read"
)]
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
        let events = from_dir(
            dir.path(),
            &["andon*"],
            r"\bandon-[a-z0-9]{3}\b",
            jiff::Timestamp::UNIX_EPOCH,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let (covered, events): (Vec<_>, Vec<_>) =
            events.into_iter().partition(|e| e.kind == Kind::Coverage);
        assert_eq!(covered.len(), 1, "the state directory states its coverage");
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.actor.as_deref(), Some("andon-1b"));
        assert_eq!(e.attr_strs(attr::TO), vec!["andon"]);
        assert_eq!(e.attr_strs(attr::GOALS), vec!["andon-76c", "andon-xa4"]);
        assert_eq!(e.attr_str(attr::MESSAGE_KIND), Some("ask"));
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
