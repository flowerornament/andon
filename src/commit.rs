//! A commit as an adapter describes it and a check reads it: below both, so
//! neither the adapters nor the checks import the other.

/// A commit, as `check commit` sees it.
#[derive(Debug, Clone, Default)]
pub struct Commit {
    pub id: String,
    pub title: String,
    /// The whole message, for projects that cite goals outside the subject.
    pub message: String,
    /// The committing seat (trailer, workspace, or `--seat`).
    pub seat: Option<String>,
}
