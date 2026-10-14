---
title: milestones 2 and 3, what the build decided
date: 2026-10-13
status: living
amends: 2026-10-13-andon-founding.md
---

# Milestones 2 and 3: what the build decided

Milestone 2 (andon-8ig) wired the refusals into this repository's own choke
points; milestone 3 (andon-dfp) added the git and GitHub adapters. This
records the choices the founding design left open, and what an independent
review of milestone 1 changed. It follows `2026-10-13-instrument-decisions.md`.

## 1. The land verb checks itself

`scripts/jj-land.sh` runs `andon check commit` on every commit it would
publish, before the gate, and refuses on the first refusal with andon's own
line. The default command is `cargo run -- check commit`: the candidate's own
andon, so a change to a check is enforced by the change itself. `LAND_CHECK`
overrides it (tests point it at a built binary and a kept ledger). The first
landing through it was its own, and the first refusal on this
repository named its fix: `cites no goal; end its subject with (andon-8ig)`.

## 2. Seat identity is set at session start

`scripts/jj-identity` exports `BEADS_ACTOR` as the workspace name through
Claude Code's session environment, so a seat's claims and closes carry the
seat. bd enforces the same rule from its side: it refused to close a goal for
an actor who was not its owner. Codex has no session environment for hooks;
a codex seat claims with `--assignee`.

## 3. The hook

`scripts/andon-hook.sh` runs the workspace's newest built andon (never
building one) for SessionStart and UserPromptSubmit, in Claude Code and
Codex. andon itself exits 0 from `hook` on any error, including a bad command
line, because Claude Code treats exit 2 as "block the prompt".

## 4. Conventions that GitHub needed, all configuration

- `project.goal_in = "message"`: a commit may cite its goal anywhere in its
  message (`Closes #12`), not only in its subject.
- A goal pattern's capture group is the goal id: `(?:Closes|Fixes) (#\d+)`
  yields `#12`, the id the github adapter gives issue 12.
- `[adapters.git] pr_pattern`: the pull-request number in a trunk subject,
  which becomes the landing's `ids.change = "pr#N"`.
- `branch_seats`: a branch prefix names a seat, for merges and CI runs.

## 5. Joins by change and by commit

A review on GitHub is attached to a pull request, not cited by a trailer. A
verdict whose adapter marks it `join = "change"` joins its landing by change
id; other verdicts join only through the landing's citations, which is what
keeps the study's 97 cited landings and 14.7-minute review time exact (the
study's verdicts also carry change ids, and joining them by change moved
both numbers; the pinned tests caught it).

Seat resolution gained two steps. A landing or change start whose actor is
no seat takes the seat of its change (a git author name gives way to the pull
request's author). An event whose source names no one takes its commit's seat
(a CI run). An event whose source names a workspace that is not a seat, such
as a shared desk's runs, stays unattributed: who ran something is a fact,
and whose commit it measured is a different fact. A landing that names no
goal takes the goal its change closes.

## 6. What review changed in milestone 1

An independent review found fifteen defects; the ones that changed design:

- `asof` showed outcomes before they happened. A past board now folds the
  ledger as it could have been known: an event still running has no outcome
  or end. The test that should have caught it compared the board with
  itself; it now builds the captured ledger independently.
- Ties at one instant sort create, claim, close.
- jj reads resolve one operation for the whole read. With divergent
  operation heads, `--at-op @` errors, and plain jj would merge the heads (a
  write); andon reads the newest head instead.
- A bd verdict belongs to the reviewed change's seat; its comment's author is
  the reviewer. Only a started issue is a claim.
- A commit cites its subject's last goal match (the convention ends the
  subject with it), and `check commit` prefers a candidate the tracker has.
- `check message` refuses a decision naming a goal the tracker lacks.

## 7. Open

- The originating project's land verb and hail's send are the founding design's other choke
  points. Both are other projects; wiring them is theirs to decide. hail has
  no pre-send hook today.
- With no `[seats]`, a person is two seats on GitHub: the git author name and
  the account login. An alias joins them; nothing derivable does.
- Reading GitHub costs seconds (four `gh` calls); a hook on a GitHub project
  should read a kept ledger.
