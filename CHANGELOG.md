# Changelog

All notable changes to `andon` are documented in this file.

## Unreleased

## v0.3.0 - 2026-10-19

Supervision you can build on: every attribute and adapter is held to a
stated contract, the board is one projection with typed gauges, and two
new facts, who is running now and which unlanded files two seats share.

- **Presence** (`.design/2026-10-19-overlap-and-presence.md` §2). With
  `[adapters.presence]`, andon reads Claude Code's per-session files and
  the process table: the board gains a `state` column (`busy 4m`, `idle
  3h`, `—`) and flags `idle Nh holding G` past `[board] idle_minutes`
  (default 60). It is current state, so `asof` an earlier instant shows
  `?`. Without the adapter there is no column.

- A session that moves between workspaces counts for each seat's own part:
  every activity span, spawn and edit is credited by the directory its own
  tool calls ran in, never by where the session mostly worked. A span
  splits where the session moved.

- **Overlapping unlanded edits** (`.design/2026-10-19-overlap-and-presence.md`
  §1). The jj adapter reads each workspace's unlanded files: the net change
  since it forked from the trunk, as of its last snapshot, whatever tool
  wrote them. The board flags `overlaps S on N files (...)` where two
  seats' workspaces hold the same file, and `--json` lists every shared
  path under `seats[].overlaps` (board schema 3). The hook says so when a
  shared file changed since the last turn. The sessions adapter also emits
  `edit` for each file an edit tool call wrote.

- Live acceptance is pinned to a kept ledger: each suite in
  `~/.config/andon/live.toml` names one (`ledger = FILE`, from `andon
  ledger`), and `cargo test` folds it, so only a change in andon's folds
  moves a number. `just live` is now a smoke test: every configured
  adapter reads its live stores and reports coverage.

- A budget can come from a numeric tracker field: `[goal.fields]
  "Budget" = "estimated_minutes"` (the unit from a `_minutes` or `_hours`
  ending), or `{ field = "...", unit = "..." }`. The board, the hook's
  over-budget flag and `check goal` read it. Budgets in minutes render as
  `min`.

- **No `[goal]`, no goal checks** (`.design/2026-10-19-goal-schema-as-config.md`).
  A config without `[goal]` gets no goal checks: `check goal` and `check
  commit` pass with a note, the board drops its goal and budget columns,
  and the WIP and over-budget flags are not raised. Declaring `[goal]`,
  even empty, keeps the founding schema; `andon init` declares it.
- The `--json` output and the hook's line have a stated contract
  (`.design/2026-10-19-output-contract.md`). `tests/schema.rs` fails when
  a field changes without the schema number moving. The board moves to
  schema 3 (`state` and `overlaps` are new; every schema 2 field is
  unchanged); the scorecard stays at schema 2.
- Goals claimed by an identity that is no seat are one line per identity,
  with a count and the fix. `--json` still lists every goal.
- `[project] land_recipe` (default `land`) names the land verb's gate
  recipe, which was hardcoded.
- An unreadable transcript blanks active hours only for windows reaching
  back before its last write.
- Library callers must set `config.capture` before reading sources; the
  clock is no longer read as a fallback.

- Event attributes are a declared contract (`src/event.rs`, `attr::ALL`):
  one name, shape and meaning per key. An external adapter's declared key
  of the wrong shape is refused; an undeclared key is carried, never read.
- `reason` is split into `gap_reason`, `close_reason`, `run_reason` and
  `logs`; a kept ledger or an adapter that writes `reason` is still read.
  Kept trailers (`adapters.jj.trailers.keep`) move under `attrs.trailers`.
  Board, scorecard and asof output is unchanged.
- No adapter drops input silently. A malformed bd verdict, a dateless
  comment or issue, a missing or unreadable GitHub snapshot list, an
  unreadable hail header, and a transcript read error or mid-file
  malformed line are each a gap, so the gauges that read them show `?`
  with the reason. A gap whose time is known (an unreadable verdict)
  stands only over the spans that hold it. GitHub issue events are read
  to the last page. `tests/contract.rs` holds every adapter to this.
- jj reads its operation from the store's op heads, and no longer claims
  coverage of `freeze`.
- A budget with no amount renders in its own unit, and a project without
  bd is never given a bd command in a fix line.

## v0.2.0 - 2026-10-14

The first adopter's fixes, and a rename that breaks 0.1.0 configs and JSON.

- The git adapter refuses a configured path that is not the top level of
  its own git repository, rather than answering for an enclosing one, and
  an inherited `GIT_DIR` (or work tree, index, object store) no longer
  redirects a read.
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
