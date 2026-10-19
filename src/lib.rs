//! andon: the signal board for many agents on one repository, derived from
//! the artifacts they already write.
//!
//! Adapters read sources into one ledger of events; folds compute every
//! surface from the ledger; refusals exit non-zero and name the fix. andon
//! writes nothing anywhere: its only effects are stdout and exit codes.

pub mod adapters;
pub mod check;
pub mod cli;
pub mod commit;
pub mod config;
pub mod event;
pub mod fold;
pub mod goal;
pub mod hook;
pub mod ledger;
pub mod render;
pub mod seat;
pub mod time;
