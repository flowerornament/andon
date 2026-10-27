//! andon: the signal board for many agents on one repository, derived from
//! the artifacts they already write.
//!
//! Adapters read sources into one ledger of events; folds compute every
//! surface from the ledger; refusals exit non-zero and name the fix. andon
//! writes into no source: its effects are stdout and exit codes, and a
//! cache under the user's cache directory that changes no output.

pub mod adapters;
pub mod check;
pub mod cli;
pub mod commit;
pub mod config;
pub mod doctor;
pub mod event;
pub mod fold;
pub mod goal;
pub mod hook;
pub mod init;
pub mod ledger;
pub mod listing;
pub mod render;
pub mod seat;
pub mod time;
