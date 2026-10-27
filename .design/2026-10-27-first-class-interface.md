---
title: a first-class interface
date: 2026-10-27
status: decided
amends: 2026-10-19-output-contract.md
---

# A first-class interface

andon's folds are sound; its surfaces are not yet a tool a person reaches
for. A review on 2026-10-27 ran every command in four repositories
(andon, murail, herald, girard) and found:

- `board` takes 10 to 18 seconds on the adopters' repositories and prints
  nothing while it reads, on a terminal or not.
- herald's board is 336 columns wide and murail's 200, on an 80-column
  terminal: overlap flags list file paths once per other seat, in the last
  column of a table that never measures the terminal.
- Nothing is emphasized: no color, flags in plain text at the right edge,
  idle seats as rows of zeros, alignment that flips with content, the
  header's time in UTC with no zone, `(1 days)`.
- Names a reader must look up (`since-land`, `no-reason`, `asks-no-goal`,
  `findings` for what is a comment count, `last`, `ah`).
- Help that leaves `--json`, every positional argument, `--seat` and
  `--goal` undescribed, shows no example, prints nothing for
  `hook --help`, and names `hook --check`, which no help lists.
- Time input that takes no `today`, `yesterday` or `7d`, quotes the parser
  on a bad date, reads a zone-less time as UTC, and a bug: `asof DATE`
  shows the day's start though its help says its end.
- No way to list goals, see the seat roster, or ask what each source read
  and how long it took; an `init` that detects nothing and writes hail
  0.6's kinds.
- A README that argues the theory and the study, shows no board, and has
  no config reference.

The refusals are the part already first-class: one line, the fix named.
Every surface should meet that bar.

## What does not change

The invariants hold. Specifically:

