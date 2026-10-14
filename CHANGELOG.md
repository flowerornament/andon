# Changelog

All notable changes to `andon` are documented in this file.

## Unreleased

- From the first adopter's use of 0.1.0:
  - `check commit @` resolves a revision in the caller's jj workspace when
    it is a workspace of the configured repository, not in the configured
    checkout's working copy.
  - `[goal.fields]` maps a goal section to a tracker field (bd's
    `acceptance_criteria`, a GitHub issue field), read where the
    description lacks the section.
  - A goal claimed by an identity that is no seat (bd's default user) is
    named, with the fix: set `BEADS_ACTOR=<seat>` or reassign it. `check
    commit` says so instead of "claim it", and the board lists such goals.
    `andon init` and the README lead with seat identity.
  - A scorecard window that ends after the capture instant counts up to it
    and says so.
  - `[goal] types` (default feature, bug, task) decides what counts toward
    WIP; an epic does not.
  - Hubs (`[messages] hubs`) get no landing flags.
  - Findings count comments on the seat's lead goal since its claim.
- A goal's effort limit is its **budget**, everywhere, with no alias: the
  required `## Budget` section, the board's `budget` column, the flag
  `over budget on <goal>`, and `board --help`, which defines it. Breaking
  for 0.1.0 configs and JSON readers: `goal.budget` and `board.budget_unit`
  are the config keys; a goal's JSON has `budget` (as written), `amount`
  and `unit`.

## v0.1.0 - 2026-10-14

The first release: the board, its folds and its refusals, over the
artifacts a many-agent repository already writes.

- Surfaces: `board` (one row per seat: goal, budget spent, landings,
  runs, last verdict, findings, flags), `scorecard` (the four laws of flow
  per seat over a window), `asof` (a past board, folded from the ledger as
  it could have been known then), `hook` (the seat's row and what changed
  since its last turn, for Claude Code and Codex), `ledger` (the event
  ledger as JSONL, for a project to keep) and `init`.
- Refusals: `check commit` (cites a goal its seat holds, within WIP),
  `check goal` (why, a done-when that can fail, a finite budget, room in
  its pool) and `check message` (a decision names a goal). Exit 1 and one
  line that names the fix; exit 2 when andon cannot decide.
- Adapters, all read-only: jj, git, bd, GitHub (issues, reviews, Actions),
  Ferry receipts, Claude Code and Codex transcripts, and hail. Every
  project convention is configuration in `andon.toml`.
- Coverage: every adapter records the kinds and span it read, and a gap
  naming each path it could not list or read. A gauge whose sources do not
  cover its span reads `?` with the reason, never 0. A tracker export
  vouches for the goals held only at the instant it was read.
- One gauge type: every value the scorecard and board print is known or
  unknown, built only through a registry of the kinds it reads. Property
  tests check that every printed cell is registered, that every fault
  reaches every gauge that reads it, and that registrations name every kind
  a value depends on.
- External adapters (`[[adapters.external]]`, contract 1): a project's own
  program or files emit ledger events, marked with the adapter's name.
  andon's code names no project (`tests/boundary.rs`).
- Fixed in acceptance (an independent reviewer seat, six rounds): false zeros before a
  source's horizon, unreadable directories skipped silently, tracker
  history claimed from current state, and date-only `since` read in local
  time by jj and git.
- Every headline number of the originating study is reproduced from its
  recorded ledger (verified privately); `just live` checks a machine's live suites
  against their stores.

## v0.0.0 - 2026-10-12

- Project scaffold: the `andon` binary prints its version and help; no
  surface is implemented yet. Never tagged.
