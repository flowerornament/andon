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

## Amendment: board schema 3 (2026-10-19, andon-3im)

Each row gains `state`, present only where a source of live sessions was
read (`[adapters.presence]`). It is a gauge: `null` when no session runs at
the seat, else `status` (`busy`, `idle`, or `live` where the harness records
none), `since` (when that status began) and `sessions` (how many run). The
text gains a `state` column, under the same condition. The flag
`idle Nh holding G` is new. Every schema 2 field is unchanged.

## Amendment: board schema 3, overlaps (2026-10-20, andon-tgv)

Schema 3 also holds each row's `overlaps[]`: every other seat whose
workspace holds unlanded changes to files this seat's does. Each entry has
`seat`, `files` (every shared path, sorted) and `snapshot` (when that
workspace was last snapshotted, or `null`). It is a current-state gauge
over jj's workspaces, so it is `{"unknown": ...}` at an instant they were
not read, or where one could not be read. `flags[]` gains `overlaps <seat>
on N files (...)`, and `overlaps ?` where a workspace was looked at and
could not be read. The columns are unchanged. Schema 3 is unreleased, so
this joins it rather than moving the number again.

## Amendment: items closed (2026-10-20, andon-nqi)

`tracker.closed` is the number of items closed in the window: an item
closed, reopened and closed again counts once, as branching (items spawned
per item closed) needs. Before bd's events were read, no source could
record a second close, so no earlier output changes, and the scorecard
stays at schema 2.

## Amendment: board schema 4 (2026-10-27, andon-tlz)

The board's text columns are renamed to read without the help
(`.design/2026-10-27-first-class-interface.md` §6): `last` is `landed`,
`since-land` is `unlanded`, `findings` is `comments`. The hook's line
labels its cells with them, so the board moves to schema 4. No JSON field
changes; schema 4's fields are schema 3's. Ages in text read `19h ago`.
The overlap flag in `flags[]` reads `overlaps <seat>, <seat> (N files)`,
one flag for all the seats a row overlaps; `overlaps[]` still lists every
path. The scorecard's text columns `no-reason`, `goal%` and
`asks-no-goal` are now `unexplained`, `on-goal` and `asks-off-goal`; its
text is unversioned and its JSON stays at schema 2.

## Amendment: goals and seats, schema 1 each (2026-10-27, andon-2fh)

`andon goals --json` and `andon seats --json` are new outputs, each with
its own `schema: 1`. `tests/schema.rs` records their fields.

- **goals:** `schema`, `asof`, `goals[]`, a gauge over the tracker's state.
  Each entry has `id`, `title`, `status` (`open`, `claimed` or `closed`),
  `owner`, `seated` (whether the owner is a configured seat), `item_type`,
  `created_at`, `claimed_at`, `closed_at`, `budget` (as written), `amount`,
  `unit` (the unit `used` counts in) and `used` (a gauge, or `null` for a
  goal not claimed).
- **seats:** `schema`, `asof`, `window_hours`, and `seats[]`. Each seat has
  `seat`, `aliases`, `prefixes`, `workspace`, `session` (`status`, `since`,
  `sessions`, or `null`) and `joined{kind}` (the facts in the window that
  joined to it). Then `unseated[]`: each name recorded that joins no seat,
  with `identity`, `facts{kind}` and `fix`.

`andon doctor` is text for people, not a contract; its exit code is: 0
when every configured source reads, 1 naming the fix otherwise.