- **stdout is an API.** Color, progress and width fitting apply only when
  the stream is a terminal. Piped stdout (an agent's Bash call, a script)
  gets plain text, with no escape codes. `--json` and the hook's line are
  untouched by layout, color and progress. `NO_COLOR` is honored, and
  `--color auto|always|never` overrides.
- **Progress is stderr**, and only when stderr is a terminal.
- **No clock in a fold.** The display zone (below) is read at render, like
  the capture instant; folds see only UTC timestamps.
- **Writes nothing.** No `init --write`; a cache, if one is ever needed
  for speed, lives under the user's cache directory, is keyed by each
  source file's identity, and can be deleted with no change to any output.

## 1. A board that fits (goal: board fits the terminal)

- **Width.** On a terminal the board, `asof` and the scorecard fit its
  width (`terminal_size`; `COLUMNS` overrides). Piped, text is not
  truncated, but the compact forms below still apply, since an agent pays
  for every column too.
- **Flags compact.** A flag in the table names what and whom, never a file
  list: `overlaps herald-0b, herald-1a (2 files)`. The files go beneath the
  table in an `overlaps` section, one line per pair of seats, each file
  once. `--json` keeps every path in `seats[].overlaps`.
- **Flags that do not fit** move beneath their row, indented, rather than
  widening the table.
- **Idle seats fold.** A seat with no goal, nothing in the window, no live
  session and no flag is not a row; one line says how many and that
  `--all` shows them. `--seat S` always shows S.
- **Alignment is per column, declared,** not inferred from the cells: a
  count is right-aligned whether it holds numbers, `?` or `—`.
- **Footnotes are short.** Each `?` reason prints once, with the gauge and
  rows it hides, trimmed of parser internals; `andon doctor` (§5) is where
  the full source error lives.
- **The unseated-goals line** moves beneath the table and its reasons.

## 2. Speed and progress (goal: a board in seconds, with progress)

- On a terminal, stderr shows one line while sources are read:
  `reading 4/7 · sessions, bd · 3.1s`, cleared when the read ends.
- `--timings` (global) prints each adapter's wall time and event count to
  stderr after the output.
- Profile murail, herald and girard, and make the slowest reads cheap:
  skip transcripts whose last write is before the window's horizon, read
  only what the window needs. The target is a `board` under 3 s on
  murail warm, and the hook under 1 s. Record before/after per adapter on
  the goal.

## 3. Help and docs (goal: every surface explains itself)

- Every argument and flag has a description; `--json` says which schema
  it prints. Every command's help ends with two or three examples.
- `andon` with no command runs `board`.
- Commands are listed in the order a reader needs them: board, goals,
  seats, asof, scorecard, check, hook, ledger, doctor, init.
- `hook --help` and `hook --version` print; only a malformed hook command
  line is swallowed (a hook still never fails a session).
- `--config` and `--ledger` appear once, as global options, not on every
  subcommand's list (`init` reads neither).
- `check goal` names every missing section, not "and 1 more".
- README: one paragraph of what, a sample board, a quickstart from
  `andon init` to the hook, the commands, a configuration reference (every
  table and key, with its default), then why and the design corpus. The
  study and milestones move to `.design/`, which already holds them. The
  diagram drops `peat` (agent memory is never an input).

## 4. Time (goal: time reads and prints in the reader's zone)

- **Display zone:** the system's local zone (`TZ` honored). Text headers
  print the zone: `board as of 2026-10-26 23:42 PDT`. JSON stays RFC 3339
  UTC. `TZ=UTC andon …` reproduces a UTC view, so `asof` still reproduces
  a past board byte-for-byte for a given zone.
- **Input:** a date or a zone-less time is read in the display zone; a
  time with `Z` or an offset is exact. Accepted everywhere a time is:
  `today`, `yesterday`, `-Nh`, `-Nd`, and as a window `Nh` or `Nd` (the
  last N hours or days, ending now). `scorecard` with no window is
  `today`.
- **`asof DATE` means that date's end**, as its help says (the bug).
- **Errors** say what is accepted, not what the parser expected:
  `"7x" is not a time; use 2026-10-24, 2026-10-24T12:00, today, yesterday, -6h or -2d`.
- `1 day`, not `1 days`; spans print as `24h` or `7d 6h`, not `7.28 days`.

## 5. Commands (goal: list goals, seats and sources)

- **`andon goals`** (the list): open goals, one row each: id, owner (seat,
  or the identity that is no seat), status, budget spent of allowed, age
  since claim, title (truncated to fit). `--seat`, `--all` (closed too),
  `--json` (its own `schema: 1`, recorded in `tests/schema.rs`).
- **`andon seats`**: the roster from config, each seat with its aliases
  and prefixes, its workspace if one is found, its live session, and what
  in the window joined to it; then every identity seen in the sources that
  joins to no seat, with the fix. `--json`.
- **`andon doctor`**: the config's path; each configured adapter with
  read ok or the full error, events read, the span it covers, its gaps
  and its wall time; then the hook's wiring (what `hook --check` reports).
  Exit 0 when every configured source reads, 1 naming the fix otherwise.
- **`andon init` detects** what it can from the directory it runs in:
  project name, jj or git and the trunk's name, bd and its issue prefix
  (for `goal_pattern`), jj workspaces as seats, Claude Code session files
  for presence. Messages use hail 0.7's kinds (`decision_kinds =
  ["ask"]`). It still prints to stdout.

## 6. Visual and names (goal: the board reads at a glance)

- On a terminal: the header line bold; flags colored by kind (over budget,
  refused and idle-holding red; WIP, no landing and overlaps yellow);
  `?` and `—` dim; a seat whose row has a flag has its name colored to
  match. Color is applied after layout, so width math never counts escape
  codes.
- **Renames (text only; JSON fields keep their names):**

  | was | now | why |
  |---|---|---|
  | board `last` | `landed` | last what |
  | board `findings` | `comments` | it counts comments |
  | board `since-land` | `unlanded` | runs since the seat last landed |
  | scorecard `no-reason` | `unexplained` | failed runs with no reason or log |
  | scorecard `goal%` | `on-goal` | share of messages naming a goal |
  | scorecard `asks-no-goal` | `asks-off-goal` | decisions naming no goal |
  | budget unit `ah` | `agent-h` | |

- Ages read as ages: `17h ago`, `idle 3h`; a verdict `GO 17h ago`.
- The scorecard, when wider than the terminal, prints as stacked tables
  by group (flow; messages; tracker and activity), each led by the seat.

## Contract

The board's text columns are versioned with its schema
(`2026-10-19-output-contract.md`), since the hook's line labels cells with
them. The renames in §6 move the board to **schema 4** with no JSON field
changed; `tests/schema.rs` records COLUMNS_V4. `goals --json` starts at
schema 1. The wording and layout of every other text output stay
unversioned.

## Order and ownership

Two seats work in parallel, in files that do not overlap:

- andon-1b: §2 (adapters, progress), then §5 (new commands, init).
- andon-1a: §4 (time, the `asof` bug), §1 (board layout), §6 (visual and
  names), then §3 (help and README), last because it documents the rest.

`cli.rs` is shared: §5 adds commands, §3 and §4 edit argument docs and
parsing. Whoever lands second rebases.

## Amendment: speed as built (2026-10-27, andon-j4a)

- **What was slow.** Profiled one adapter at a time (load average above
  100 throughout, so the times are what an agent sees). The slow reads:
  - murail: Codex rollouts (6 GB kept) at 22 s, and jj's operation log at
    20 s. The log holds 44,731 operations, nearly all since the configured
    `since`, and jj loads each one.
  - girard: transcripts at 57 s, because its config reads every project
    on the machine with no `since`, and hail's 49k files at 8 s.
  - herald: bd at 8 s, nearly all of it bd's events query, which filters
    long text columns.
- **A cache, under the user cache directory** (`$ANDON_CACHE`, else
  `$XDG_CACHE_HOME/andon`, else `~/.cache/andon`; `ANDON_CACHE=off`
  disables it). Each entry is checked against its source before use, so
  deleting the cache changes no output. It never lives in a repository or
  a tracker. Three things are kept:
  - Transcript parses, with where each read stopped. Transcripts are
    append-only, so the next read resumes there. An entry is used only
    while the file's device, inode and the bytes just before the stop
    still match.
  - jj's operation log. Operations are immutable: the log is read from
    the newest down to the first kept operation from which every
    operation still owed is kept, and the rest comes from the cache,
    reachable through parents.
  - bd's event rows, by id. Rows are only ever added or pruned, so only
    ids not seen before are fetched, and a pruned id drops out.
- **Parallel reads.** jj's four reads and its per-workspace reads run at
  once, and so do bd's export and events query and hail's file reads.
  Merges keep the old order.
- **Transcripts are read whole.** The seek to near `since` in large files
  is gone: a parse is kept from the start. A span that crossed `since` now
  starts at its first call, not wherever the seek landed. No board or
  scorecard on the three repositories changed.
- **What cannot get there.** The hook is bound by `bd export` (1.4 to
  2.2 s warm on murail and herald). That is bd's own query over the whole
  tracker, and no cheaper read gives the same state.

## Amendment: goals lists the held ones by default (2026-10-27, andon-9ci)

§5 had `andon goals` list every open goal. On the largest tracker read in
acceptance that is 1,202 rows, 1,132 of them open goals nobody has claimed, which buries the answer to
the list's own question: who is holding what.

- **With no flags,** the list holds only the goals someone holds (claimed,
  with an owner), newest claim first. One line beneath counts the rest:
  `1132 open goals no one holds, 126 of them assigned (--open lists them)`.
  The line is left out when there are none, and under `--seat`.
- **`--open`** adds the open goals no one has claimed, newest first. A goal
  assigned but not claimed is one of these: an assignment names an owner,
  not a seat at work on it.
- **`--all`** adds the closed goals as well.
- **`--json`** honors the same filters and gains `unheld` and `assigned`,
  the two counts; goals is schema 2.
- **The footnote names each owner that is no seat,** with how many goals it
  holds: `* Shared User (57) is no seat; andon seats names the fix`.

On that tracker the default list is 77 lines.
