---
title: the project boundary, and external adapters
date: 2026-10-14
status: living
amends: 2026-10-13-andon-founding.md
---

# The project boundary

andon is one tool for any project. Its code knows no project: no project's
name, desk layout, review protocol or evidence format. A project meets andon
at a boundary andon defines, and when its stores are its own, the project
builds the bridge. `tests/boundary.rs` fails when a project's name appears in
`src/`.

This came from building against one project first. Reading that project live,
the build put its conventions into andon's code (its parked-receipt layout,
its critic trailer and verdict comment, a config naming its seats, the
recorded format of its study) and the user drew the line on 2026-10-14.

## 1. Two contracts

A project reaches andon through exactly two things.

**Configuration** (`andon.toml`, in the project's own repository). Every
convention is a setting: seat names and their labels, the goal-id pattern and
where a commit cites it, trailer names, a verdict comment's format, receipt
locations, message labels, pools and WIP. Defaults are andon's own
conventions (the trailers `scripts/jj-identity` writes, goal sections Why,
Done when and Appetite), never another project's.

**The ledger** (JSONL, one event per line). It is what `andon ledger` prints
and `--ledger FILE` reads, and it is what an external adapter emits.

## 2. The event, as the contract states it

```
{
  "t":       RFC 3339 UTC, required: when the fact happened
  "t_end":   RFC 3339 UTC, optional: when it ended (a run, a span)
  "actor":   string, optional: who the source says acted, verbatim
  "kind":    land | freeze | commit | verdict | gate_run | run | measurement |
             message | create | claim | close | session_active | spawn |
             other:<name>
  "subject": string, optional: the goal the fact is about
  "ids":     { commit, change, tree, goal, verdict, receipt, gate_run,
               session, critic_session, message, cites: [verdict ids] }
  "outcome": pass | fail | go | return | refused | stopped | unknown
  "source":  string, required: the artifact the fact came from
  "attrs":   object: facts the folds read by name (recipe, repo, reason,
             failure_tail, message_kind, to, re, goals, title, appetite, pool,
             assignee, workspace, branch)
}
```

`seat` is not part of the contract: seat identity is andon's join (from
`actor`, sessions, workspaces and shared ids), and a `seat` an adapter sets is
discarded. Joins are by id. A fact is on the board only if some artifact
records it.

## 3. Core adapters and external adapters

**Core adapters** read general tools: jj, git, bd, GitHub, Ferry receipts,
hail, and Claude Code and Codex transcripts. They are andon's, and they read
each tool's own format only.

**External adapters** read anything else: a project's evidence store, its
desk conventions, its review records, a CI system andon does not know. Each
is a program (or a set of files) the project owns, declared by name:

```toml
[[adapters.external]]
name = "proj-review"                        # project, then what it reads
command = ["scripts/andon-review", "--since", "2026-10-01"]
contract = 1

[[adapters.external]]
name = "proj-study"
files = ["study/events/*.andon.jsonl"]
```

The protocol, version 1:

- The program prints events (section 2) on stdout as JSONL and exits 0.
- It reads; it never writes to the stores it reads.
- A non-zero exit, or a line that is not an event, is an error naming the
  adapter; `board` and the checks then exit 2 (andon cannot decide).
- andon marks every event it emits: `attrs.adapter` is the adapter's name and
  `source` is prefixed `adapter <name>:`. A reader of the ledger or the board
  can always tell a core fact from a bridged one.
- `contract` is the version the adapter speaks; andon refuses a version it
  does not read, naming both.

External adapters are named for their project (`proj-review`), live in the
project's repository, and are maintained there. andon's documentation may
list known ones; andon's code never contains one.

## 4. What moved

- The recorded adapter (the study's file format) left the product. The
  study is andon's founding history (read-only), so its translation lives
  as test support and a study test still pins every number.
- The originating project's config left andon. It is offered to that
  project as its own `andon.toml`; the project decides whether to adopt it.
- The Ferry adapter's knowledge of a parked-receipt directory became a rule:
  the seat is the path component the receipt pattern's last wildcard
  matched. A project with its own layout writes its pattern.
- The seat and verdict-citation trailers and the verdict comment format are
  read only when configured (`trailers.seat`, `trailers.critic`,
  `trailers.critic_id_word`, `[adapters.bd.verdicts]`).

## 5. Open

- A registry of known external adapters (a page listing name, project,
  contract version and what it reads) once there is more than one.
- Whether core adapters should themselves be external programs. Not yet: the
  core tools are few and general, and a single binary keeps the hook fast.

## Amendment: coverage (2026-10-14)

Contract 1 now includes coverage (`2026-10-14-coverage.md`): an external
adapter emits `coverage` events for the kinds and span it read, and a gap
for each source it could not read. Without them, the kinds it emits are not
covered, and every gauge that reads them is unknown.

## Amendment: current state (2026-10-14, round four)

A source that holds only current state (a tracker's export) emits
`state:<kind>` coverage, a span of one instant: when it was read. It vouches
for the state at that instant, never for history. A kind's state at an
instant is known if the kind is covered over all history, or if a
`state:<kind>` span holds that instant (`2026-10-14-coverage.md`, round four).
