//! Instants and windows. Time enters andon only here: when an adapter reads a
//! timestamp from an artifact, and when a surface is rendered "as of" a moment.
//! The fold itself never asks the clock.

use anyhow::{Context, Result, bail};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan};

/// Parse an artifact's timestamp. Accepts RFC 3339 with `Z` or an offset.
/// A timestamp without a zone is read as UTC, because every source andon reads
/// writes UTC; an adapter whose source does not must convert before calling.
///
/// # Errors
/// When the text is not a timestamp.
pub fn parse_instant(text: &str) -> Result<Timestamp> {
    let text = text.trim();
    if let Ok(t) = text.parse::<Timestamp>() {
        return Ok(t);
    }
    let civil: jiff::civil::DateTime = text
        .parse()
        .with_context(|| format!("not a timestamp: {text:?}"))?;
    Ok(civil.to_zoned(TimeZone::UTC)?.timestamp())
}

/// Where a source with a date filter must start reading for its coverage to
/// hold from `since`: two days earlier, as an RFC 3339 UTC instant. A tool's
/// own date parsing (local time, the current time of day) never decides what
/// the read misses, and a commit made before `since` that landed after it
/// is still read.
#[must_use]
pub fn read_from(since: Timestamp) -> String {
    since
        .checked_sub(jiff::SignedDuration::from_hours(48))
        .unwrap_or(since)
        .to_string()
}

/// Milliseconds since the epoch, as Ferry receipts record them.
///
/// # Errors
/// When the value is outside the representable range.
pub fn from_millis(ms: i64) -> Result<Timestamp> {
    Timestamp::from_millisecond(ms).with_context(|| format!("timestamp out of range: {ms} ms"))
}

/// The start of a UTC day.
///
/// # Errors
/// When the date cannot be placed in UTC (never, for real dates).
pub fn day_start(date: Date) -> Result<Timestamp> {
    Ok(date.to_zoned(TimeZone::UTC)?.timestamp())
}

/// A half-open window `[from, to)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from: Timestamp,
    pub to: Timestamp,
    /// The end asked for, when it lay after `now`: the window then ends at
    /// `now`, since nothing after it has been read.
    pub asked_to: Option<Timestamp>,
}

impl Window {
    #[must_use]
    pub fn contains(&self, t: Timestamp) -> bool {
        self.from <= t && t < self.to
    }

    /// Parse `DAY`, `FROM..TO` or `FROM..`. A date endpoint means whole days:
    /// `2026-10-11..2026-10-12` is both days. A timestamp endpoint is exact:
    /// `2026-10-11..2026-10-13T00:30:00Z` ends at 00:30. An open end needs
    /// `now`, which only the render step supplies.
    ///
    /// # Errors
    /// When an endpoint is neither a date nor a timestamp, or the window is empty.
    pub fn parse(text: &str, now: Timestamp) -> Result<Self> {
        let (from, to) = match text.split_once("..") {
            Some((from, to)) => (from, Some(to)),
            None => (text, None),
        };
        let start = endpoint(from, false)?;
        let end = match to {
            None => endpoint(from, true)?,
            Some("") => now,
            Some(to) => endpoint(to, true)?,
        };
        if end <= start {
            bail!("empty window {text:?}: its end is not after its start");
        }
        // a window that ends in the future counts up to now, and says so
        let (end, asked_to) = if end > now && start < now {
            (now, Some(end))
        } else {
            (end, None)
        };
        Ok(Self {
            from: start,
            to: end,
            asked_to,
        })
    }

    /// The UTC days the window touches, in order.
    #[must_use]
    pub fn days(&self) -> Vec<Date> {
        let mut days = Vec::new();
        let first = self.from.to_zoned(TimeZone::UTC).date();
        let mut day = first;
        loop {
            let Ok(start) = day_start(day) else { break };
            if start >= self.to {
                break;
            }
            days.push(day);
            match day.checked_add(1.day()) {
                Ok(next) => day = next,
                Err(_) => break,
            }
        }
        days
    }

    /// The length in fractional days, for per-day rates.
    #[must_use]
    pub fn length_days(&self) -> f64 {
        let span: SignedDuration = self.to.duration_since(self.from);
        span.as_secs_f64() / 86_400.0
    }
}

/// One endpoint: a date (whole day, its start or its end) or an instant.
fn endpoint(text: &str, end: bool) -> Result<Timestamp> {
    let text = text.trim();
    if let Ok(date) = text.parse::<Date>() {
        let date = if end {
            date.checked_add(1.day())?
        } else {
            date
        };
        return day_start(date);
    }
    parse_instant(text).with_context(|| format!("{text:?} is neither a date nor a timestamp"))
}

/// Minutes between two instants, as a float, for medians.
#[must_use]
pub fn minutes(from: Timestamp, to: Timestamp) -> f64 {
    to.duration_since(from).as_secs_f64() / 60.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        parse_instant(s).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn instants_parse_with_and_without_zone() {
        assert_eq!(ts("2026-10-11T00:18:27Z"), ts("2026-10-11T00:18:27"));
        assert_eq!(ts("2026-10-11T02:18:27+02:00"), ts("2026-10-11T00:18:27Z"));
        assert!(parse_instant("yesterday").is_err());
    }

    #[test]
    fn date_ranges_are_whole_days() {
        let now = ts("2030-01-01T00:00:00Z");
        let w = Window::parse("2026-10-11..2026-10-12", now).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(w.from, ts("2026-10-11T00:00:00Z"));
        assert_eq!(w.to, ts("2026-10-13T00:00:00Z"));
        assert_eq!(w.days().len(), 2);
        let single = Window::parse("2026-10-11", now).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(single.to, ts("2026-10-12T00:00:00Z"));
    }

    #[test]
    fn instant_endpoints_are_exact_and_empty_windows_refuse() {
        let now = ts("2030-01-01T00:00:00Z");
        let w = Window::parse("2026-10-11..2026-10-13T00:30:00Z", now)
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(w.to, ts("2026-10-13T00:30:00Z"));
        assert_eq!(w.days().len(), 3);
        assert!(Window::parse("2026-10-12..2026-10-11", now).is_err());
        let open = Window::parse("2026-10-11..", now).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(open.to, now);
    }
}
