//! What a read is doing, on stderr only (stdout is the API).
//!
//! On a terminal, one line shows while the sources are read: how many
//! adapters finished, the ones still running, and the time since the read
//! began; it is cleared when the read ends. `--timings` records each
//! adapter's wall time and event count, printed after the output. Neither
//! ever writes to stdout, and with neither asked for nothing is written.

use std::fmt::Write as _;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What the command line asked for.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Show the progress line (stderr is a terminal).
    pub line: bool,
    /// Record each adapter's time for `--timings`.
    pub timings: bool,
}

static OPTIONS: OnceLock<Options> = OnceLock::new();
static TIMINGS: Mutex<Vec<Timing>> = Mutex::new(Vec::new());

/// Set once, by the command line, before any read.
pub fn configure(options: Options) {
    let _ = OPTIONS.set(options);
}

fn options() -> Options {
    OPTIONS.get().copied().unwrap_or_default()
}

/// One adapter's read.
#[derive(Debug, Clone)]
pub struct Timing {
    pub adapter: String,
    pub wall: Duration,
    /// Events read, or `None` when the read failed.
    pub events: Option<usize>,
}

/// The timings recorded so far, taken.
#[must_use]
pub fn take_timings() -> Vec<Timing> {
    TIMINGS
        .lock()
        .map(|mut t| std::mem::take(&mut *t))
        .unwrap_or_default()
}

/// `--timings`, as text: one adapter a line, slowest first.
#[must_use]
pub fn timings_text(timings: &[Timing]) -> String {
    let mut sorted = timings.to_vec();
    sorted.sort_by(|a, b| b.wall.cmp(&a.wall).then_with(|| a.adapter.cmp(&b.adapter)));
    let width = sorted.iter().map(|t| t.adapter.len()).max().unwrap_or(0);
    let mut out = String::new();
    for t in sorted {
        let events = t
            .events
            .map_or_else(|| "failed".to_owned(), |n| format!("{n} events"));
        let _ = writeln!(
            out,
            "andon: {:<width$}  {:>7.2}s  {events}",
            t.adapter,
            t.wall.as_secs_f64()
        );
    }
    out
}

/// The progress line: `reading 4/7 · sessions, bd · 3.1s`.
#[must_use]
pub fn line(done: usize, total: usize, running: &[&str], elapsed: Duration) -> String {
    format!(
        "reading {done}/{total} · {} · {:.1}s",
        running.join(", "),
        elapsed.as_secs_f64()
    )
}

/// A read of several adapters at once, reporting as it goes.
pub struct Read {
    started: Instant,
    names: Vec<String>,
    done: Mutex<Vec<bool>>,
    options: Options,
}

/// How long a read runs before the line shows: a quick read draws nothing.
const QUIET: Duration = Duration::from_millis(250);
const TICK: Duration = Duration::from_millis(100);

impl Read {
    #[must_use]
    pub fn new(names: Vec<String>) -> Self {
        let n = names.len();
        Self {
            started: Instant::now(),
            names,
            done: Mutex::new(vec![false; n]),
            options: options(),
        }
    }

    /// Adapter `i` finished after `wall`, with `events` read.
    fn finished(&self, i: usize, wall: Duration, events: Option<usize>) {
        if let Ok(mut done) = self.done.lock()
            && let Some(d) = done.get_mut(i)
        {
            *d = true;
        }
        if self.options.timings
            && let Ok(mut t) = TIMINGS.lock()
        {
            t.push(Timing {
                adapter: self.names[i].clone(),
                wall,
                events,
            });
        }
    }

    /// Adapter `i` starts; it finishes when the guard is done or dropped.
    #[must_use]
    pub fn guard(&self, i: usize) -> Guard<'_> {
        Guard {
            read: self,
            i,
            started: Instant::now(),
            finished: false,
        }
    }

    fn all_done(&self) -> bool {
        self.done.lock().map_or(true, |d| d.iter().all(|x| *x))
    }

    fn current(&self) -> String {
        let done = self.done.lock().map(|d| d.clone()).unwrap_or_default();
        let running: Vec<&str> = self
            .names
            .iter()
            .zip(&done)
            .filter(|(_, d)| !**d)
            .map(|(n, _)| n.as_str())
            .collect();
        let finished = done.iter().filter(|d| **d).count();
        line(finished, self.names.len(), &running, self.started.elapsed())
    }

    /// Draw the line on stderr until every adapter finished, then clear it.
    /// Returns at once when no line was asked for.
    pub fn draw(&self) {
        if !self.options.line {
            return;
        }
        let mut drawn = false;
        while !self.all_done() {
            std::thread::sleep(TICK);
            if self.started.elapsed() < QUIET || self.all_done() {
                continue;
            }
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[2K{}", self.current());
            let _ = err.flush();
            drawn = true;
        }
        if drawn {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[2K");
            let _ = err.flush();
        }
    }
}

/// One adapter's part of a read.
pub struct Guard<'r> {
    read: &'r Read,
    i: usize,
    started: Instant,
    finished: bool,
}

impl Guard<'_> {
    /// The adapter finished, with `events` read (`None`: it failed).
    pub fn done(mut self, events: Option<usize>) {
        self.finished = true;
        self.read.finished(self.i, self.started.elapsed(), events);
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.read.finished(self.i, self.started.elapsed(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_names_what_is_still_running() {
        assert_eq!(
            line(4, 7, &["sessions", "bd"], Duration::from_millis(3_140)),
            "reading 4/7 · sessions, bd · 3.1s"
        );
    }

    #[test]
    fn timings_list_the_slowest_first() {
        let t = |a: &str, ms, n| Timing {
            adapter: a.into(),
            wall: Duration::from_millis(ms),
            events: n,
        };
        assert_eq!(
            timings_text(&[
                t("jj", 120, Some(10)),
                t("sessions", 2_500, Some(3)),
                t("bd", 50, None)
            ]),
            "andon: sessions     2.50s  3 events\nandon: jj           0.12s  10 events\nandon: bd           0.05s  failed\n"
        );
    }
}
