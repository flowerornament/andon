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

    /// Parse a window a person typed, read in `zone`: `DAY`, `FROM..TO`,
    /// `FROM..` (each endpoint as [`endpoint_in`] reads it), or `6h`, `2d`
    /// for the last hours or days up to `now`.
    ///
    /// # Errors
    /// When the text is none of these, naming what is accepted, or the
    /// window is empty.
    pub fn parse_in(text: &str, now: Timestamp, zone: &TimeZone) -> Result<Self> {
        let text = text.trim();
        if let Some(span) = ago(text) {
            let from = now
                .checked_sub(span)
                .map_err(|_| anyhow::anyhow!("{text:?} reaches before any time andon can read"))?;
            return Ok(Self {
                from,
                to: now,
                asked_to: None,
            });
        }
        let (from, to) = match text.split_once("..") {
            Some((from, to)) => (from, Some(to)),
            None => (text, None),
        };
        let window = || anyhow::anyhow!("{text:?} is not a window; {ACCEPTED_WINDOW}");
        let start = endpoint_in(from, now, zone, false).map_err(|_| window())?;
        let end = match to {
            // one relative endpoint is the span from it up to now
            None if from.trim().starts_with('-') => now,
            None => endpoint_in(from, now, zone, true).map_err(|_| window())?,
            Some("") => now,
            Some(to) => endpoint_in(to, now, zone, true)?,
        };
        if end <= start {
            bail!("empty window {text:?}: its end is not after its start");
        }
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

/// What a person may type for a time; every refusal of one says so.
pub const ACCEPTED: &str = "use a date (2026-10-24), a time (2026-10-24T12:00, in the local zone, or with Z or an offset), now, today, yesterday, -6h or -2d";

/// What a person may type for a window.
pub const ACCEPTED_WINDOW: &str =
    "use a day (2026-10-24, today, yesterday), FROM..TO, FROM.., or the last 6h or 2d";

/// One endpoint a person typed, read in `zone`: a date, `today` or
/// `yesterday` is a whole day (its start, or with `end` its end); a time with
/// `Z` or an offset is exact; a time without one is in `zone`; `now`, `-Nh`,
/// `-Nd` and `-Nm` count back from `now`.
///
/// # Errors
/// When the text is none of these, naming what is accepted.
pub fn endpoint_in(text: &str, now: Timestamp, zone: &TimeZone, end: bool) -> Result<Timestamp> {
    let text = text.trim();
    let refuse = || anyhow::anyhow!("{text:?} is not a time; {ACCEPTED}");
    let day = match text {
        "now" => return Ok(now),
        "today" => Some(now.to_zoned(zone.clone()).date()),
        "yesterday" => Some(now.to_zoned(zone.clone()).date().yesterday()?),
        // jiff reads a date out of a datetime too; only a bare date is a day
        _ if text.contains(['T', 't', ' ']) => None,
        _ => text.parse::<Date>().ok(),
    };
    if let Some(date) = day {
        let date = if end { date.tomorrow()? } else { date };
        return Ok(date.to_zoned(zone.clone())?.timestamp());
    }
    if let Some(back) = text.strip_prefix('-') {
        let span = ago(back).ok_or_else(refuse)?;
        return now.checked_sub(span).map_err(|_| refuse());
    }
    if let Ok(t) = text.parse::<Timestamp>() {
        return Ok(t);
    }
    let civil: jiff::civil::DateTime = text.parse().map_err(|_| refuse())?;
    Ok(civil
        .to_zoned(zone.clone())
        .map_err(|_| refuse())?
        .timestamp())
}

/// `6h`, `2d` or `30m` as a duration.
fn ago(text: &str) -> Option<SignedDuration> {
    let (n, unit) = text.split_at(text.len().checked_sub(1)?);
    let n: i64 = n.parse().ok().filter(|n| *n > 0)?;
    let secs = match unit {
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return None,
    };
    Some(SignedDuration::from_secs(n.checked_mul(secs)?))
}

/// One endpoint of a window written for andon's own sources: a date is a
/// whole UTC day, a time without a zone is UTC.
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

    /// Los Angeles as a POSIX rule: a build sandbox has no zone database.
    const LOS_ANGELES: &str = "PST8PDT,M3.2.0,M11.1.0";

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
    fn a_typed_date_is_read_in_the_readers_zone() {
        let now = ts("2026-10-27T06:42:00Z");
        let pdt = TimeZone::posix(LOS_ANGELES).unwrap_or_else(|e| panic!("{e}"));
        let at = |text: &str, end: bool| {
            endpoint_in(text, now, &pdt, end).unwrap_or_else(|e| panic!("{e}"))
        };
        // 06:42Z is still the 26th in Los Angeles
        assert_eq!(at("today", false), ts("2026-10-26T07:00:00Z"));
        assert_eq!(at("today", true), ts("2026-10-27T07:00:00Z"));
        assert_eq!(at("yesterday", true), ts("2026-10-26T07:00:00Z"));
        assert_eq!(at("2026-10-24", true), ts("2026-10-25T07:00:00Z"));
        assert_eq!(at("2026-10-24T12:00", false), ts("2026-10-24T19:00:00Z"));
        assert_eq!(at("2026-10-24T12:00Z", false), ts("2026-10-24T12:00:00Z"));
        assert_eq!(at("-6h", false), ts("2026-10-27T00:42:00Z"));
        assert_eq!(at("now", false), now);
    }

    #[test]
    fn a_bad_time_names_what_is_accepted_not_the_parser() {
        for bad in ["7x", "tomorrow", "-0h", "2026-13-01"] {
            let error = endpoint_in(bad, ts("2026-10-27T00:00:00Z"), &TimeZone::UTC, false)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert_eq!(error, format!("{bad:?} is not a time; {ACCEPTED}"));
        }
        let error = Window::parse_in("soon", ts("2026-10-27T00:00:00Z"), &TimeZone::UTC)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert_eq!(
            error,
            format!("\"soon\" is not a window; {ACCEPTED_WINDOW}")
        );
    }

    #[test]
    fn typed_windows_count_back_from_now_or_cover_whole_days() {
        let now = ts("2026-10-27T06:00:00Z");
        let w = |text: &str| {
            Window::parse_in(text, now, &TimeZone::UTC).unwrap_or_else(|e| panic!("{e}"))
        };
        assert_eq!(
            (w("6h").from, w("6h").to),
            (ts("2026-10-27T00:00:00Z"), now)
        );
        assert_eq!(w("2d").from, ts("2026-10-25T06:00:00Z"));
        assert_eq!(w("-6h").from, ts("2026-10-27T00:00:00Z"));
        assert_eq!(w("yesterday").from, ts("2026-10-26T00:00:00Z"));
        assert_eq!(w("yesterday").to, ts("2026-10-27T00:00:00Z"));
        // today ends tonight: counted to now, and the asked end is kept
        assert_eq!(w("today").to, now);
        assert_eq!(w("today").asked_to, Some(ts("2026-10-28T00:00:00Z")));
        assert_eq!(w("-2d..yesterday").to, ts("2026-10-27T00:00:00Z"));
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
