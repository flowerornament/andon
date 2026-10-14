---
title: what the first acceptance changed
date: 2026-10-14
status: living
amends: 2026-10-13-andon-founding.md
---

# What the first acceptance changed

A reviewer seat accepted andon against the originating project's live stores
with defects (the review is kept in that project). The fixes, and the
decisions in them:

- **Ferry output logs (D1).** Ferry keeps a local run's output at
  `local/v1/runs/<id>/`, `local/runs/<id>/` or `runs/<id>/` in the store
  holding the receipts. All three are read; the project's no-reason count is 377
  of 436, as the reviewer recounted, not 436.
- **A live test the gate runs (D2).** `tests/live.rs` reads machine-local
  suites (`~/.config/andon/live.toml`): a project's own config, a window, and
  the scorecard values it must give. andon's repository holds no project's
  suite; on a machine with one, `just check` checks the adapters against real
  stores, not only the fold against recorded files.
- **The lead goal is the work (D6).** A seat's goals are ordered by the
  latest commit or landing citing each; the claim order breaks ties.
- **Done-when must observe something (D7).** A done-when can fail only if it
  names a command, a number, or a configured check word (`goal.checkable`);
  a denylist of vague verbs let "the board is good" pass.
- **Message kinds (D8)** match without case, and a kind the messenger does
  not define (`messages.kinds`) is refused.
- **The hook (D9)** reads only what one row needs: every core source but the
  messenger, and only the seat's sessions from its earliest held claim. It
  groups what changed (`landed 40 (goal ×8, …)`), puts returns and refusals
  first, and caps the line at 280 bytes. On the originating project: 1.0–1.7 s and under 400
  bytes, from 9.5–13 s and 4 KB. Bad input is named in one line on stdout,
  still exit 0.
- **Reading faster everywhere.** Adapters run at once; transcripts are read
  in parallel, from a binary-searched `since`, parsing only lines that can
  matter; jj's op log stops two days before `since`. The project's full scorecard:
  2 s, from 10.
- **No landing (D10)** reads "N runs and no landing since <date>".
- **Definitions (D4, D5).** "Unowned" is open with neither a claim nor an
  assignee (847 on the originating project, the study's 849 less two changed since);
  "received" leaves out a seat's messages to itself.

D3 (scope) is the originating project's config: its reviews receipts, the
ferry repository's seats, and a gates source (an external adapter) are the
project's to add.

## Round three (re-acceptance)

The reviewer re-accepted with defects; amended here:

- **No-landing origin (D10).** "N runs and no landing since D" now dates D
  from the later of the ledger's landing coverage (its earliest landing of
  any seat) and the seat's earliest counted run, and counts runs from D.
  Nothing before the landing source's horizon says a seat did not land.
- **The hook's row is the board's row (N1).** The scoped read filters
  sessions by the directory they are attributed to, never the one they
  started in; `tests/hook_board.rs` asserts the two rows are equal.
- **Wiring is checkable.** `andon hook --check` reports the config, seat,
  workspace, transcript and row a payload resolves to, and exits 1 naming
  the fix; on a prompt whose transcript cannot be read, the hook says so.
  The hook itself still exits 0.
- **D6 and D10 are pinned** by unit tests that fail on the old code.
- **Done-when, positively (D7).** It must name a command, a standalone
  number, or a check outcome as a whole word; still a heuristic, a floor.
- **The live suite left the land gate (N2).** `just live` runs it on
  purpose; `cargo test` ignores it. A suite's `[suite.stores]` is checked
  first, so a changed store reads "the stores changed", not a regression.
  This replaces the line above saying `just check` runs it.
- A transcript reader that stops is an error, not a shorter ledger; jj's op
  log failure carries jj's own message.
