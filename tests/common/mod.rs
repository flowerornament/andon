//! Shared test support: one way to make a config, with its capture instant
//! set, as the command line sets it from its one clock read.

#![allow(
    dead_code,
    clippy::disallowed_methods,
    reason = "each test file uses part of this; a test reads the clock as the command line does"
)]

use std::path::Path;

use andon::config::Config;
use jiff::Timestamp;

/// A config from text, captured now.
#[must_use]
pub fn config(text: &str, root: &Path) -> Config {
    captured(text, root, Timestamp::now())
}

/// A config from text, captured at `at`.
#[must_use]
pub fn captured(text: &str, root: &Path, at: Timestamp) -> Config {
    let mut config = Config::parse(text, root).unwrap_or_else(|e| panic!("{e:#}"));
    config.capture = Some(at);
    config
}

/// A config file, captured now.
#[must_use]
pub fn load(path: &Path) -> Config {
    let mut config = Config::load(path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
    config.capture = Some(Timestamp::now());
    config
}
