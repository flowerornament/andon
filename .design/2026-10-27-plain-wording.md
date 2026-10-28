---
title: plain wording
date: 2026-10-27
status: decided
amends: 2026-10-27-first-class-interface.md
---

# Plain wording

A reader of 0.4.0 asked what this line means:

```text
not shown, no source reads them: runs, pass, unlanded, verdict
```

Their verdict on lines like it ("no source reads them", "no goals held"):
they read as generated prose, not as a tool's output. The causes are the
same everywhere:

- **Data is made the actor.** "no source reads them", "the tracker does
  not have", "the messenger defines", "a decision ... decides nothing",
  "andon cannot decide". A tool reports state; it does not narrate what
  its inputs think.
- **Private verbs.** andon says *holds*, *cites*, *names*, *joins*,
  *covers* and *carries* where every tool a programmer uses says
  *claimed*, *references*, *has*, *matches*, *available* and *is*.
- **"no X" fused to a verb.** "names no seat", "cites no goal", "joins
  no seat", "no goals held". The idiom is "has no seat", "doesn't
  reference a goal", "no claimed goals".
- **Generic nouns for named tools.** "the tracker", "the messenger",
  "the runner", "a source": the reader configured bd, hail, ferry and
  GitHub by name, and the output should use those names.
- **A layout that hides the cause.** The line above lists four columns
  but not what is missing (a CI source, a review source) or what to add.

The bar: each line reads as if git, cargo, gh or bd printed it. Where
lean production has the standard name for a flow concept (andon is its
word), that name is used: WIP, cycle time, rework, stalled.

## Grammar

- **Severity prefixes are cargo's and git's:** `error:`, `warning:`,
  `ok:`, lowercase, then the problem. `check` prints `ok:` or `error:`
  (not `refused:`); `warn:` becomes `warning:`. andon's own failure is
  `error:` (not `andon: cannot decide:`), exit 2 as now.
- **The fix follows on the same line**, after `; ` in the imperative,
  with the exact command or config key in backticks-free plain text, as
  now: `error: abc123 doesn't reference a goal; add (andon-xyz) to its
  subject: jj describe -r abc123`. The one-line rule stands.
- **Subjects are the things themselves:** the commit, the goal, the seat,
  the config key, the tool by name (bd, jj, git, gh, hail, ferry). Never
  "the tracker", "the messenger", "the runner", "the sources".
- **Empty states are short and lowercase:** `no claimed goals`, `no
  goals`, `nothing to show`. No period.
- **Hidden rows and columns say how many and how to show them:**
  `4 inactive seats hidden (--all to show): a, b, c, d`.
- **Unknown values (`?`) say why in the words of configuration:** "not
  configured", "not available before 2026-10-10", "current state only (no
  history)", "read failed: ...". Not "no source reads", "are read from".
- **Contractions are fine** (doesn't, isn't, can't), as git and cargo use
  them.
- **No andon-internal nouns in output:** fold, gauge, projection,
  capture instant, coverage, facts. The ledger's unit is an *event*.

## Glossary

| was | now | where |
|---|---|---|
| holds, held (a goal) | claimed | goals, board, check |
| cites, names (a goal) | references | check, hook |
| joins (to) a seat | matches a seat | seats, board |
| names that join no seat | unmatched names | seats |
| the tracker | bd (or gh), by the adapter's name | everywhere |
| the messenger | hail | check message |
| a source / the sources | the adapter, by its table: `[adapters.bd]` | reasons, doctor |
| no source reads X | X: not configured | `?` reasons, board footer |
| covers, is read from D | available from D | `?` reasons, doctor |
| read only as current state | current state only (no history) | `?` reasons |
| cannot decide | error | cli, hook |
| refused (a check) | error | check |
| the fix / names the fix | (the imperative itself) | everywhere |
| quiet seat | inactive seat | board |
| lead goal | current goal | board help |
| facts | events | seats |
| carries a decision | is a decision message (ask) | check message |
| no landing in Nd | stalled Nd | board flag |
| last land refused | last land failed | board flag |
| N runs since last landing | N runs, nothing landed | board flag |
| idle Nh holding G | idle Nh with G claimed | board flag |
| runs/land | tries | scorecard (text) |
| change start → land | cycle time | scorecard summary |
| hub | busiest inbox | scorecard summary |
| joins (summary) | attribution | scorecard summary |
| (more accounts appear with nothing counted) | N more with no activity (--json lists them) | scorecard |

Kept, with the reason:

- **seat**: a named agent identity that outlives its sessions and maps to
  a workspace; "agent" means the running process. It is the `[seats]`
  config key and hail's address. Help defines it once.
- **land, landing**: `arc land`, jj and merge queues use it; it is the
  `land_recipe` key. "merge" would be wrong for a rebase-and-push.
- **goal, budget, WIP, verdict, ledger**: andon's model, each defined in
  `andon --help` and the README.
- **The board's column names** (schema 4) stay. No text column changes,
  so the board stays at schema 4 and the hook's labels are unchanged.
  The flag wording changes: flags are not versioned (the output contract
  versions columns, not cell text).

## The board's footer for missing sources

A column dropped because nothing configured feeds it is grouped by what
is missing, with the config that turns it on:

```text
not configured:
  runs, pass, unlanded  CI runs: add [adapters.ferry] or [adapters.github] runs = true
  verdict               review verdicts: add [adapters.bd.verdicts]
```

One line per missing input; dimmed on a terminal; wrapped to the width
with the hint indented under itself. The same table opens `andon doctor`'s
output after the adapters, under `columns off:`. `--json` is unchanged
(`{"unknown": "..."}` per gauge, now in the new reason wording).

## A check that can fail

`tests/wording.rs` scans every string literal outside `#[cfg(test)]` in
`src/` and refuses the phrasings above: `no source reads`, `cannot
decide`, `the tracker`, `the messenger`, `the runner`, ` holds `, `held`,
`cites`, `names no`, `joins no`, `join no`, `quiet`, `refused:`, `warn:`.
A test makes it fire on a planted string.

## Ownership

- andon-1a: `render.rs`, `listing.rs`, `doctor.rs`, `hook.rs`,
  `fold/board.rs` flags, `fold/coverage.rs` reasons, `cli.rs` and every
  `///` help line, README, the footer, and `tests/wording.rs`.
- andon-1b: `check.rs`, `goal.rs`, `seat.rs`, `config.rs`, `time.rs`,
  `init.rs`, `ledger.rs` and `src/adapters/*`: every error and hint they
  return.

Tests that pin old wording change with it. Whoever lands second rebases.
