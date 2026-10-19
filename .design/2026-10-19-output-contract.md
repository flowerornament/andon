---
title: the output contract
date: 2026-10-19
status: decided
amends: 2026-10-13-andon-founding.md
---

# The output contract

stdout is an API (AGENTS.md). Two of andon's outputs are read by programs:
`--json` from `board`, `asof` and `scorecard`, which a project's tooling
reads, and `andon hook`, whose stdout a harness puts into an agent's turn.
Until andon-3eu neither had a stated contract, and the JSON schema moved
from 1 to 2 with no notice to its one consumer. This document is the
contract. `tests/schema.rs` holds the code to it.

## What is versioned

Each JSON output carries `schema`, a whole number. The schema number moves
when any field is added, removed or renamed, or when its type or meaning
changes. `tests/schema.rs` records every field path for each schema
number, and fails when the output's fields differ from the record for its
number. Moving the number means recording the new fields there and here.

The board's text columns are versioned with the board's schema, since the
hook's line labels its cells with them.

Not versioned:

- the wording of reasons, notes and fix lines;
- the layout of text output other than the hook's line;
- the order of keys in a JSON object.

## Values

A gauge is either its value, or `{"unknown": "<reason>"}` where the ledger
could not look (`.design/2026-10-14-coverage.md`). It is never 0 for
"unknown". A reader handles both forms for every gauge field. Times are
RFC 3339 UTC. `null` is a known absence: no landing yet, no verdict, a
budget with no amount.

## Board, schema 2 (`board --json`, `asof --json`)

- `schema`, `asof`, `window_hours`: the board's instant, and the hours its
  counts cover.
- `seats[]`, one per seat:
  - `seat`;
  - `goals[]`: goals held, the lead first, each with `id`, `title`,
    `claimed_at`, `budget` (as written), `amount` and `unit` (as parsed)
    and `used` (spent since the claim, in `unit`);
  - `landings`, `last_land`, `runs`, `runs_pass`, `runs_since_land`;
  - `last_verdict`, with `outcome`, `t` and `subject`;
  - `findings`;
  - `flags[]`: the text of each flag raised.
- `unseated[]`: goals claimed by an identity that is no seat, with `goal`
  and `owner`. The text groups them, one line per owner.

With no `[goal]` section the text drops the goal and budget columns. The
JSON keeps `goals`, which is a tracker fact
(`.design/2026-10-19-goal-schema-as-config.md`).

## Scorecard, schema 2 (`scorecard --json`)

- `schema`, `from`, `to`, `asked_to` (only when the window ended after
  the capture), `days`.
- `seats{seat}`, `total`, `unattributed`: cards, each with
  - `landings`;
  - `runs{total, pass, fail, other, fail_without_reason}`;
  - `runs_per_landing`, `gate_runs`;
  - `verdicts_go`, `verdicts_return`;
  - `messages_sent`, `messages_received`, `messages_naming_goal`,
    `decisions_without_goal`;
  - `items_created`, `wip_at_end`, `active_hours`.
- `flow`: `lead_median_min`, `lead_n`, `review_median_min`, `review_n`,
  `gate_median_min`, `gate_n`.
- `tracker`: `created`, `closed`, `branching`, `open_at_end`,
  `unowned_at_end`.
- `hub`: `seat`, `inbound`, `inbound_per_day`, `decisions_without_goal`,
  `reply_median_min`, `reply_n`, or `null` with no messages.
- `joins`: `landings`, `landing_session`, `landing_session_seen`,
  `landings_citing_verdict`, `cites`, `cites_resolved`, `runs`,
  `runs_attributed`.
- `runs_by_repo{repo}`: run counts per repository.

## Hook

`andon hook` prints nothing, or:

```
andon: <seat>  <goal>  budget <cell>  lands <cell>  …  ⚑ <flags>
  since your last turn: <change>; <change>; … N more
```

- The first line is the seat's board row: each non-empty cell after its
  column's name. The goal cell is bare, and flags follow `⚑`.
- The second line appears on a prompt when something changed. It is
  capped at 280 bytes.
- A line starting `andon: hook cannot` says why the hook could not
  decide.
- The hook exits 0 always: it never fails a session.

## Exit codes

- 0: rendered, or passed;
- 1: a check refused, with one line naming the fix;
- 2: andon could not decide, with one line saying what to repair.

## Amendment: budgets in minutes (2026-10-19, andon-co4)

`seats[].goals[].unit` gains the value `minutes`, for a budget read from a
numeric tracker field such as bd's `estimated_minutes`. Its `used` is then
in minutes too. The field and its meaning are unchanged, so the schema
stays 2. Its values are `agent-hours`, `wall-clock`, `runs` and `minutes`,
or `null` where the budget states no amount.
